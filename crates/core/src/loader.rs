//! Turns library picks into playable kits (design D16, D18, D20, D21).
//!
//! The sound seed picks one file per kit slot ([`pick_drums`]); a pinned
//! file (dropped or rolled on its own) replaces the seed's pick. Loading
//! decodes each file, brings it to [`SAMPLE_LOUDNESS_TARGET`] and resamples it
//! to the host rate. Everything here allocates and reads files: run it off
//! the audio thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use basedrop::Shared;
use serde::{Deserialize, Serialize};

use crate::error::AudioFileError;
use crate::kit::{grid_slices, share, DrumKit};
use crate::library::{Library, Pool, Tag};
use crate::loudness::{measure_hit_lufs, measure_lufs};
use crate::sampler::{SampleData, LEVEL_PARTS};
use crate::types::StereoSample;

/// Loudness every sample is brought to before the knobs (as in Mesh live).
pub const SAMPLE_LOUDNESS_TARGET: f64 = -12.0;
/// Largest cut and boost of the normalization: a quiet file with a loud
/// click must not come up so far that the click dominates.
const CUT_MAX_DB: f64 = 24.0;
const BOOST_MAX_DB: f64 = 12.0;
/// Root of a mudpie whose pitch is unknown (F1, as the Mesh mudpies).
pub const DEFAULT_MUDPIE_ROOT: u8 = 29;
/// MIDI key one-shots are stored at (they play unpitched).
const HIT_ROOT: u8 = 60;
/// Break chops per bar of loop.
const CHOPS_PER_BAR: u32 = 16;

// MUSICAL: Mesh live's sub-voice balance (style-calibrate, user 2026-10-15):
// the kit's voices sum on one lane, so the quieter roles sit below the
// backbone. Applied on top of each sample's normalization.
/// Level of a slot against the kick and snare, in dB.
pub fn slot_offset_db(slot: Slot) -> f64 {
    match slot {
        Slot::Kick | Slot::Snare => 0.0,
        Slot::Break => -2.0,
        Slot::HatClosed | Slot::HatOpen => -8.0,
        Slot::CymbalA | Slot::CymbalB | Slot::Shaker => -6.0,
        Slot::Perc => -10.0,
    }
}

/// A drum kit slot (design D16; the shaker comes from the cymbal pool).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Slot {
    Kick,
    Snare,
    HatClosed,
    HatOpen,
    CymbalA,
    CymbalB,
    Shaker,
    Perc,
    Break,
}

impl Slot {
    pub const ALL: [Slot; 9] =
        [Slot::Kick, Slot::Snare, Slot::HatClosed, Slot::HatOpen, Slot::CymbalA, Slot::CymbalB, Slot::Shaker, Slot::Perc, Slot::Break];

    /// The pool the slot picks from and the tag it prefers.
    pub fn source(self) -> (Pool, Option<Tag>) {
        match self {
            Slot::Kick => (Pool::Kick, None),
            Slot::Snare => (Pool::Snare, None),
            Slot::HatClosed => (Pool::Hats, Some(Tag::None)),
            Slot::HatOpen => (Pool::Hats, Some(Tag::OpenHat)),
            Slot::CymbalA => (Pool::Cymbal, Some(Tag::Ride)),
            Slot::CymbalB => (Pool::Cymbal, Some(Tag::Crash)),
            Slot::Shaker => (Pool::Cymbal, Some(Tag::Shaker)),
            Slot::Perc => (Pool::Perc, None),
            Slot::Break => (Pool::Break, None),
        }
    }
}

/// The files of a kit, by slot.
pub type DrumPicks = BTreeMap<Slot, PathBuf>;

