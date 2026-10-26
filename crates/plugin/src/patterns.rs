//! The user's patterns (design D60): saved with the project per mode, the
//! editor's copy of the lane they play in, and the form as a MIDI file.
//!
//! The editor never touches the audio thread's lane: it builds its own from
//! the same parameters ([`preview_lane`]), which renders the same, for a
//! pattern's source and for the form it exports.

use std::path::PathBuf;
use std::sync::Arc;

use mesh_vst_core::generator::DrumLayout;
use mesh_vst_core::key::MusicalKey;
use mesh_vst_core::macros;
use mesh_vst_core::pattern::PatternLane;
use mesh_vst_core::slots::{PatternBook, Slot, UserPattern};
use serde::{Deserialize, Serialize};

use crate::params::{MeshParams, Mode};

/// Both modes' patterns (an instance can switch its mode).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternBooks {
    pub drums: PatternBook,
    pub bass: PatternBook,
}

impl PatternBooks {
    pub fn of(&self, mode: Mode) -> &PatternBook {
        match mode {
            Mode::Drums => &self.drums,
            Mode::Bass => &self.bass,
        }
    }

    pub fn of_mut(&mut self, mode: Mode) -> &mut PatternBook {
        match mode {
            Mode::Drums => &mut self.drums,
            Mode::Bass => &mut self.bass,
        }
    }
}

/// A lane for `mode` as the audio thread plays it: the same settings, the
/// drum kit's layout, the saved patterns. Not RT-safe.
pub fn preview_lane(mode: Mode, params: &MeshParams, pins: [Option<u32>; 4], layout: Option<DrumLayout>, clips: &Arc<mesh_vst_core::clips::ClipSet>) -> PatternLane {
    let settings = crate::lane_settings(params, pins, 0.0);
    let (mut lane, base) = match mode {
        Mode::Drums => (PatternLane::drums(layout.unwrap_or_else(DrumLayout::standard), 1, 1), macros::drum_params()),
        Mode::Bass => (PatternLane::bass_with_clips(1, 1, clips.clone()), macros::mudpie_params()),
    };
    macros::configure(&mut lane, base, &settings);
    let book = params.patterns.lock().map(|b| b.of(mode).clone()).unwrap_or_default();
    let _ = lane.set_patterns(Arc::new(book));
    lane
}

/// The key a lane plays in: `root` (a MIDI note) in the scale family.
pub fn key_of(params: &MeshParams, root: u8) -> MusicalKey {
    MusicalKey::new(root % 12, params.scale.value().family_mode().0)
}

/// Slot `slot`'s pattern as the editor shows it: the user's, or the seed's
/// source with its window.
pub fn source(lane: &PatternLane, slot: Slot, key: MusicalKey) -> UserPattern {
    lane.slot_source(slot, key)
}

/// Where `form`'s cycles start over its two passes, in beats (D61; none
/// for a written form).
pub fn cycle_starts(form: &mesh_vst_core::form::Form) -> Vec<u32> {
    let section = 16 * mesh_vst_core::form::BEATS_PER_BAR as u32;
    let sections = 2 * form.beats() as u32 / section;
    let mut out = Vec::new();
    for s in 0..sections {
        let cycle = form.cycle_at((s * section) as u64) as u32;
        if cycle > 0 {
            out.extend((0..section / cycle).map(|k| s * section + k * cycle));
        }
    }
    out
}

/// Where exported forms go: `~/Music/mesh vst/exports` (or the home folder's).
pub fn export_dir() -> Option<PathBuf> {
    dirs::audio_dir().or_else(dirs::home_dir).map(|d| d.join("mesh vst").join("exports"))
}

/// Write `lane`'s form as a MIDI file named for the instance, at `bpm`.
/// Returns its path.
pub fn export_form(lane: &PatternLane, name: &str, bpm: f64) -> std::io::Result<PathBuf> {
    let notes = mesh_vst_core::midi::form_notes(lane);
    let bytes = mesh_vst_core::midi::write_smf(&notes, bpm, name);
    let dir = export_dir().ok_or_else(|| std::io::Error::other("no music or home folder"))?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.mid", name.replace([' ', '/'], "-")));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn books_round_trip_through_the_saved_state() {
        let mut b = PatternBooks::default();
        b.bass.rerolls[3] = 7;
        b.bass.slots[0] = Some(UserPattern { notes: vec![], ticks: 384, start: 0, len: 384 });
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<PatternBooks>(&json).unwrap(), b);
        // An older project (no rerolls saved) still loads.
        let old = r#"{"drums":{"slots":[null,null,null,null,null,null,null,null]},"bass":{"slots":[null,null,null,null,null,null,null,null]}}"#;
        assert_eq!(serde_json::from_str::<PatternBooks>(old).unwrap(), PatternBooks::default());
    }
}
