//! Bass clips (D50): MIDI clips per letter that the bass plays from instead
//! of generating its line, when the Bass source is Clips.
//!
//! A clip is taken as written, any length: its length is the MIDI file's
//! end of track (or its last note's end, if later), rounded up to the next
//! beat. At load time it becomes one voice (the lowest note of a chord), its
//! root is detected ([`crate::motif::key`]) and each note is kept as
//! semitones over that root. When it plays, the root lands on the key's root
//! and every note outside the key's scale moves to the nearest note in it,
//! so a clip plays in any key and follows a key change.
//!
//! Ten clips per letter ship with the plugin ([`tables`], written to
//! `assets/clips` by the `write_clips` example and embedded). Users add their
//! own under a folder with one subfolder per letter (`a`, `b`, `c`, `d`).
//!
//! Everything here runs at load time, never on the audio thread.

pub mod tables;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::harmony::{MAJOR_SCALE, MINOR_SCALE};
use crate::motif::{key, midi, RawNote};
use crate::{rng, TICKS_PER_BAR};

/// Engine ticks per beat.
pub const BEAT_TICKS: u32 = TICKS_PER_BAR as u32 / 4;
/// Longest clip (beats); longer ones are cut there.
pub const MAX_CLIP_BEATS: u32 = 64;
/// Most notes of a clip (the rest are ignored).
pub const MAX_CLIP_NOTES: usize = 256;
/// Most clips a letter holds (the shipped ones and a user's).
pub const MAX_CLIPS_PER_LETTER: usize = 64;
/// Onsets this close (ticks) count as one chord: the lowest note survives.
const CHORD_TICKS: u32 = 6;
/// The clip moves by octaves so its lowest note sits this many semitones
/// from its root at most below (a 4th, so a 5th below the root stays put
/// as the note under it).
const FLOOR_SEMIS: i32 = -5;
const S_PICK: u32 = 0xD50;

/// One note of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipNote {
    /// Ticks from the clip start.
    pub tick: u32,
    /// Written length (cut at the next note).
    pub len: u32,
    pub vel: u8,
    /// Semitones over the clip's root (the lowest note in `FLOOR_SEMIS ..`).
    pub semis: i16,
    /// Scale step over the root in the detected scale (rounded down), to
    /// break a tie when a note snaps into another scale.
    pub degree: i16,
}

/// A bass clip as the generator plays it.
#[derive(Debug, Clone, PartialEq)]
pub struct BassClip {
    pub name: String,
    /// Length in beats (the file's, rounded up to a beat).
    pub beats: u16,
    /// Monophonic, sorted by tick.
    pub notes: Vec<ClipNote>,
}

impl BassClip {
    /// A clip from a file's notes and its end (ticks). `None` without notes.
    pub fn from_notes(name: &str, mut notes: Vec<RawNote>, end: u32) -> Option<Self> {
        notes.sort_by_key(|n| (n.start, n.pitch));
        let last = notes.iter().map(|n| n.start + n.len).max()?;
        let beats = end.max(last).div_ceil(BEAT_TICKS).clamp(1, MAX_CLIP_BEATS);
        let limit = beats * BEAT_TICKS;
        notes.retain(|n| n.start < limit);
        // One voice: chords keep their lowest note.
        let mut mono: Vec<RawNote> = Vec::with_capacity(notes.len());
        for n in notes {
            match mono.last_mut() {
                Some(l) if n.start - l.start <= CHORD_TICKS => {
                    if n.pitch < l.pitch {
                        *l = n;
                    }
                }
                _ => mono.push(n),
            }
        }
        mono.truncate(MAX_CLIP_NOTES);
        for i in 0..mono.len() {
            let next = mono.get(i + 1).map_or(limit, |n| n.start);
            mono[i].len = mono[i].len.min(next - mono[i].start).max(1);
        }
        let est = key::detect(&mono)?;
        let lowest = mono.iter().map(|n| n.pitch as i32 - est.tonic as i32).min()?;
        let octaves = (lowest - FLOOR_SEMIS).div_euclid(12);
        let notes = mono
            .iter()
            .map(|n| {
                let (degree, _) = key::to_degree(n.pitch, est.tonic, est.minor);
                ClipNote {
                    tick: n.start,
                    len: n.len,
                    vel: n.vel.clamp(1, 127),
                    semis: (n.pitch as i32 - est.tonic as i32 - 12 * octaves) as i16,
                    degree: (degree - 7 * octaves) as i16,
                }
            })
            .collect();
        Some(Self { name: name.to_string(), beats: beats as u16, notes })
    }

    /// Parse a MIDI file's bytes.
    pub fn from_midi(name: &str, bytes: &[u8]) -> Result<Self, String> {
        let (notes, end) = midi::parse(bytes).map_err(|e| e.to_string())?;
        Self::from_notes(name, notes, end).ok_or_else(|| "no notes".to_string())
    }

