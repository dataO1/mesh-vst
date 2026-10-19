//! State the audio thread shares with the editor: lock-free, written once
//! per block, read when the editor redraws.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use mesh_vst_core::generator::layout::DrumVoice;
use mesh_vst_core::pattern::RenderedBar;
use mesh_vst_core::role::Role;
use mesh_vst_core::{STEPS_PER_BAR, TICKS_PER_STEP};
use nice_plug_iced::iced::PollSubNotifier;
use serde::{Deserialize, Serialize};

/// Rows of the step view: the drum voices, or one row for the bass.
pub const ROWS: usize = 8;

/// The editor's view of the playing lane.
pub struct UiShared {
    /// Held note's key, 255 = none.
    pub root: AtomicU8,
    /// Host tempo, f32 bits.
    pub bpm: AtomicU32,
    /// Pattern bar playing and the 16th step in it (`u32::MAX` = stopped).
    pub bar: AtomicU32,
    pub step: AtomicU32,
    /// Form letter of the bar (0..=3 = A..D).
    pub letter: AtomicU8,
    /// Velocity per row and step of the playing bar (0 = no hit).
    pub hits: [[AtomicU8; STEPS_PER_BAR]; ROWS],
    /// Raised when a library edit or a pin needs the kit loaded again.
    pub kit_generation: AtomicU32,
    pub gated: AtomicBool,
    pub notifier: PollSubNotifier,
}

impl Default for UiShared {
    fn default() -> Self {
        Self {
            root: AtomicU8::new(255),
            bpm: AtomicU32::new(120f32.to_bits()),
            bar: AtomicU32::new(u32::MAX),
            step: AtomicU32::new(u32::MAX),
            letter: AtomicU8::new(0),
            hits: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU8::new(0))),
            kit_generation: AtomicU32::new(0),
            gated: AtomicBool::new(false),
            notifier: PollSubNotifier::new(),
        }
    }
}

impl UiShared {
    /// Publish a bar's hits (on a new bar, or when it re-rendered).
    pub fn publish_bar(&self, role: Role, bar: &RenderedBar) {
        let mut grid = [[0u8; STEPS_PER_BAR]; ROWS];
        for e in bar.events.as_slice() {
            if e.vel == 0 {
                continue;
            }
            let step = ((e.tick + TICKS_PER_STEP / 2) / TICKS_PER_STEP) as usize;
            if step >= STEPS_PER_BAR {
                continue;
            }
            let row = match role {
                Role::Drums => match DrumVoice::of_target(e.target) {
                    Some(v) if bar.voices & v.bit() != 0 => v as usize,
                    _ => continue,
                },
                _ => 0,
            };
            grid[row][step] = grid[row][step].max(e.vel);
        }
        for (r, row) in grid.iter().enumerate() {
            for (s, v) in row.iter().enumerate() {
                self.hits[r][s].store(*v, Ordering::Relaxed);
            }
        }
        self.letter.store(bar.letter, Ordering::Relaxed);
    }

    pub fn hits(&self) -> [[u8; STEPS_PER_BAR]; ROWS] {
        std::array::from_fn(|r| std::array::from_fn(|s| self.hits[r][s].load(Ordering::Relaxed)))
    }

    pub fn bump_kit(&self) {
        self.kit_generation.fetch_add(1, Ordering::AcqRel);
    }
}

/// Editor settings saved with the project: seed locks (D20).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    pub sound_locked: bool,
    pub pattern_locked: bool,
}
