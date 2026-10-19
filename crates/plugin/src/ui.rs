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
    /// The plugin's mono output for the visual, while the editor is open.
    pub audio: std::sync::Mutex<Option<rtrb::Consumer<f32>>>,
    pub sample_rate: AtomicU32,
    /// A library file the editor wants to hear (D35); the loader takes it.
    pub audition: std::sync::Mutex<Option<std::path::PathBuf>>,
    /// Mod-out source per channel (index into `Source::ALL`), bass then drums.
    pub mod_map: [[AtomicU8; mesh_vst_core::modout::CHANNELS]; 2],
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
            audio: std::sync::Mutex::new(None),
            sample_rate: AtomicU32::new(48_000),
            audition: std::sync::Mutex::new(None),
            mod_map: [
                std::array::from_fn(|c| AtomicU8::new(source_index(mesh_vst_core::modout::DEFAULT_BASS[c]))),
                std::array::from_fn(|c| AtomicU8::new(source_index(mesh_vst_core::modout::DEFAULT_DRUMS[c]))),
            ],
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

/// Index of a source in `Source::ALL`.
pub fn source_index(s: mesh_vst_core::modout::Source) -> u8 {
    mesh_vst_core::modout::Source::ALL.iter().position(|x| *x == s).unwrap_or(0) as u8
}

impl UiShared {
    /// Copy a saved mapping into the atomics the audio thread reads.
    pub fn load_mod_map(&self, state: &UiState) {
        use mesh_vst_core::modout::{DEFAULT_BASS, DEFAULT_DRUMS};
        for (m, (saved, default)) in [(&state.mod_bass, DEFAULT_BASS), (&state.mod_drums, DEFAULT_DRUMS)].into_iter().enumerate() {
            for c in 0..mesh_vst_core::modout::CHANNELS {
                let s = saved.get(c).copied().unwrap_or(default[c]);
                self.mod_map[m][c].store(source_index(s), Ordering::Relaxed);
            }
        }
    }
}

/// Editor settings saved with the project: seed locks (D20).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    pub sound_locked: bool,
    pub pattern_locked: bool,
    /// The step view instead of the shader field (low power, D35).
    pub low_power: bool,
    /// Mod-out sources per channel (D34); empty = the defaults.
    pub mod_bass: Vec<mesh_vst_core::modout::Source>,
    pub mod_drums: Vec<mesh_vst_core::modout::Source>,
}