/// The seed's file per slot, with `pins` replacing it. A shaker slot only
/// takes a file tagged as a shaker; the open hat falls back to a closed one.
pub fn pick_drums(lib: &Library, seed: u32, pins: &DrumPicks) -> DrumPicks {
    let mut out = DrumPicks::new();
    for (i, slot) in Slot::ALL.into_iter().enumerate() {
        if let Some(p) = pins.get(&slot) {
            out.insert(slot, p.clone());
            continue;
        }
        let (pool, prefer) = slot.source();
        let pick = lib.pick(pool, seed, i as u8, prefer);
        let pick = match slot {
            Slot::Shaker => pick.filter(|e| e.tag == Tag::Shaker),
            _ => pick,
        };
        if let Some(e) = pick {
            out.insert(slot, e.path.clone());
        }
    }
    // Two cymbal slots on the same file: drop the second.
    if out.get(&Slot::CymbalA).is_some() && out.get(&Slot::CymbalA) == out.get(&Slot::CymbalB) && !pins.contains_key(&Slot::CymbalB) {
        out.remove(&Slot::CymbalB);
    }
    out
}

/// The seed's mudpie, unless one is pinned.
pub fn pick_mudpie(lib: &Library, seed: u32, pin: Option<&Path>) -> Option<PathBuf> {
    pin.map(Path::to_path_buf).or_else(|| lib.pick(Pool::Mudpie, seed, 0, None).map(|e| e.path.clone()))
}

/// A loaded drum kit.
pub struct LoadedDrums {
    pub kit: DrumKit,
    /// Tempo of the break loop, for playing it at the host tempo (D29).
    pub break_bpm: Option<f32>,
    /// Slots whose file failed to load, with the reason.
    pub failed: Vec<(Slot, String)>,
}

/// Load the picked files at `sample_rate`.
pub fn load_drums(lib: &Library, picks: &DrumPicks, sample_rate: u32) -> LoadedDrums {
    let mut kit = DrumKit::default();
    let mut failed = Vec::new();
    let mut break_bpm = None;
    for (&slot, path) in picks {
        // One-shots start at their onset; a break keeps its grid.
        let trim = if slot == Slot::Break { Trim::None } else { Trim::FirstHit };
        let sample = match load_trimmed(path, HIT_ROOT, sample_rate, slot_offset_db(slot), trim) {
            Ok(s) => s,
            Err(e) => {
                failed.push((slot, e.to_string()));
                continue;
            }
        };
        match slot {
            Slot::Kick => kit.kick = Some(share(sample)),
            Slot::Snare => kit.snare = Some(share(sample)),
            Slot::HatClosed => kit.hats_closed.push(share(sample)),
            Slot::HatOpen => kit.hats_open.push(share(sample)),
            Slot::CymbalA => kit.rides.push(share(sample)),
            Slot::CymbalB => kit.crashes.push(share(sample)),
            Slot::Shaker => kit.shakers.push(share(sample)),
            Slot::Perc => kit.perc.push(share(sample)),
            Slot::Break => {
                let entry = lib.entry(path);
                let beats = entry.and_then(|e| e.beats).unwrap_or(4).max(1);
                break_bpm = entry.and_then(|e| e.bpm);
                let chops = (beats * CHOPS_PER_BAR / 4).clamp(1, 64) as usize;
                kit.break_loop = Some(share(grid_slices(sample, chops)));
            }
        }
    }
    // A kit without a closed hat plays its open hat closed.
    if kit.hats_closed.is_empty() && !kit.hats_open.is_empty() {
        kit.hats_closed = std::mem::take(&mut kit.hats_open);
    }
    LoadedDrums { kit, break_bpm, failed }
}

/// Load a mudpie at its root key (the library's, else [`DEFAULT_MUDPIE_ROOT`]).
pub fn load_mudpie(lib: &Library, path: &Path, sample_rate: u32) -> Result<Shared<SampleData>, AudioFileError> {
    let root = lib.entry(path).and_then(|e| e.root).unwrap_or(DEFAULT_MUDPIE_ROOT);
    // A mudpie plays from seeded positions across the whole file: its
    // silent stretches go, so every position sounds.
    load_trimmed(path, root, sample_rate, 0.0, Trim::Gaps).map(share)
}

/// Which silence a load cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trim {
    None,
    /// Before the onset and after the tail.
    Edges,
    /// The edges and every silent stretch inside.
    Gaps,
    /// A one-shot: the first hit only, up to its first silent stretch, and
    /// at most [`ONE_SHOT_MAX_SECS`] (a file of several hits plays one).
    FirstHit,
}