    /// Length in ticks.
    pub fn ticks(&self) -> u32 {
        self.beats as u32 * BEAT_TICKS
    }

    /// The lowest note (semitones over the root).
    pub fn lowest(&self) -> i32 {
        self.notes.iter().map(|n| n.semis as i32).min().unwrap_or(0)
    }
}

/// `semis` over the key's root moved into `scale` (the nearest scale note;
/// between two, the one at the note's own scale step, else the lower).
pub fn snap(semis: i32, degree: i32, scale: &[u8; 7]) -> i32 {
    let pc = semis.rem_euclid(12) as u8;
    if scale.contains(&pc) {
        return semis;
    }
    let (down, up) = (scale.contains(&((pc + 11) % 12)), scale.contains(&((pc + 1) % 12)));
    match (down, up) {
        (true, true) => {
            let own = scale[degree.rem_euclid(7) as usize];
            if own == (pc + 1) % 12 { semis + 1 } else { semis - 1 }
        }
        (false, true) => semis + 1,
        (true, false) => semis - 1,
        // No neighbour in the scale (cannot happen with 7 notes): as written.
        _ => semis,
    }
}

/// The scale of a detected key.
pub fn detected_scale(minor: bool) -> &'static [u8; 7] {
    if minor { &MINOR_SCALE } else { &MAJOR_SCALE }
}

/// The clips of each letter (A..D).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipSet {
    pub letters: [Vec<BassClip>; 4],
}

macro_rules! builtin_files {
    () => {
        [
            builtin_files!(@letter "a"),
            builtin_files!(@letter "b"),
            builtin_files!(@letter "c"),
            builtin_files!(@letter "d"),
        ]
    };
    (@letter $l:literal) => {
        [
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/01.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/02.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/03.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/04.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/05.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/06.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/07.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/08.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/09.mid")),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clips/", $l, "/10.mid")),
        ]
    };
}

/// The shipped clips' files, per letter.
const BUILTIN: [[&[u8]; tables::PER_LETTER]; 4] = builtin_files!();


/// Letter folder names.
pub const LETTER_DIRS: [&str; 4] = ["a", "b", "c", "d"];

impl ClipSet {
    /// The shipped clips (parsed once).
    pub fn builtin() -> Arc<ClipSet> {
        static SET: OnceLock<Arc<ClipSet>> = OnceLock::new();
        SET.get_or_init(|| {
            let mut set = ClipSet::default();
            for (l, files) in BUILTIN.iter().enumerate() {
                for (i, bytes) in files.iter().enumerate() {
                    let name = format!("{}{} {}", LETTER_DIRS[l].to_uppercase(), i + 1, tables::of(l)[i].name);
                    set.letters[l].push(BassClip::from_midi(&name, bytes).expect("shipped clips parse"));
                }
            }
            Arc::new(set)
        })
        .clone()
    }

