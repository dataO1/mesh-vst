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
use crate::sampler::SampleData;
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
        let sample = match load_sample(path, HIT_ROOT, sample_rate) {
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
    load_sample(path, root, sample_rate).map(share)
}

/// Decode, normalize to [`SAMPLE_LOUDNESS_TARGET`] and resample a file.
pub fn load_sample(path: &Path, root: u8, sample_rate: u32) -> Result<SampleData, AudioFileError> {
    let data = SampleData::decode(path, root)?;
    let frames: Vec<StereoSample> = (0..data.len()).map(|i| data.frame(i)).collect();
    let rate = data.sample_rate() as f64;
    // Hits shorter than one gating block are measured over their own length.
    let lufs = if frames.len() < (rate * 0.4).ceil() as usize { measure_hit_lufs(&frames, rate) } else { measure_lufs(&frames, rate) };
    let gain = if lufs.is_finite() { 10f64.powf((SAMPLE_LOUDNESS_TARGET - lufs).clamp(-CUT_MAX_DB, BOOST_MAX_DB) / 20.0) as f32 } else { 1.0 };
    let left: Vec<f32> = frames.iter().map(|s| s.left * gain).collect();
    let right: Vec<f32> = frames.iter().map(|s| s.right * gain).collect();
    let cents = data.root_cents;
    SampleData::from_channels(&left, &right, data.sample_rate(), root).with_root_cents(cents).resampled_to(sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

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
