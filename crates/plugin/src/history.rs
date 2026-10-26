//! Undo and redo (design D26, D60): every change the editor makes, back and
//! forth. A snapshot holds the parameters (by id), the pinned files, the
//! editor settings that change what plays (phrase pins, the Splice host,
//! the mod-out map) and the user's patterns. Not the view settings (locks,
//! the low-power view).

use std::collections::BTreeMap;

use mesh_vst_core::loader::Slot;

use crate::patterns::PatternBooks;
use crate::worker::FileRef;

/// Steps kept each way.
const DEPTH: usize = 200;

/// The pinned and rolled files.
pub type Pins = (BTreeMap<Slot, FileRef>, Option<FileRef>, BTreeMap<Slot, FileRef>);

/// What the editor settings hold of what plays.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Played {
    pub phrase_pins: [Option<u32>; 4],
    pub host: Option<u64>,
    pub mod_bass: Vec<mesh_vst_core::modout::Source>,
    pub mod_drums: Vec<mesh_vst_core::modout::Source>,
}

impl Played {
    pub fn of(u: &crate::ui::UiState) -> Self {
        Self { phrase_pins: u.phrase_pins, host: u.host, mod_bass: u.mod_bass.clone(), mod_drums: u.mod_drums.clone() }
    }

    pub fn apply(&self, u: &mut crate::ui::UiState) {
        u.phrase_pins = self.phrase_pins;
        u.host = self.host;
        u.mod_bass = self.mod_bass.clone();
        u.mod_drums = self.mod_drums.clone();
    }
}

/// One step.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Normalized values by parameter id.
    pub values: Vec<(String, f32)>,
    pub pins: Pins,
    pub played: Played,
    pub patterns: PatternBooks,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    /// Remember `before`, the state before a change: it ends the redo line.
    /// The same state twice in a row is kept once.
    pub fn record(&mut self, before: Snapshot) {
        if self.undo.last() != Some(&before) {
            self.undo.push(before);
            if self.undo.len() > DEPTH {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    /// The state to go back to from `now`, if any.
    pub fn undo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let s = self.undo.pop()?;
        self.redo.push(now);
        Some(s)
    }

    pub fn redo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let s = self.redo.pop()?;
        self.undo.push(now);
        Some(s)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(v: f32) -> Snapshot {
        Snapshot { values: vec![("density".into(), v)], ..Default::default() }
    }

    #[test]
    fn back_and_forth() {
        let mut h = History::default();
        h.record(snap(0.1));
        h.record(snap(0.2));
        assert_eq!(h.undo(snap(0.3)), Some(snap(0.2)));
        assert_eq!(h.undo(snap(0.2)), Some(snap(0.1)));
        assert_eq!(h.undo(snap(0.1)), None);
        assert_eq!(h.redo(snap(0.1)), Some(snap(0.2)));
        assert_eq!(h.redo(snap(0.2)), Some(snap(0.3)));
        assert!(!h.can_redo());
        // A new change ends the redo line.
        h.undo(snap(0.3));
        h.record(snap(0.2));
        assert!(!h.can_redo());
    }

    #[test]
    fn the_same_state_is_kept_once() {
        let mut h = History::default();
        h.record(snap(0.1));
        h.record(snap(0.1));
        assert_eq!(h.undo(snap(0.2)), Some(snap(0.1)));
        assert!(!h.can_undo());
    }
}