    /// The shipped clips and a user's from `dir/a` .. `dir/d` (`*.mid`,
    /// `*.midi`); returns the set and a warning per file skipped.
    pub fn with_dir(dir: &Path) -> (ClipSet, Vec<String>) {
        let mut set = (*Self::builtin()).clone();
        let mut warnings = Vec::new();
        for (l, sub) in LETTER_DIRS.iter().enumerate() {
            let folder = [sub.to_string(), sub.to_uppercase()].into_iter().map(|s| dir.join(s)).find(|p| p.is_dir());
            let Some(folder) = folder else { continue };
            for path in crate::motif::scan_dir(&folder) {
                if set.letters[l].len() >= MAX_CLIPS_PER_LETTER {
                    warnings.push(format!("clip {}: more than {MAX_CLIPS_PER_LETTER} clips, ignored", path.display()));
                    continue;
                }
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
                match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| BassClip::from_midi(name, &b)) {
                    Ok(c) => set.letters[l].push(c),
                    Err(e) => warnings.push(format!("clip {}: {e}", path.display())),
                }
            }
        }
        (set, warnings)
    }

    /// The clip letter `letter` (0 = A) plays for its seed.
    pub fn pick(&self, letter: u8, seed: u32, lane: u8) -> Option<&BassClip> {
        let clips = self.letters.get(letter as usize)?;
        (!clips.is_empty()).then(|| &clips[rng::below(rng::hash(seed, lane, letter, 0, S_PICK), clips.len() as u32) as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motif::starter::StarterClip;

    fn raw(start: u32, len: u32, pitch: u8) -> RawNote {
        RawNote { start, len, pitch, vel: 100 }
    }

    #[test]
    fn the_shipped_files_are_the_tables_and_play_32_beats() {
        let set = ClipSet::builtin();
        for l in 0..4 {
            assert_eq!(set.letters[l].len(), tables::PER_LETTER);
            for (i, c) in tables::of(l).iter().enumerate() {
                let written = midi::write(&c.clip(crate::motif::Voice::Lowest), c.name);
                assert_eq!(BUILTIN[l][i], &written[..], "{}: run `cargo run --example write_clips`", c.name);
                let clip = &set.letters[l][i];
                assert_eq!(clip.beats, 32, "{}", clip.name);
                assert!(clip.notes.iter().any(|n| n.semis.rem_euclid(12) == 0), "{} plays its root", clip.name);
            }
        }
    }

    #[test]
    fn shipped_clips_have_their_root_on_c_and_differ() {
        let mut shapes = std::collections::BTreeSet::new();
        let mut wrong = Vec::new();
        for l in 0..4 {
            for c in tables::of(l) {
                let clip: &StarterClip = c;
                let notes = clip.clip(crate::motif::Voice::Lowest).notes;
                let est = key::detect(&notes).unwrap();
                // The root is what matters (the mode only breaks snapping ties).
                if est.tonic != 0 {
                    wrong.push((c.name, est.tonic));
                }
                assert_eq!(clip.bars.len(), 8, "{}", c.name);
                shapes.insert(clip.bars.iter().map(|b| b.0).collect::<Vec<_>>());
            }
        }
        assert!(wrong.is_empty(), "the root is C: {wrong:?}");
        assert_eq!(shapes.len(), 4 * tables::PER_LETTER, "every clip has its own rhythm");
    }

    #[test]
    fn a_clip_keeps_its_length_and_silence() {
        let beat = BEAT_TICKS;
        // A note on beat 2, the track ending on beat 7 and a half.
        let c = BassClip::from_notes("t", vec![raw(beat, 40, 36), raw(3 * beat, 40, 43)], 6 * beat + beat / 2).unwrap();
        assert_eq!(c.beats, 7, "the file's end, up to a beat");
        assert_eq!(c.notes[0].tick, beat, "a leading rest stays");
        // Without a later end, the last note's end.
        let c = BassClip::from_notes("t", vec![raw(0, 40, 36), raw(2 * beat, 3 * beat, 43)], 0).unwrap();
        assert_eq!(c.beats, 5);
        assert!(BassClip::from_notes("t", Vec::new(), 100).is_none());
    }

    #[test]
    fn chords_keep_the_lowest_note_and_the_root_is_detected() {
        // E minor: E2, G2 over E1 (a chord), B2, E2.
        let notes = vec![raw(0, 96, 40), raw(2, 96, 28), raw(96, 96, 43), raw(192, 96, 47), raw(288, 96, 40)];
        let c = BassClip::from_notes("t", notes, 0).unwrap();
        assert_eq!(c.notes.len(), 4);
        assert_eq!(c.notes[0].semis, 0, "the chord's E1 is the root");
        assert_eq!(c.notes.iter().map(|n| n.semis).collect::<Vec<_>>(), vec![0, 15, 19, 12]);
        assert!(c.lowest() >= FLOOR_SEMIS);
    }

    #[test]
    fn notes_snap_into_the_scale() {
        // In C minor: E (4) goes to Eb (3) or F (5): its step (the 3rd) says Eb.
        assert_eq!(snap(4, 2, &MINOR_SCALE), 3);
        // B natural (11) has only Bb (10) and C (12) around it: Bb is in, C too; the 7th says Bb.
        assert_eq!(snap(11, 6, &MINOR_SCALE), 10);
        // In key notes stay.
        for s in MINOR_SCALE {
            assert_eq!(snap(s as i32 + 12, 0, &MINOR_SCALE), s as i32 + 12);
        }
        // Every pitch lands in the scale, in every mode.
        for (scale, ..) in crate::harmony::MINOR_MODES.iter().chain(crate::harmony::MAJOR_MODES.iter()) {
            for s in -24..24 {
                for d in 0..7 {
                    assert!(scale.contains(&(snap(s, d, scale).rem_euclid(12) as u8)), "{s} in {scale:?}");
                }
            }
        }
    }

    #[test]
    fn a_users_folder_adds_clips_per_letter() {
        let dir = std::env::temp_dir().join(format!("mesh-vst-clips-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let clip = tables::A[0].clip(crate::motif::Voice::Lowest);
        std::fs::write(dir.join("b").join("mine.mid"), midi::write(&clip, "mine")).unwrap();
        std::fs::write(dir.join("b").join("broken.mid"), b"nope").unwrap();
        let (set, warnings) = ClipSet::with_dir(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(set.letters[1].len(), tables::PER_LETTER + 1);
        assert_eq!(set.letters[0].len(), tables::PER_LETTER);
        assert_eq!(set.letters[1].last().unwrap().name, "mine");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }
}