// MUSICAL: first draft, tune by ear.
/// Silence: this far below the loudest 10 ms of the file.
const SILENCE_DB: f32 = -40.0;
/// Inside a file, only silent stretches at least this long go.
const MIN_GAP_SECS: f64 = 0.05;
/// Sound kept around each loud stretch (attacks, tails).
const KEEP_SECS: f64 = 0.02;
/// Crossfade where a gap was cut.
const JOIN_SECS: f64 = 0.005;
/// Longest one-shot, faded out over [`ONE_SHOT_FADE_SECS`] at the end.
const ONE_SHOT_MAX_SECS: f64 = 3.0;
const ONE_SHOT_FADE_SECS: f64 = 0.03;

/// Cut silence from `frames` as `trim` says.
fn trim_silence(frames: Vec<StereoSample>, rate: f64, trim: Trim) -> Vec<StereoSample> {
    if trim == Trim::None || frames.is_empty() {
        return frames;
    }
    let block = ((rate * 0.01) as usize).max(1);
    let level: Vec<f32> = frames
        .chunks(block)
        .map(|c| (c.iter().map(|s| 0.5 * (s.left * s.left + s.right * s.right)).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let loudest = level.iter().cloned().fold(0.0, f32::max);
    if loudest <= 0.0 {
        return frames;
    }
    let floor = loudest * 10f32.powf(SILENCE_DB / 20.0);
    let keep = (KEEP_SECS * rate / block as f64).ceil() as usize;
    let mut on: Vec<bool> = level.iter().map(|&l| l >= floor).collect();
    // Grow every loud stretch by `keep` blocks.
    let loud = on.clone();
    for (i, &l) in loud.iter().enumerate() {
        if l {
            for o in on.iter_mut().take((i + keep + 1).min(loud.len())).skip(i.saturating_sub(keep)) {
                *o = true;
            }
        }
    }
    let first = on.iter().position(|&b| b).unwrap_or(0);
    let last = on.iter().rposition(|&b| b).unwrap_or(on.len() - 1);
    // Runs of blocks to keep.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = first;
    let mut first_only = false;
    if matches!(trim, Trim::Gaps | Trim::FirstHit) {
        // A silent stretch of at least `min_gap` blocks splits two runs.
        let min_gap = (MIN_GAP_SECS * rate / block as f64).ceil() as usize;
        let mut i = first;
        while i <= last {
            if on[i] {
                i += 1;
                continue;
            }
            let gap = on[i..=last].iter().take_while(|&&b| !b).count();
            if gap >= min_gap {
                if trim == Trim::FirstHit {
                    runs.push((start, i));
                    first_only = true;
                    break;
                }
                runs.push((start, i));
                start = i + gap;
            }
            i += gap;
        }
    }
    if !first_only {
        runs.push((start, last + 1));
    }
    let join = (JOIN_SECS * rate) as usize;
    // The file starts at its onset to the sample (a block is 10 ms), less a
    // millisecond.
    let onset = frames.iter().position(|s| s.left.abs().max(s.right.abs()) >= floor).unwrap_or(0);
    let lead = onset.saturating_sub((rate * 0.001) as usize);
    let mut out: Vec<StereoSample> = Vec::with_capacity(frames.len());
    for (k, (a, b)) in runs.into_iter().enumerate() {
        let from = if k == 0 { lead.min(b * block) } else { a * block };
        let seg = &frames[from..(b * block).min(frames.len())];
        // Crossfade into what is already there.
        let x = join.min(seg.len()).min(out.len());
        let base = out.len() - x;
        for (k, s) in seg.iter().enumerate() {
            if k < x {
                let t = (k as f32 + 0.5) / x as f32;
                let o = &mut out[base + k];
                o.left = o.left * (1.0 - t) + s.left * t;
                o.right = o.right * (1.0 - t) + s.right * t;
            } else {
                out.push(*s);
            }
        }
    }
    if trim == Trim::FirstHit {
        cap_one_shot(&mut out, rate);
    }
    out
}

/// Cut a one-shot to [`ONE_SHOT_MAX_SECS`] with a short fade.
fn cap_one_shot(frames: &mut Vec<StereoSample>, rate: f64) {
    let max = (ONE_SHOT_MAX_SECS * rate) as usize;
    if frames.len() <= max {
        return;
    }
    frames.truncate(max);
    let fade = ((ONE_SHOT_FADE_SECS * rate) as usize).min(max);
    for (k, s) in frames[max - fade..].iter_mut().enumerate() {
        let g = 1.0 - (k as f32 + 0.5) / fade as f32;
        s.left *= g;
        s.right *= g;
    }
}

/// Decode, normalize to [`SAMPLE_LOUDNESS_TARGET`] plus `offset_db` and
/// resample a file.
pub fn load_sample(path: &Path, root: u8, sample_rate: u32, offset_db: f64) -> Result<SampleData, AudioFileError> {
    load_trimmed(path, root, sample_rate, offset_db, Trim::None)
}

fn load_trimmed(path: &Path, root: u8, sample_rate: u32, offset_db: f64, trim: Trim) -> Result<SampleData, AudioFileError> {
    let data = SampleData::decode(path, root)?;
    let rate = data.sample_rate() as f64;
    let frames = trim_silence((0..data.len()).map(|i| data.frame(i)).collect(), rate, trim);
    // Hits shorter than one gating block are measured over their own length.
    let lufs = if frames.len() < (rate * 0.4).ceil() as usize { measure_hit_lufs(&frames, rate) } else { measure_lufs(&frames, rate) };
    let norm = if lufs.is_finite() { (SAMPLE_LOUDNESS_TARGET - lufs).clamp(-CUT_MAX_DB, BOOST_MAX_DB) } else { 0.0 };
    let gain = 10f64.powf((norm + offset_db) / 20.0) as f32;
    let left: Vec<f32> = frames.iter().map(|s| s.left * gain).collect();
    let right: Vec<f32> = frames.iter().map(|s| s.right * gain).collect();
    let cents = data.root_cents;
    let gains = if trim == Trim::Gaps { level_gains(&left, &right, rate) } else { Vec::new() };
    SampleData::from_channels(&left, &right, data.sample_rate(), root)
        .with_root_cents(cents)
        .with_level_gains(gains)
        .resampled_to(sample_rate)
}

// MUSICAL: first draft, tune by ear.
/// What a streamed note hears from its start position: this much of the sample.
const LEVEL_WINDOW_SECS: f64 = 0.4;
/// Largest correction of a part's level, in dB.
const LEVEL_MAX_DB: f32 = 18.0;

/// Per part of the sample, the gain that brings the next
/// [`LEVEL_WINDOW_SECS`] from there to the sample's overall level, so every
/// streamed note plays at the loudness the lane was calibrated for.
fn level_gains(left: &[f32], right: &[f32], rate: f64) -> Vec<f32> {
    let n = left.len();
    if n == 0 {
        return Vec::new();
    }
    let power = |a: usize, b: usize| (a..b).map(|i| 0.5 * (left[i] * left[i] + right[i] * right[i])).sum::<f32>() / (b - a).max(1) as f32;
    let overall = power(0, n).sqrt();
    let window = ((LEVEL_WINDOW_SECS * rate) as usize).clamp(1, n);
    let max = 10f32.powf(LEVEL_MAX_DB / 20.0);
    (0..LEVEL_PARTS)
        .map(|i| {
            let a = i * n / LEVEL_PARTS;
            // Past the end the note wraps to the start, as the stream does.
            let local = if a + window <= n {
                power(a, a + window)
            } else {
                (power(a, n) * (n - a) as f32 + power(0, a + window - n) * (a + window - n) as f32) / window as f32
            }
            .sqrt();
            if local <= 0.0 { 1.0 } else { (overall / local).clamp(1.0 / max, max) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `secs` of a tone (`on`) or silence, in order.
    fn pieces(parts: &[(bool, f64)]) -> Vec<StereoSample> {
        let rate = 48_000.0;
        parts
            .iter()
            .flat_map(|&(on, secs)| {
                (0..(secs * rate) as usize).map(move |i| {
                    let v = if on { 0.5 * (i as f32 * 0.05).sin() } else { 0.0 };
                    StereoSample { left: v, right: v }
                })
            })
            .collect()
    }

    #[test]
    fn mudpies_lose_their_silence_and_hits_start_at_the_onset() {
        let rate = 48_000.0;
        let f = pieces(&[(false, 0.3), (true, 1.0), (false, 2.0), (true, 1.0), (false, 0.01), (true, 0.5), (false, 1.0)]);
        let gaps = trim_silence(f.clone(), rate, Trim::Gaps);
        let secs = gaps.len() as f64 / rate;
        // The 2 s gap and the edges go, the 10 ms gap stays (plus kept tails).
        assert!((2.5..2.7).contains(&secs), "{secs}");
        let edges = trim_silence(f.clone(), rate, Trim::Edges);
        let lead = edges.iter().position(|s| s.left != 0.0).unwrap();
        assert!(lead <= 48, "starts within 1 ms of the onset: {lead}");
        assert!((edges.len() as f64 / rate - 4.51).abs() < 0.05, "{}", edges.len() as f64 / rate);
        assert_eq!(trim_silence(f.clone(), rate, Trim::None).len(), f.len());
    }

    #[test]
    fn a_one_shot_of_several_hits_plays_the_first() {
        let rate = 48_000.0;
        // A "kick" file that is a run of kicks.
        let run = pieces(&[(false, 0.05), (true, 0.2), (false, 0.3), (true, 0.2), (false, 0.3), (true, 0.2)]);
        let secs = trim_silence(run, rate, Trim::FirstHit).len() as f64 / rate;
        assert!((0.2..0.25).contains(&secs), "{secs}");
        // A long ringing one-shot stops at the cap, faded.
        let long = trim_silence(pieces(&[(true, 5.0)]), rate, Trim::FirstHit);
        assert_eq!(long.len() as f64 / rate, ONE_SHOT_MAX_SECS);
        assert_eq!(long.last().map(|s| s.left.abs() < 0.01), Some(true));
    }

    fn write_wav(path: &Path, secs: f32, freq: f32, amp: f32) {
        let spec = hound::WavSpec { channels: 1, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(44_100.0 * secs) as usize {
            let t = i as f32 / 44_100.0;
            let v = (std::f32::consts::TAU * freq * t).sin() * amp * (-t / 0.2).exp();
            w.write_sample((v * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn a_scanned_folder_becomes_a_kit_at_the_host_rate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_wav(&root.join("Kick 01.wav"), 0.4, 50.0, 0.9);
        write_wav(&root.join("Snare 01.wav"), 0.3, 200.0, 0.3);
        write_wav(&root.join("Closed hat 01.wav"), 0.1, 8000.0, 0.5);
        write_wav(&root.join("170_Break_01.wav"), 4.0 * 60.0 / 170.0, 120.0, 0.5);
        let mut lib = Library::new(vec![root.to_path_buf()]);
        assert_eq!(lib.scan(), 4);
        let picks = pick_drums(&lib, 3, &DrumPicks::new());
        assert!(picks.contains_key(&Slot::Kick) && picks.contains_key(&Slot::Snare) && picks.contains_key(&Slot::Break));
        let loaded = load_drums(&lib, &picks, 48_000);
        assert!(loaded.failed.is_empty(), "{:?}", loaded.failed);
        assert_eq!(loaded.break_bpm, Some(170.0));
        let (layout, _bank) = loaded.kit.build();
        assert!(layout.kick.is_some());
        // Loud and quiet files come out at the same loudness.
        let k = loaded.kit.kick.unwrap();
        let s = loaded.kit.snare.unwrap();
        let lufs = |d: &SampleData| measure_hit_lufs(&(0..d.len()).map(|i| d.frame(i)).collect::<Vec<_>>(), 48_000.0);
        assert!((lufs(&k) - lufs(&s)).abs() < 1.5, "{} {}", lufs(&k), lufs(&s));
        assert_eq!(k.sample_rate(), 48_000);
        // A pin replaces the seed's pick.
        let mut pins = DrumPicks::new();
        pins.insert(Slot::Kick, root.join("Snare 01.wav"));
        assert_eq!(pick_drums(&lib, 3, &pins)[&Slot::Kick], root.join("Snare 01.wav"));
    }
}
