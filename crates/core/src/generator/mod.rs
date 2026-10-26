//! Lane generators (design §6).
//!
//! A generator is a pure function of its [`BarCtx`]: the same context always
//! renders the same bar. Generators run on the bar-renderer thread, never on
//! the audio thread, and write only into the fixed-size [`BarEvents`].

use crate::event::BarEvents;
use crate::form::SeedClass;
use crate::role::Role;
use crate::harmony::HarmonyCtx;
use crate::STEPS_PER_BAR;

pub mod bass;
pub mod blocks;
pub mod drums;
pub mod fixed;
pub mod idiom;
pub mod layout;
pub mod recipe;
#[cfg(test)]
pub(crate) mod test_util;

pub use layout::DrumLayout;

/// Maximum number of generator parameters a lane can expose to controls.
pub const MAX_GEN_PARAMS: usize = 13;

/// Everything a generator may read to render one bar.
#[derive(Debug, Clone, Copy)]
pub struct BarCtx<'a> {
    /// The pattern seed: what holds across all letters (the rhythm family).
    pub style_seed: u32,
    /// Seed of the letter being rendered (see [`super::form`]).
    pub seed: u32,
    /// Seed that picks sounds (drum pools): the master seed in main bars, the
    /// letter seed in filler bars.
    pub sound_seed: u32,
    /// Main or filler material.
    pub class: SeedClass,
    /// Lane index, 0 = drums.
    pub lane: u8,
    pub role: Role,
    /// Global bar % 4.
    pub bar_in_phrase: u8,
    pub phrase: u32,
    /// Render the fill variant of this bar.
    pub fill: bool,
    /// Half-time feel (the clock is unchanged).
    pub half_time: bool,
    /// Normalized generator parameters, indexed like `param_names()`.
    pub params: &'a [f32; MAX_GEN_PARAMS],
    pub harmony: HarmonyCtx,
    /// The drum lane's onsets for this bar (velocity per step, 0 = none).
    /// All zero while the drum lane itself is rendered.
    pub drum_mask: &'a [u8; STEPS_PER_BAR],
    /// Per-note variation per parameter: each note reads its parameter up to
    /// this far from the value, seeded per note ([`blocks::note_param`]).
    pub jitter: [f32; MAX_GEN_PARAMS],
    /// The bass lane's onsets this bar, one bit per step (lanes after the
    /// bass follow it). 0 for the drum and bass lanes.
    pub bass_onsets: u16,
    /// The bass lane's motif onsets over the phrase, one step mask per bar
    /// (all 0 without a bass clip). Lead clips are picked against them.
    pub bass_phrase: [u16; 4],
    /// The drum lane fills this bar (a filler segment's last bar). Motif
    /// leads rest.
    pub drum_fill: bool,
    /// Splice: the focus per step (1 = as generated, see [`crate::splice`]).
    pub focus: [f32; STEPS_PER_BAR],
    /// How far the focus moves each parameter a note reads (per unit of focus).
    pub focus_shift: [f32; MAX_GEN_PARAMS],
    /// Splice: a pitched host's line under this bar (D46; off by default).
    pub host: crate::splice::HostLine,
    /// The letter being rendered (0 = A .. 3 = D).
    pub letter: u8,
    /// Bar of the letter's material: of its clip in clip mode (D50), which
    /// plays bar `material_bar % clip bars`.
    pub material_bar: u16,
    /// The pattern the user edited for this letter (D60): it is the
    /// skeleton, bar `material_bar` of its looping window.
    pub user: Option<&'a crate::slots::UserPattern>,
}

/// One lane's pattern generator.
pub trait LaneGenerator: Send {
    /// R1 (D57): render one bar's skeleton into `out` (cleared by the
    /// caller): the drums' whole bar, a pitched lane's rhythm on its base
    /// note. Must be deterministic in `ctx` and leave events sorted by tick.
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents);

    /// R5 (D57): Density's fill of the rests, on `finest` (the grid the
    /// speed allows). The drums fill in their skeleton.
    fn fill(&self, _ctx: &BarCtx, _out: &mut BarEvents, _finest: u16) {}

    /// R6 (D57): every note's length (`finest` the grid the speed allows).
    fn lengths(&self, _ctx: &BarCtx, _out: &mut BarEvents, _finest: u16) {}

    /// R8 (D57): the pitches of the finished rhythm.
    fn melody(&self, _ctx: &BarCtx, _out: &mut BarEvents) {}

    /// R11 (D57): velocities, slides, the filter, the attack, the start
    /// position, loops.
    fn expression(&self, _ctx: &BarCtx, _out: &mut BarEvents) {}

    /// Parameter names; index = position in `BarCtx::params`.
    fn param_names(&self) -> &'static [&'static str];

    /// Drum lane: voices on by default in this bar (bit per
    /// [`DrumVoice`](layout::DrumVoice)); the pads flip them. Other lanes: all.
    fn voice_mask(&self, _ctx: &BarCtx) -> u8 {
        0xFF
    }

    /// Clip mode (D50): the length in beats of the clip the letter of `ctx`
    /// plays (its material, longer than a bar). `None` when it generates.
    /// The key a pitched lane's rows count from (D60: the user's pattern
    /// and the editor's source; 0 for the drums).
    fn home_key(&self, _ctx: &BarCtx) -> i32 {
        0
    }

    fn material_beats(&self, _ctx: &BarCtx) -> Option<u32> {
        None
    }

    /// Default normalized value for each parameter in `param_names()`.
    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        [0.5; MAX_GEN_PARAMS]
    }
}
