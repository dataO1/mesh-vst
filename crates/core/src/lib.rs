//! mesh vst core: seeded pattern generators and the sampler voice.
//!
//! No plugin-API or GUI code lives here (design D5). The generators, forms
//! and sampler started as a copy of Mesh live mode
//! (`mesh-core/src/live`), relicensed under GPL-3.0-or-later by their author.

pub mod calibrate;
pub mod clips;
pub mod contrast;
pub mod error;
pub mod event;
pub mod arrange;
pub mod form;
pub mod ops;
pub mod gc;
pub mod generator;
pub mod harmony;
pub mod heat;
pub mod key;
pub mod kit;
pub mod library;
pub mod loader;
pub mod loudness;
pub mod macros;
pub mod modout;
pub mod motif;
pub mod pattern;
pub mod player;
pub mod rng;
pub mod role;
pub mod sampler;
pub mod spec;
pub mod splice;
pub mod squash;
pub mod tail;
pub mod tempo;
pub mod types;

/// Steps (16ths) per bar.
pub const STEPS_PER_BAR: usize = 16;
/// Event time resolution: ticks per bar (96 per beat).
pub const TICKS_PER_BAR: u16 = 384;
/// Ticks per 16th step.
pub const TICKS_PER_STEP: u16 = TICKS_PER_BAR / STEPS_PER_BAR as u16;
/// Fixed event capacity per bar (covers 32nd rolls across sub-voices).
pub const MAX_EVENTS_PER_BAR: usize = 256;

/// RT-safety test support: count allocator calls inside a closure.
#[cfg(all(test, debug_assertions))]
pub(crate) mod test_support {
    use std::sync::Mutex;

    #[global_allocator]
    static ALLOC_CHECK: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    static SERIAL: Mutex<()> = Mutex::new(());

    /// Run `f` with allocation checking on and return how many allocator
    /// calls it made. Serialized, because the violation counter is global.
    pub(crate) fn count_allocs<R>(f: impl FnOnce() -> R) -> (R, u32) {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        assert_no_alloc::reset_violation_count();
        let r = assert_no_alloc::assert_no_alloc(f);
        let n = assert_no_alloc::violation_count();
        assert_no_alloc::reset_violation_count();
        (r, n)
    }
}
