//! Bass generator (design §6.3 rule A6; research §7.1.2, §7.1.7).
//!
//! - **Rhythm** comes from the gaps in the drums: metric importance, raised
//!   on steps the drum backbone leaves empty. Kick steps are either avoided,
//!   left alone, or doubled (`kick_mode`). `density` reveals steps in order of
//!   importance and `complexity` shifts the importance towards syncopation.
//! - **Pitch** is a root-heavy pool of chord-relative scale degrees (root,
//!   octave, 5th, 7th, 3rd — b7 and b3 in a minor key) resolved against the
//!   bar's chord, so the line follows the progression and any key change.
//! - **Articulation**: accents, slides (`TIE` + glide), and note length from
//!   the density: sparse lines hold long notes, dense lines play short ones.
//! - **Phrase shape**: bars 0-2 repeat, bar 3 re-draws some steps and ends
//!   on a turnaround note.
//!
//! The idiom family uses the same generator with the rhythm importance read
//! from a style rhythm cell instead of the metric tables.
//!
//! Performance parameters, in order: `density`, `complexity`, `variation`,
//! `kick_mode` (< 1/3 avoid the kick, < 2/3 ignore it, else double it),
//! `register` (octave down / base / up), `slide`, `accent`, `spread`
//! (size of the "odd block" jumps: most notes stay in one octave, and a
//! seeded run of 1-3 notes per bar may jump up; 0 = no jumps, then a 5th, an
//! octave, an octave and a 3rd, up to an octave and a 5th (+19 semitones)).
//! `length` and `expression` (0.5 = unchanged) act as on the `phrase`
//! generator, so a sound pack's shape control works on generated lines:
//! length shortens notes towards stabs or holds and ties them, expression
//! scales each note's filter offset. `stabs` (0.5 = as the density says)
//! sets the share of stab notes (flagged `STAB`: no sustain, the stab
//! accent) against held ones.
//!
//! **Motif mode**: with motif clips loaded ([`crate::motif`]) and the
//! construction option `motifs` not 0, the seed picks a clip and the line
//! plays it as written (see [`super::motif`]): `density` thins or repeats
//! notes and sets their length, `spread` lifts weak notes an octave,
//! `variation` changes the phrase's last bar, `register` shifts octaves.
//! The pitch and rhythm never change per bar. `complexity` and `kick_mode`
//! do not act. Accent, slide and the note filter still vary per note.
//!
//! Construction options: `rule_a3` (phrase shape), `rule_a5` (velocity
//! tiers), `rule_a6` (gap-following and root weighting). All default to 1.
//! `motifs` = 0 turns motif mode off.

use std::collections::BTreeMap;

use super::blocks::{
    density_attack_in, lerp, metric_weight, note_param, note_param_for, NoteShape, step_motif, level_unit, note, option, param, reveals, step_tick, stream_start, velocity, LevelMap,
    PhraseRng, Tier, BASS_ATTACK_BY_DENSITY,
};
use super::blocks::{VARIATION_BAR, VEL_ACCENT};
use super::layout::MASK_KICK_MIN;
use super::motif::{self, Plan, Play, Shape};
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::event::{flags, BarEvents, EventKind, LiveEvent};
use crate::harmony::SCALE_LEN;
use crate::clips::{self, BassClip, ClipSet};
use crate::motif::Motif;
use std::sync::Arc;
use crate::rng;
use crate::sampler::CUTOFF_OFFSET_OCTAVES;
use crate::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};
use crate::key::MusicalKey;

pub const PARAM_NAMES: [&str; 13] =
    ["density", "complexity", "variation", "kick_mode", "register", "slide", "accent", "spread", "length", "expression", "stabs", "syncopation", "source"];
const P_DENSITY: usize = 0;
const P_COMPLEXITY: usize = 1;
const P_VARIATION: usize = 2;
const P_KICK_MODE: usize = 3;
const P_REGISTER: usize = 4;
const P_SLIDE: usize = 5;
const P_ACCENT: usize = 6;
const P_SPREAD: usize = 7;
const P_LENGTH: usize = 8;
const P_EXPRESSION: usize = 9;
const P_STABS: usize = 10;
const P_SYNCOPATION: usize = 11;
/// The Bass source (D50): below 0.5 the generated line, from 0.5 the clips.
const P_SOURCE: usize = 12;

// MUSICAL: first draft, tune by ear.
pub const PARAM_DEFAULTS: [f32; MAX_GEN_PARAMS] = [0.5, 0.4, 0.5, 0.0, 0.5, 0.5, 0.5, 0.3, 0.5, 0.5, 0.5, 0.0, 0.0];

const S_CELL: u32 = 0x200;
const S_RHYTHM: u32 = 0x201;
const S_PITCH: u32 = 0x202;
const S_ACCENT: u32 = 0x203;
const S_SLIDE: u32 = 0x204;
const S_VEL: u32 = 0x205;
const S_TURN: u32 = 0x206;
const S_LENGTH: u32 = 0x20B;
const S_PERSONA: u32 = 0x210;
const S_HALF: u32 = 0x211;
const S_RATCHET: u32 = 0x212;
const S_LEGATO: u32 = 0x213;
const S_FIGURE: u32 = 0x214;
const S_GROOVE: u32 = 0x215;
const S_REGION: u32 = 0x216;
const S_FILTER: u32 = 0x20D;
const S_JUMP: u32 = 0x207;
const S_JUMP_START: u32 = 0x208;
const S_JUMP_LEN: u32 = 0x209;
const S_JUMP_SIZE: u32 = 0x20A;

// MUSICAL: first draft, tune by ear. Rhythm importance per step at
// complexity 0 (on the beat) and at complexity 1 (syncopated).
const BASS_SIMPLE: [f32; 16] =
    [1.0, 0.1, 0.3, 0.15, 0.6, 0.1, 0.45, 0.2, 0.8, 0.1, 0.35, 0.3, 0.55, 0.1, 0.45, 0.25];
const BASS_SYNC: [f32; 16] =
    [0.9, 0.2, 0.4, 0.7, 0.3, 0.2, 0.75, 0.55, 0.4, 0.3, 0.6, 0.7, 0.3, 0.3, 0.75, 0.5];
/// Importance added on steps the drum backbone leaves empty (A6).
const GAP_BONUS: f32 = 0.1;
/// Importance taken off steps with a snare.
const ONSET_PENALTY: f32 = 0.15;
/// Rhythm randomness at complexity 1.
const BASS_CHAOS_MAX: f32 = 0.35;

// MUSICAL: first draft, tune by ear. Pitch pool as (chord-relative scale
// degree, weight): root, octave, 5th, 7th, 3rd (research §7.1.2: root-heavy).
// Root, octave, 5th, 7th, 3rd above, and the 5th and 7th below the root.
const BASS_POOL: [(i32, f32); 7] = [(0, 6.0), (7, 1.5), (4, 2.0), (6, 1.0), (2, 1.0), (-3, 2.0), (-1, 1.5)];
/// Weight factor of the non-root degrees at spread 0 and 1 (spread moves the
/// line off the root more often).
const POOL_FACTOR: (f32, f32) = (0.2, 1.3);
/// Notes below the root may go this many semitones under it, and never more
/// than `BELOW_KEY_FLOOR` under the key's root in the register.
const BELOW_ROOT_SEMITONES: i32 = 8;
const BELOW_KEY_FLOOR: i32 = 4;
/// Rhythm density at density 0 and 1, eased in with density to this power
/// (user, 2026-10-17: at normal speed the middle plays about 4 notes a bar,
/// 2.7 at the bottom, 13 at the top).
const RHYTHM_DENSITY: (f32, f32) = (0.3, 1.0);
const RHYTHM_CURVE: f32 = 3.0;
/// Importance taken off the off-16th steps at density 0, so sparse lines sit
/// on the 8ths and hold their notes longer. Fades out by `THIN_DENSITY_MAX`.
const THIN_PENALTY: f32 = 0.35;
const THIN_DENSITY_MAX: f32 = 0.6;
/// Share of short, stabby notes at density 0 and 1; the rest are held.
const SHORT_SHARE: (f32, f32) = (0.2, 0.8);
/// `stabs` 0..1 moves the stab share by this much around the density's
/// (± half of it), within the range.
const STABS_SHIFT: f32 = 1.2;
const STABS_RANGE: (f32, f32) = (0.04, 0.96);
/// Short note: share of the gap at density 0 and 1, and longest length in steps.
const SHORT_LENGTH: (f32, f32, u16) = (0.6, 0.35, 2);
// MUSICAL: first draft, tune by ear. Odd-block jumps (spread = jump size).
/// Jump sizes in scale degrees: 5th, octave, octave + 3rd, octave + 5th.
const JUMPS: [i32; 4] = [4, 7, 9, 11];
/// Spread below this: no odd blocks (the walk keeps the line close).
const JUMP_SPREAD_MIN: f32 = 0.4;
/// Chance that a bar has an odd block.
const JUMP_BLOCK_PROB: (f32, f32) = (0.15, 0.7);
/// Longest odd block in notes.
const JUMP_BLOCK_MAX: u32 = 3;
/// Share of blocks that take the largest jump the spread allows.
const JUMP_LARGEST_SHARE: f32 = 0.6;
/// Degrees a phrase-end turnaround note may take (5th, 7th).
const TURNAROUND_DEGREES: [i32; 2] = [4, 6];
/// Steps tried for the turnaround note, last first.
const TURNAROUND_STEPS: [usize; 3] = [15, 14, 13];

// MUSICAL: first draft, tune by ear (user, 2026-10-17: each seed its own
// rhythmic character). Seed personality, drawn once per letter seed:
/// Syncopation offset added to `complexity`.
const PERSONA_SYNC: (f32, f32) = (-0.2, 0.35);
/// Slide probability multiplier.
const PERSONA_SLIDE: (f32, f32) = (0.3, 2.5);
/// Most likely share of beats played at half speed (only the beat's first
/// note, held), and of notes doubled into two 32nds (ratchets).
const PERSONA_HALF_MAX: f32 = 0.35;
const PERSONA_RATCHET_MAX: f32 = 0.3;
/// How much tonal stability steers the pitch walk (so seeds differ: some
/// circle the chord tones, some wander the scale).
const PERSONA_TONAL: (f32, f32) = (0.3, 2.0);

// MUSICAL: first draft, tune by ear. Motif mode.
/// Lowest MIDI key a motif bar may reach (C1); lower bars move up an octave.
const MOTIF_MIN_KEY: i32 = 24;
/// Most notes of a bar after the density's repeats.
const MOTIF_MAX_NOTES: usize = 12;
/// Written velocity from which a note is an accent.
const MOTIF_ACCENT_VEL: u8 = 110;

// MUSICAL: first draft, tune by ear (D50). Clip mode.
/// Metric weight a note needs at density 0 (the beats, 0.7, stay).
const CLIP_THIN_MAX: f32 = 0.7;
/// Weak notes (metric weight under this) may lift an octave, at this
/// chance at Spread 1.
const CLIP_LIFT_BELOW: f32 = 0.7;
const CLIP_LIFT_MAX: f32 = 0.45;
/// Written notes shorter than this are stabs.
const CLIP_SHORT_TICKS: u16 = 2 * TICKS_PER_STEP;
/// Density from which long notes split into 16ths (else 8ths).
const CLIP_SPLIT_16THS: f32 = 0.85;
/// Ticks between split repeats (near legato).
const CLIP_SPLIT_GAP: u16 = 2;
/// Written velocity from which a clip note is an accent.
const CLIP_ACCENT_VEL: u8 = 116;
// MUSICAL: tune by ear (user, 2026-10-21: the Density fill speeds the line
// up and fills its rests; it follows the Groove feel and Variation sets how
// varied it is). Density from which the rests fill on the 8th (else from
// the 16th) grid, the chance at Density 1, how much more Variation skips,
// the chance at Variation 1 a fill takes the octave or a scale neighbour.
const CLIP_FILL_8THS: (f32, f32) = (0.55, 0.95);
const CLIP_FILL_16THS: (f32, f32) = (0.8, 0.8);
const CLIP_FILL_SKIP: f32 = 0.35;
const CLIP_FILL_ALT: f32 = 0.4;
/// A fill plays this share of the velocity window under the note it fills
/// from: on the 8ths, and on the 16ths between them.
const CLIP_FILL_VEL: (f32, f32) = (0.25, 0.4);
const S_CLIP_FILL: u32 = 0x233;
const S_CLIP_FILL_ALT: u32 = 0x234;
/// How far Complexity above its default adds to the pushes.
const CLIP_COMPLEXITY_SYNC: f32 = 0.6;
const S_CLIP_LIFT: u32 = 0x230;
const S_CLIP_STAB: u32 = 0x231;
const S_CLIP_SPLIT: u32 = 0x232;

// MUSICAL: first draft, tune by ear. Register and articulation.
/// Lowest MIDI key of the key's root at the base register (E1).
const BASS_ROOT_MIN: i32 = 28;
/// Notes more than this many semitones above the root drop an octave (odd
/// blocks are exempt).
const BASS_SPAN: f32 = 12.0;
/// Accent / slide probability at parameter value 1 (the defaults of 0.5 give
/// the acid-style 30 % accents and 10 % slides).
const ACCENT_PROB_MAX: f32 = 0.5;
const SLIDE_PROB_MAX: f32 = 0.45;
// MUSICAL: first draft, tune by ear (user: a seeded, moving lowpass).
/// Where a note's lowpass sits between the lane's filter range (0 = 700 Hz,
/// 1 = 15 kHz, see `lane::BASS_FILTER`): the seed's base, then the density,
/// a repeating step motif, the pitch (per octave above the root) and the
/// accent push it up.
const FILTER_BASE: (f32, f32) = (0.35, 0.8);
const FILTER_DENSITY: f32 = 0.25;
const FILTER_MOTIF: f32 = 0.7;
const FILTER_PITCH: f32 = 0.15;
const FILTER_ACCENT: f32 = 0.15;
/// Half the filter range in octaves (700 Hz .. 15 kHz around 3.24 kHz).
const FILTER_HALF_OCTAVES: f32 = 2.21;
/// A slide needs the previous note at most this many steps back.
const SLIDE_MAX_GAP_STEPS: usize = 4;
// MUSICAL: user 2026-10-20: the Slide knob reaches about 250 ms.
/// Glide time of a slide, seconds: Slide 0 .. 1.
const SLIDE_SECS: (f32, f32) = (crate::tempo::note(3.0 / 128.0), crate::tempo::note(3.0 / 16.0));

/// The glide byte of a slide at Slide `slide`.
fn slide_glide(slide: f32) -> u8 {
    crate::sampler::glide_value(lerp(SLIDE_SECS.0, SLIDE_SECS.1, slide.clamp(0.0, 1.0)))
}

// MUSICAL: tune by ear (user, 2026-10-21: the loudness between notes
// differs far too much; at most about 20 velocity at full Variation).
/// The velocity window (the most two notes of a bar differ) at Variation 0
/// and 1, on both sources.
pub const VEL_WINDOW: (f32, f32) = (8.0, 20.0);
/// Where the generated line's velocities sit, and how far its accents and
/// normal notes lie apart before they are fitted into the window.
const GEN_VEL_CENTER: f32 = (VEL_ACCENT as f32 + super::blocks::VEL_NORMAL as f32) / 2.0;
const GEN_VEL_SPREAD: f32 = (VEL_ACCENT - super::blocks::VEL_NORMAL) as f32;
/// The accent parameter's value from which the written dynamics fill the
/// window (the macros' middle: `lerp(0.2, 0.9, 0.5)`); below it they flatten,
/// above it they stay (the window is the cap).
const ACCENT_MID: f32 = 0.55;

/// The velocity window at Variation `variation`.
pub fn vel_window(variation: f32) -> f32 {
    lerp(VEL_WINDOW.0, VEL_WINDOW.1, variation.clamp(0.0, 1.0))
}

/// How far the accent parameter `a` scales the dynamics: 0 flat, 1 from
/// [`ACCENT_MID`] on.
fn accent_depth(a: f32) -> f32 {
    (a / ACCENT_MID).clamp(0.0, 1.0)
}

/// `vel` from dynamics centred on `center` that reach `half` either way,
/// fitted into `window` around it at `depth`.
fn fit_vel(vel: f32, center: f32, half: f32, window: f32, depth: f32) -> u8 {
    let off = ((vel - center) / half.max(1.0) * window / 2.0 * depth).clamp(-window / 2.0, window / 2.0);
    (center + off).round().clamp(1.0, 127.0) as u8
}

/// Keep every note of `events` inside `window` around `center`.
fn clamp_vels(events: &mut [LiveEvent], center: f32, window: f32) {
    for e in events.iter_mut().filter(|e| e.vel > 0) {
        e.vel = (e.vel as f32).clamp(center - window / 2.0, center + window / 2.0).round().clamp(1.0, 127.0) as u8;
    }
}

/// The Slide value a slide's glide byte came from ([`slide_glide`] inverted).
pub fn slide_of_glide(glide: u8) -> f32 {
    ((crate::sampler::glide_secs(glide) - SLIDE_SECS.0) / (SLIDE_SECS.1 - SLIDE_SECS.0)).clamp(0.0, 1.0)
}
/// Ticks a slid-from note overlaps the next one so the tie finds it held.
const TIE_OVERLAP_TICKS: u16 = 2;
// MUSICAL: first draft, tune by ear (user: note length follows the density).
/// Note length as a share of the distance to the next note, at density 0
/// (sustained) and 1 (short).
const GATE_BY_DENSITY: (f32, f32) = (1.0, 0.3);
/// Longest note in steps at density 0 and 1.
const GATE_MAX_BY_DENSITY: (f32, f32) = (16.0, 4.0);
const GATE_MIN_TICKS: f32 = 6.0;
// MUSICAL: the phrase generator's length and expression (generator/phrase.rs),
// so a sound pack's shape control acts the same on generated lines.
/// Shortest stab at length 0, in ticks (a 16th and a half).
const STAB_TICKS: u16 = 36;
/// From this length held notes start to tie into the next one and glide;
/// at length 1 all of them do (user, 2026-10-17: the sustained end of shape
/// forms one line).
const LEGATO_FROM: f32 = 0.75;

/// Share of held notes that tie into the next at `length`.
// MUSICAL: first draft, tune by ear (user: Slide makes the line liquid).
/// Extra share of held notes that tie into the next at full Slide.
const LEGATO_SLIDE: f32 = 0.35;

fn legato_share(length: f32) -> f32 {
    ((length - LEGATO_FROM) / (1.0 - LEGATO_FROM)).clamp(0.0, 1.0)
}

/// How the bass treats steps with a kick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KickMode {
    Avoid,
    Free,
    Double,
}

impl KickMode {
    pub fn from_param(v: f32) -> Self {
        if v < 1.0 / 3.0 {
            KickMode::Avoid
        } else if v < 2.0 / 3.0 {
            KickMode::Free
        } else {
            KickMode::Double
        }
    }
}

/// Rule switches and scales, fixed at construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BassRules {
    /// A3: bars 0-2 repeat, bar 3 varies.
    pub phrase: bool,
    /// A5: velocity tier depth.
    pub tiers: f32,
    /// A6: strength of gap-following (rhythm).
    pub gaps: f32,
    /// A6: share of notes drawn from the root-heavy pool (the rest are any
    /// scale degree).
    pub root: f32,
    /// Play motif clips when the generator has them.
    pub motifs: bool,
    /// Each seed's rhythmic character ([`Persona`]): syncopation, slides,
    /// half-speed beats and ratchets.
    pub persona: bool,
    /// Pitch as a walk from the previous note, its steps sized by `spread`
    /// ([`walk_step`]); off: each note drawn from the root-heavy pool.
    pub walk: bool,
    /// Rhythm from the built-in groove vocabulary ([`GROOVES`]) and repeats
    /// only as fills; off: the step-importance rhythm everywhere.
    pub scheme: bool,
    /// Rhythm cells ([`CELLS`]): a seeded share of beats from another grid
    /// (triplets, dotted, ratchet clusters, sustains); off: the step line only.
    pub cells: bool,
}

impl Default for BassRules {
    fn default() -> Self {
        Self { phrase: true, tiers: 1.0, gaps: 1.0, root: 1.0, motifs: true, persona: true, walk: true, scheme: true, cells: true }
    }
}

impl BassRules {
    pub fn from_spec(params: &BTreeMap<String, f32>) -> Self {
        let a6 = option(params, "rule_a6", 1.0);
        Self {
            phrase: option(params, "rule_a3", 1.0) >= 0.5,
            tiers: option(params, "rule_a5", 1.0),
            gaps: a6,
            root: a6,
            motifs: option(params, "motifs", 1.0) >= 0.5,
            persona: option(params, "persona", 1.0) >= 0.5,
            walk: option(params, "walk", 1.0) >= 0.5,
            scheme: option(params, "scheme", 1.0) >= 0.5,
            cells: option(params, "cells", 1.0) >= 0.5,
        }
    }
}

/// Octave (for `HarmonyCtx::resolve`) that puts the key's root at or just
/// above `min_midi`.
pub(super) fn root_octave(key: MusicalKey, min_midi: i32) -> i32 {
    (min_midi - key.root as i32 + 11).div_euclid(12) - 1
}

/// The root's MIDI note the generator plays for `key` at the centre
/// register (E1..D#2). The held note's octave moves it (D7).
pub fn home_root(key: MusicalKey) -> i32 {
    key.root as i32 + 12 * (root_octave(key, BASS_ROOT_MIN) + 1)
}

/// Register parameter as an octave shift: −1, 0 or +1.
pub(super) fn register_shift(v: f32) -> i32 {
    (v * 2.0).round() as i32 - 1
}

/// Bass generator. See the module docs.
pub struct BassGenerator {
    defaults: [f32; MAX_GEN_PARAMS],
    rules: BassRules,
    /// Idiom rhythm cells. Empty = procedural rhythm.
    cells: Vec<LevelMap>,
    /// Motif clips. Empty = the procedural line.
    motifs: Vec<Motif>,
    /// The bass clips per letter (D50), played when the source is Clips.
    clips: Arc<ClipSet>,
}

impl BassGenerator {
    pub fn new(defaults: [f32; MAX_GEN_PARAMS], rules: BassRules, cells: Vec<LevelMap>) -> Self {
        Self { defaults, rules, cells, motifs: Vec::new(), clips: Arc::new(ClipSet::default()) }
    }

    /// Play from these clips when the source is Clips (D50).
    pub fn with_clips(mut self, clips: Arc<ClipSet>) -> Self {
        self.clips = clips;
        self
    }

    /// The clip `ctx`'s letter plays in clip mode, if any.
    fn clip(&self, ctx: &BarCtx) -> Option<&BassClip> {
        if param(ctx, P_SOURCE) < 0.5 {
            return None;
        }
        self.clips.pick(ctx.letter, ctx.seed, ctx.lane)
    }

    /// Play these motif clips (when the rules allow it).
    pub fn with_motifs(mut self, motifs: Vec<Motif>) -> Self {
        self.motifs = motifs;
        self
    }

    fn uses_motifs(&self) -> bool {
        self.rules.motifs && !self.motifs.is_empty()
    }

    /// Draw a chord-relative degree from the pool.
    /// Event `cutoff` of a note on `step`, `above` semitones over the root.
    fn cutoff(&self, ctx: &BarCtx, step: u8, above: i32, accented: bool) -> i8 {
        let base = rng::range_f32(rng::hash(ctx.seed, ctx.lane, 0, 0, S_FILTER), FILTER_BASE.0, FILTER_BASE.1);
        let u = base
            + FILTER_DENSITY * note_param(ctx, P_DENSITY, step)
            + FILTER_MOTIF * (step_motif(ctx, step, S_FILTER) - 0.5)
            + FILTER_PITCH * above as f32 / 12.0
            + if accented { FILTER_ACCENT } else { 0.0 };
        let octaves = (u.clamp(0.0, 1.0) * 2.0 - 1.0) * FILTER_HALF_OCTAVES;
        (octaves / CUTOFF_OFFSET_OCTAVES * 127.0).round().clamp(-127.0, 127.0) as i8
    }

    /// Motif mode: the seed's clip, re-pitched to the bar's key, mode and
    /// chord. Only slide, accent and the note filter vary per note.
    fn motif_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        let variation = param(ctx, P_VARIATION);
        let clip = &self.motifs[motif::pick(&self.motifs, ctx.seed, ctx.lane, &[])];
        let shape = Shape {
            density,
            spread: param(ctx, P_SPREAD),
            variation,
            max_notes: MOTIF_MAX_NOTES,
            lead: false,
            play: Play::Whole,
            yield_to: 0,
            yield_prob: 0.0,
        };
        let mut plan = Plan::new();
        motif::plan_bar(clip, ctx, &shape, &mut plan);
        if plan.as_slice().is_empty() {
            return;
        }
        let octave = root_octave(ctx.harmony.key, BASS_ROOT_MIN) + register_shift(param(ctx, P_REGISTER));
        let fold = motif::fold(ctx, octave, &plan, MOTIF_MIN_KEY);
        let root_key = ctx.harmony.resolve(0, octave) as i32 + fold;
        let rng = PhraseRng::new(ctx, variation, self.rules.phrase);
        let attack = density_attack_in(density, BASS_ATTACK_BY_DENSITY);
        // (tick, key) of the previous note, for slides.
        let mut prev: Option<(u16, i32)> = None;
        let mut events = [LiveEvent::default(); motif::MAX_PLAN];
        let mut n = 0usize;
        for (i, p) in plan.as_slice().iter().enumerate() {
            let step = p.step() as u8;
            let key = motif::note_key(ctx, octave, fold, p).clamp(0, 127);
            let shape = NoteShape::note(step, p.gate, key, ctx.harmony.key.root);
            let random_accent = rng.chance(step, S_ACCENT, note_param_for(ctx, P_ACCENT, shape) * ACCENT_PROB_MAX);
            let accented = random_accent || p.vel >= MOTIF_ACCENT_VEL;
            let vel = if random_accent { p.vel.max(VEL_ACCENT) } else { p.vel };
            let mut ev = note(p.tick, key as u8, vel, p.gate, i as u16 + 1);
            ev.attack = attack;
            ev.start = stream_start(ctx, step, 0);
            ev.cutoff = self.cutoff(ctx, step, key - root_key, accented);
            if accented {
                ev.flags |= flags::ACCENT;
            }
            if let Some((prev_tick, prev_key)) = prev {
                if p.tick - prev_tick <= SLIDE_MAX_GAP_STEPS as u16 * TICKS_PER_STEP
                    && key != prev_key
                    && rng.chance(step, S_SLIDE, note_param_for(ctx, P_SLIDE, shape) * SLIDE_PROB_MAX)
                {
                    ev.flags |= flags::TIE;
                    ev.glide = slide_glide(note_param_for(ctx, P_SLIDE, shape));
                    // Hold the previous note into this one.
                    events[n - 1].gate = p.tick - prev_tick + TIE_OVERLAP_TICKS;
                }
            }
            events[n] = ev;
            n += 1;
            prev = Some((p.tick, key));
        }
        shape_notes(&mut events[..n], |e| note_length(ctx, e), param(ctx, P_EXPRESSION), param(ctx, P_SLIDE), |step| rng.unit(step, S_LEGATO));
        for ev in &events[..n] {
            out.push(*ev);
        }
    }

    /// Clip mode (D50): bar `ctx.material_bar` of the letter's clip as
    /// written, its root on the key's root and its notes in the key's scale,
    /// moved with the chord by scale steps. The knobs reshape it as they
    /// shape the generated line: Density at its middle plays the clip as
    /// written, lower thins the weak notes (the beats stay), higher splits
    /// long notes into 8ths or 16ths; Spread lifts weak notes an octave;
    /// Syncopation (and Complexity above its default) pushes notes off the
    /// 8ths; Stabs shortens held notes or holds stabs; Register moves the
    /// octave; the accent, slide, Length, expression and loops act as on the
    /// generated line; a drum host's kicks thin it (Avoid) or double it.
    fn clip_bar(&self, ctx: &BarCtx, clip: &BassClip, out: &mut BarEvents) {
        let bar_ticks = TICKS_PER_BAR as u32;
        let bars = clip.ticks().div_ceil(bar_ticks).max(1);
        let lo = (ctx.material_bar as u32 % bars) * bar_ticks;
        let hi = (lo + bar_ticks).min(clip.ticks());
        let density = param(ctx, P_DENSITY);
        let variation = param(ctx, P_VARIATION);
        let persona = if self.rules.persona { Persona::new(ctx) } else { Persona::PLAIN };
        let rng = PhraseRng::new(ctx, variation, self.rules.phrase);
        let octave = root_octave(ctx.harmony.key, BASS_ROOT_MIN) + register_shift(param(ctx, P_REGISTER));
        let home = ctx.harmony.resolve_in_key(0, octave) as i32;
        let mut fold = 0;
        while home + fold + clip.lowest() < MOTIF_MIN_KEY {
            fold += 12;
        }
        let scale = ctx.harmony.scale();
        let root_pc = ctx.harmony.key.root as i32;
        // The chord moves the clip by scale steps, the nearer way.
        let chord = match ctx.harmony.chord_degree as i32 {
            d if d > 3 => d - 7,
            d => d,
        };
        let to_key = |semis: i32, degree: i32| {
            let k = home + fold + clips::snap(semis, degree, scale);
            if chord == 0 {
                return k;
            }
            let rel = k - root_pc;
            let idx = scale.iter().position(|&s| s as i32 == rel.rem_euclid(12)).unwrap_or(0) as i32 + chord;
            root_pc + 12 * (rel.div_euclid(12) + idx.div_euclid(7)) + scale[idx.rem_euclid(7) as usize] as i32
        };
        let is_kick = |s: usize| ctx.drum_mask[s.min(STEPS_PER_BAR - 1)] >= MASK_KICK_MIN;
        let mode = KickMode::from_param(param(ctx, P_KICK_MODE));
        let attack = density_attack_in(density, BASS_ATTACK_BY_DENSITY);
        // The written dynamics, fitted into the Variation's window around the
        // clip's mean; the accent sets how far they reach (no random accents).
        let window = vel_window(variation);
        let (lo_vel, hi_vel, sum) = clip.notes.iter().fold((127u8, 0u8, 0u32), |(l, h, s), n| (l.min(n.vel), h.max(n.vel), s + n.vel as u32));
        let center = sum as f32 / clip.notes.len().max(1) as f32;
        let half = ((hi_vel - lo_vel.min(hi_vel)) as f32 / 2.0).max(1.0);
        let depth = accent_depth(param(ctx, P_ACCENT));
        let mut events = [LiveEvent::default(); 2 * STEPS_PER_BAR];
        let mut n = 0usize;
        let mut prev: Option<(u16, i32)> = None;
        let mut first = true;
        for (i, cn) in clip.notes.iter().enumerate().filter(|(_, x)| (lo..hi).contains(&x.tick)) {
            if n >= events.len() {
                break;
            }
            let tick = (cn.tick - lo) as u16;
            let step = ((tick + TICKS_PER_STEP / 2) / TICKS_PER_STEP).min(STEPS_PER_BAR as u16 - 1) as u8;
            let weight = metric_weight(step as usize);
            // Density below its middle thins the weak notes; the bar's first
            // note and the beats stay.
            let d = note_param(ctx, P_DENSITY, step);
            let need = CLIP_THIN_MAX * (1.0 - d / 0.5).max(0.0);
            if !first && weight < need {
                continue;
            }
            if !first && mode == KickMode::Avoid && is_kick(step as usize) && weight < 1.0 {
                continue;
            }
            let mut key = to_key(cn.semis as i32, cn.degree as i32);
            // Spread lifts weak notes an octave.
            if weight < CLIP_LIFT_BELOW && rng.chance(step, S_CLIP_LIFT, note_param(ctx, P_SPREAD, step) * CLIP_LIFT_MAX) {
                key += 12;
            }
            let key = key.clamp(0, 127);
            let mut gate = cn.len.min(hi - cn.tick).min(u16::MAX as u32) as u16;
            let shape = NoteShape::note(step, gate, key, ctx.harmony.key.root);
            // Stabs: above its middle held notes turn short, below stabs hold.
            let stabs = note_param(ctx, P_STABS, step);
            let written_short = gate < CLIP_SHORT_TICKS;
            let mut short = written_short;
            if !written_short && rng.chance(step, S_CLIP_STAB, (2.0 * stabs - 1.0).max(0.0)) {
                short = true;
                gate = (gate / 2).max(GATE_MIN_TICKS as u16);
            } else if written_short && rng.chance(step, S_CLIP_STAB, (1.0 - 2.0 * stabs).max(0.0)) {
                short = false;
                let next = clip.notes.get(i + 1).map_or(hi, |x| x.tick.min(hi));
                gate = (next - cn.tick).min(u16::MAX as u32) as u16;
            }
            let accented = cn.vel >= CLIP_ACCENT_VEL;
            let vel = fit_vel(cn.vel as f32, center, half, window, depth);
            let mut ev = note(tick, key as u8, vel, gate, n as u16 + 1);
            ev.attack = attack;
            ev.start = stream_region(ctx, step);
            ev.cutoff = self.cutoff(ctx, step, key - home, accented);
            if accented {
                ev.flags |= flags::ACCENT;
            }
            if short {
                ev.flags |= flags::STAB;
            }
            if let Some((prev_tick, prev_key)) = prev {
                if tick - prev_tick <= SLIDE_MAX_GAP_STEPS as u16 * TICKS_PER_STEP
                    && key != prev_key
                    && rng.chance(step, S_SLIDE, note_param_for(ctx, P_SLIDE, shape) * SLIDE_PROB_MAX * persona.slide)
                {
                    ev.flags |= flags::TIE;
                    ev.glide = slide_glide(note_param_for(ctx, P_SLIDE, shape));
                    events[n - 1].gate = tick - prev_tick + TIE_OVERLAP_TICKS;
                }
            }
            events[n] = ev;
            n += 1;
            prev = Some((tick, key));
            first = false;
        }
        // A drum host's kicks on empty steps double with the root.
        if mode == KickMode::Double {
            for s in (0..STEPS_PER_BAR).filter(|&s| is_kick(s)) {
                let t = (s as u16) * TICKS_PER_STEP;
                if n < events.len() && !events[..n].iter().any(|e| e.tick <= t && e.tick + e.gate > t) {
                    let k = to_key(0, 0).clamp(0, 127);
                    let mut ev = note(t, k as u8, VEL_ACCENT, TICKS_PER_STEP, n as u16 + 1);
                    ev.attack = attack;
                    ev.start = stream_region(ctx, s as u8);
                    ev.flags |= flags::STAB;
                    events[n] = ev;
                    n += 1;
                }
            }
            events[..n].sort_by_key(|e| e.tick);
        }
        // Density above its middle splits long notes into repeats (8ths,
        // 16ths near the top) of the same pitch.
        if density > 0.5 {
            let mut k = 0;
            while k < n {
                let e = events[k];
                let d = note_param(ctx, P_DENSITY, e.step);
                let sub = if d >= CLIP_SPLIT_16THS { TICKS_PER_STEP } else { 2 * TICKS_PER_STEP };
                let room = events.len() - n;
                let parts = (e.gate / sub) as usize;
                if e.kind != EventKind::NoteOn || e.has(flags::TIE) || parts < 2 || room == 0 || !rng.chance(e.step, S_CLIP_SPLIT, (2.0 * d - 1.0).max(0.0)) {
                    k += 1;
                    continue;
                }
                let extra = (parts - 1).min(room);
                events[k].gate = sub - CLIP_SPLIT_GAP;
                events.copy_within(k + 1..n, k + 1 + extra);
                for r in 1..=extra {
                    let mut x = e;
                    x.tick = e.tick + r as u16 * sub;
                    x.step = (x.tick / TICKS_PER_STEP) as u8;
                    x.gate = sub - CLIP_SPLIT_GAP;
                    x.vel = (e.vel as f32 - CLIP_FILL_VEL.0 * window).max(1.0) as u8;
                    x.flags &= !(flags::ACCENT | flags::TIE);
                    x.note_id = e.note_id + 64 + r as u16;
                    events[k + r] = x;
                }
                n += extra;
                k += 1 + extra;
            }
        }
        // Density above its middle also fills the rests after a note, on the
        // Groove's feel (8ths, then 16ths near the top).
        if density > CLIP_FILL_8THS.0 {
            n = self.fill_rests(ctx, &mut events, n, &rng, window, root_pc, scale);
        }
        let complexity = param(ctx, P_COMPLEXITY);
        let push = (param(ctx, P_SYNCOPATION) + CLIP_COMPLEXITY_SYNC * (complexity - PARAM_DEFAULTS[P_COMPLEXITY]).max(0.0)).clamp(0.0, 1.0);
        syncopate(&mut events[..n], push, &rng);
        shape_notes(&mut events[..n], |e| note_length(ctx, e), param(ctx, P_EXPRESSION), param(ctx, P_SLIDE), |step| rng.unit(step, S_LEGATO));
        loops(&mut events[..n], variation * persona.looping, persona.reverses, &rng);
        clamp_vels(&mut events[..n], center, window);
        for ev in &events[..n] {
            out.push(*ev);
        }
    }

    /// Fill the rests of `events[..n]` (sorted) after their notes, on the
    /// grid of the Groove's feel: the 8ths from [`CLIP_FILL_8THS`], the 16ths
    /// between them from [`CLIP_FILL_16THS`]. A fill repeats the note before
    /// it, softer, held to the next grid point (near legato); Variation skips
    /// some places and turns some fills to the octave or a scale neighbour.
    /// Returns the new count.
    #[allow(clippy::too_many_arguments)]
    fn fill_rests(&self, ctx: &BarCtx, events: &mut [LiveEvent; 2 * STEPS_PER_BAR], mut n: usize, rng: &PhraseRng, window: f32, root_pc: i32, scale: &[u8; 7]) -> usize {
        let d = param(ctx, P_DENSITY);
        let variation = param(ctx, P_VARIATION);
        let ramp = |(from, max): (f32, f32), top: f32| ((d - from) / (top - from)).clamp(0.0, 1.0) * max;
        let (p8, p16) = (ramp(CLIP_FILL_8THS, 0.9), ramp(CLIP_FILL_16THS, 1.0));
        // The Groove's feel: triplet, straight or dotted 16ths.
        let sub = ((TICKS_PER_STEP as f32) * crate::tempo::feel(param(ctx, P_SYNCOPATION))).round() as u32;
        let neighbour = |key: i32, up: bool| {
            let rel = key - root_pc;
            let Some(i) = scale.iter().position(|&s| s as i32 == rel.rem_euclid(12)) else { return key };
            let j = i as i32 + if up { 1 } else { -1 };
            root_pc + 12 * (rel.div_euclid(12) + j.div_euclid(7)) + scale[j.rem_euclid(7) as usize] as i32
        };
        let bar = TICKS_PER_BAR as u32;
        let count = n;
        let mut k = 0u32;
        while k * sub < bar {
            let t = k * sub;
            let on_8th = k % 2 == 0;
            let p = if on_8th { p8 } else { p16 } * (1.0 - CLIP_FILL_SKIP * variation);
            let slot = (k as u8).wrapping_add(64);
            k += 1;
            if p <= 0.0 || n >= events.len() || !rng.chance(slot, S_CLIP_FILL, p) {
                continue;
            }
            // A rest: nothing sounds there or starts within the grid step (a
            // note as long as it plays at a short Length).
            let busy = events[..n].iter().any(|e| {
                let l = note_length(ctx, e);
                let gate = if l < 0.5 && e.flags & flags::FIGURE == 0 { shortened(e.gate, l) } else { e.gate };
                let (a, b) = (e.tick as u32, e.tick as u32 + gate.max(1) as u32);
                (a <= t && t < b) || (t < a && a < t + sub)
            });
            let Some(src) = events[..count].iter().filter(|e| (e.tick as u32) < t && e.kind == EventKind::NoteOn).last().copied() else { continue };
            if busy {
                continue;
            }
            let next = events[..n].iter().map(|e| e.tick as u32).filter(|&a| a > t).min().unwrap_or(bar);
            let mut x = src;
            let alt = rng.unit(slot, S_CLIP_FILL_ALT);
            if alt < CLIP_FILL_ALT * variation {
                let k = src.key as i32;
                x.key = match (alt * 3.0 / (CLIP_FILL_ALT * variation).max(1e-6)) as u8 {
                    0 => k + 12,
                    1 => neighbour(k, true),
                    _ => neighbour(k, false),
                }
                .clamp(0, 127) as u8;
            }
            x.tick = t as u16;
            x.step = (t / TICKS_PER_STEP as u32).min(STEPS_PER_BAR as u32 - 1) as u8;
            x.gate = ((t + sub).min(next) - t).saturating_sub(CLIP_SPLIT_GAP as u32).max(GATE_MIN_TICKS as u32) as u16;
            x.vel = (src.vel as f32 - window * if on_8th { CLIP_FILL_VEL.0 } else { CLIP_FILL_VEL.1 }).max(1.0) as u8;
            x.flags &= !(flags::ACCENT | flags::TIE | flags::STAB);
            x.note_id = 200 + n as u16;
            events[n] = x;
            n += 1;
        }
        events[..n].sort_unstable_by_key(|e| e.tick);
        n
    }

    fn pick_degree(&self, h: u64, spread: f32) -> i32 {
        if !rng::chance(rng::derive(h, 1), self.rules.root) {
            return rng::below(h, SCALE_LEN as u32) as i32;
        }
        let factor = lerp(POOL_FACTOR.0, POOL_FACTOR.1, spread);
        let mut weights = [0.0f32; BASS_POOL.len()];
        for (i, &(degree, w)) in BASS_POOL.iter().enumerate() {
            weights[i] = if degree == 0 { w } else { w * factor };
        }
        BASS_POOL[rng::weighted_pick(h, &weights).unwrap_or(0)].0
    }
}

impl LaneGenerator for BassGenerator {
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        if density <= 0.0 {
            return;
        }
        if let Some(clip) = self.clip(ctx) {
            self.clip_bar(ctx, clip, out);
            return;
        }
        if self.uses_motifs() {
            self.motif_bar(ctx, out);
            return;
        }
        let complexity = param(ctx, P_COMPLEXITY);
        let persona = if self.rules.persona { Persona::new(ctx) } else { Persona::PLAIN };
        let variation = param(ctx, P_VARIATION);
        let mode = KickMode::from_param(param(ctx, P_KICK_MODE));
        let rng = PhraseRng::new(ctx, variation, self.rules.phrase);
        let cell = (!self.cells.is_empty())
            .then(|| &self.cells[rng::below(rng.base.at(0, S_CELL), self.cells.len() as u32) as usize]);
        // The built-in groove scheme (user, 2026-10-17): a seeded figure from
        // the bass vocabulary, picked by syncopation (complexity).
        let groove = (self.rules.scheme && cell.is_none())
            .then(|| pick_groove(rng.base.at(0, S_GROOVE), (complexity + persona.sync).clamp(0.0, 1.0)));
        let is_kick = |s: usize| ctx.drum_mask[s] >= MASK_KICK_MIN;
        // Each step reads its own density (per-note variation).
        let step_density = |d: f32| lerp(RHYTHM_DENSITY.0, RHYTHM_DENSITY.1, d.powf(RHYTHM_CURVE));
        let thin = THIN_PENALTY * (1.0 - density / THIN_DENSITY_MAX).max(0.0);

        // Rhythm.
        let mut onsets = 0u16;
        if let Some(gv) = groove {
            // Density reveals the figure's levels: the core first, then the
            // answers, the anticipations last.
            // Stabs leave holes, so a stabby, dense line also fills the
            // figure's empty 8ths (user, 2026-10-17).
            // The fill comes from the figure itself: its own hits again a beat or
            // half a bar later (by the seed), on the 8th grid, then any empty 8th.
            let stab_fill = (2.0 * param(ctx, P_STABS) - 1.0).max(0.0);
            let shift = if rng::chance(rng.base.at(0, S_GROOVE + 1), 0.5) { 4 } else { 8 };
            for s in 0..STEPS_PER_BAR {
                let echo = if s % 2 == 0 { (gv.levels[(s + STEPS_PER_BAR - shift) % STEPS_PER_BAR] as f32 * GROOVE_ECHO) as u8 } else { 0 };
                let own = gv.levels[s];
                let level = if own == 0 && s % 2 == 0 { echo.max(GROOVE_FILL_LEVEL) } else { own };
                if level == 0 {
                    continue;
                }
                let d = note_param(ctx, P_DENSITY, s as u8);
                let threshold =
                    (lerp(GROOVE_REVEAL.0, GROOVE_REVEAL.1, d) - (GROOVE_FILL_REACH + GROOVE_STAB_REACH * stab_fill) * d) as i32;
                // The core plays with the kick; the rest leaves it room.
                let kick_clash = is_kick(s) && mode == KickMode::Avoid && level < 255;
                if level as i32 >= threshold && !kick_clash {
                    onsets |= 1 << s;
                }
            }
        }
        for s in 0..STEPS_PER_BAR {
            if groove.is_some() {
                break;
            }
            let kick = is_kick(s);
            if kick && mode == KickMode::Avoid {
                continue;
            }
            let mut importance = match cell {
                Some(c) => level_unit(c.at(ctx.bar_in_phrase, s)),
                None => {
                    let gap = if ctx.drum_mask[s] == 0 {
                        GAP_BONUS
                    } else if kick {
                        0.0
                    } else {
                        -ONSET_PENALTY
                    };
                    let cx = (note_param(ctx, P_COMPLEXITY, s as u8) + persona.sync).clamp(0.0, 1.0);
                    lerp(BASS_SIMPLE[s], BASS_SYNC[s], cx) + self.rules.gaps * gap
                        - if s % 2 == 1 { thin } else { 0.0 }
                }
            };
            if kick && mode == KickMode::Double {
                importance = 1.0;
            }
            if importance > 0.0
                && reveals(importance, step_density(note_param(ctx, P_DENSITY, s as u8)), complexity * BASS_CHAOS_MAX, rng.unit(s as u8, S_RHYTHM))
            {
                onsets |= 1 << s;
            }
        }

        // Half speed: some beats keep only their first note, held.
        for beat in 0..STEPS_PER_BAR / 4 {
            if groove.is_some() {
                break;
            }
            let mask = 0xFu16 << (beat * 4);
            let first = (onsets & mask).trailing_zeros();
            if (onsets & mask).count_ones() > 1 && rng.chance((beat * 4) as u8, S_HALF, persona.half) {
                onsets = (onsets & !mask) | 1 << first;
            }
        }

        // Phrase-end turnaround: one more note on the last free step, or, in
        // a full bar, a new pitch on the last note.
        let mut turnaround = None;
        if rng.is_variation_bar() {
            turnaround = TURNAROUND_STEPS
                .iter()
                .copied()
                .find(|&s| onsets & (1 << s) == 0 && !(mode == KickMode::Avoid && is_kick(s)));
            match turnaround {
                Some(s) => onsets |= 1 << s,
                None if onsets != 0 => turnaround = Some(15 - onsets.leading_zeros() as usize),
                None => {}
            }
        }

        // Pitch and articulation.
        let octave = root_octave(ctx.harmony.key, BASS_ROOT_MIN) + register_shift(param(ctx, P_REGISTER));
        let spread = param(ctx, P_SPREAD);
        let ceiling = ctx.harmony.resolve_in_key(0, octave) as i32 + BASS_SPAN as i32;
        // Odd block: notes `start..start + len` of the bar jump up by `size`.
        // The bar's first note stays home (the root), so a block starts later.
        let block_prob = lerp(JUMP_BLOCK_PROB.0, JUMP_BLOCK_PROB.1, spread);
        let block = (spread >= JUMP_SPREAD_MIN && rng.chance(0, S_JUMP, block_prob) && onsets.count_ones() > 1).then(|| {
            let largest = (((spread - JUMP_SPREAD_MIN) / (1.0 - JUMP_SPREAD_MIN)) * JUMPS.len() as f32 - 0.001) as usize;
            let largest = largest.min(JUMPS.len() - 1);
            let size = if rng.chance(0, S_JUMP_SIZE, JUMP_LARGEST_SHARE) {
                JUMPS[largest]
            } else {
                JUMPS[rng::below(rng.at(1, S_JUMP_SIZE), largest as u32 + 1) as usize]
            };
            let start = 1 + rng::below(rng.at(0, S_JUMP_START), onsets.count_ones() - 1) as usize;
            let len = 1 + rng::below(rng.at(0, S_JUMP_LEN), JUMP_BLOCK_MAX) as usize;
            (start, len, size)
        });
        let mut note_index = 0usize;
        let gate_frac = lerp(GATE_BY_DENSITY.0, GATE_BY_DENSITY.1, density);
        let gate_max = lerp(GATE_MAX_BY_DENSITY.0, GATE_MAX_BY_DENSITY.1, density).round() as usize;
        let root_key = ctx.harmony.resolve(0, octave) as i32;
        let floor = (root_key - BELOW_ROOT_SEMITONES).max(ctx.harmony.resolve_in_key(0, octave) as i32 - BELOW_KEY_FLOOR);

        // Room for repeats (the array holds two per step on average).
        let mut events = [LiveEvent::default(); 2 * STEPS_PER_BAR];
        let mut n = 0usize;
        // Step and degree of the previous note.
        let mut prev: Option<(usize, i32)> = None;
        // The walk's degree (before odd blocks and turnarounds).
        let mut walked = 0i32;
        let mut last_move = 0i32;
        for s in 0..STEPS_PER_BAR {
            if onsets & (1 << s) == 0 {
                continue;
            }
            let su = s as u8;
            let first = prev.is_none() && self.rules.root >= 0.5;
            let note_spread = note_param(ctx, P_SPREAD, su);
            // Splice: how each candidate moves against a pitched host (D46).
            let prev_note = prev.map(|(ps, _)| (ps, events[n - 1].key as i32));
            let fit = |d: i32| crate::splice::motion_weight(&ctx.host, s, prev_note, ctx.harmony.resolve(d, octave) as i32);
            let mut degree = if first {
                0
            } else if self.rules.walk {
                walk_step(walked, last_move, rng.at(su, S_PITCH), note_spread, persona.tonal, &fit)
            } else {
                self.pick_degree(rng.at(su, S_PITCH), note_spread)
            };
            last_move = if first { 0 } else { degree - walked };
            walked = degree;
            if turnaround == Some(s) {
                // Differ from what the repeating bars play on this step.
                let repeated = if first {
                    0
                } else if self.rules.walk {
                    degree
                } else {
                    self.pick_degree(rng.base.at(su, S_PITCH), spread)
                };
                let i = rng::below(rng.bar.at(su, S_TURN), TURNAROUND_DEGREES.len() as u32) as usize;
                degree = if TURNAROUND_DEGREES[i] == repeated {
                    TURNAROUND_DEGREES[(i + 1) % TURNAROUND_DEGREES.len()]
                } else {
                    TURNAROUND_DEGREES[i]
                };
            }
            let mut key = ctx.harmony.resolve(degree, octave) as i32;
            if self.rules.walk && turnaround != Some(s) {
                // The walk steps back inside the register instead of
                // folding an octave (a fold would be a jump).
                while key < floor && degree < WALK_RANGE.1 {
                    degree += 1;
                    key = ctx.harmony.resolve(degree, octave) as i32;
                }
                while key > ceiling && degree > WALK_RANGE.0 {
                    degree -= 1;
                    key = ctx.harmony.resolve(degree, octave) as i32;
                }
                walked = degree;
            }
            while key > ceiling {
                key -= 12;
            }
            // Below the root: no further than a 6th under it.
            while key < floor {
                key += 12;
            }
            // The odd block jumps from where the line would have been.
            if let Some((start, len, size)) = block {
                if (start..start + len).contains(&note_index) {
                    key = ctx.harmony.resolve(degree + size, octave) as i32;
                    while key > ceiling + 12 * (size / SCALE_LEN + 1) {
                        key -= 12;
                    }
                    degree += size;
                }
            }
            note_index += 1;

            let next = (s + 1..STEPS_PER_BAR).find(|&t| onsets & (1 << t) != 0).unwrap_or(STEPS_PER_BAR);
            // A seeded mix of short stabs and held notes: density sets the
            // balance, `stabs` moves it (0.5 = as the density says).
            let stab_share = lerp(SHORT_SHARE.0, SHORT_SHARE.1, note_param(ctx, P_DENSITY, su))
                + STABS_SHIFT * (note_param(ctx, P_STABS, su) - 0.5);
            // No stabs at all at the sustained end (stabs 0).
            let stab_share = stab_share.clamp(STABS_RANGE.0, STABS_RANGE.1) * (2.0 * note_param(ctx, P_STABS, su)).min(1.0);
            // Splice: lengths alike or against a pitched host's (D46). The
            // bar's first note stays as it is, an anchor (music theory §3.3).
            let stab_share = if first { stab_share } else { crate::splice::short_share(&ctx.host, s, stab_share) };
            let short = rng.chance(su, S_LENGTH, stab_share);
            let gate_ticks = if short {
                let span = ((next - s).min(SHORT_LENGTH.2 as usize) as u16 * TICKS_PER_STEP) as f32;
                (span * lerp(SHORT_LENGTH.0, SHORT_LENGTH.1, density)).max(GATE_MIN_TICKS).round() as u16
            } else {
                let span = ((next - s).min(gate_max) as u16 * TICKS_PER_STEP) as f32;
                (span * gate_frac.max(0.85)).max(GATE_MIN_TICKS).round() as u16
            };

            let shape = NoteShape::note(su, gate_ticks, key, ctx.harmony.key.root);
            // Accents mostly on the strong steps (user, 2026-10-21: less chance).
            let accented = rng.chance(su, S_ACCENT, note_param_for(ctx, P_ACCENT, shape) * ACCENT_PROB_MAX * metric_weight(s));
            let tier = if accented { Tier::Accent } else { Tier::Normal };
            let vel = velocity(tier, rng.unit(su, S_VEL), self.rules.tiers);
            let mut ev = note(step_tick(s), key.clamp(0, 127) as u8, vel, gate_ticks, s as u16 + 1);
            ev.attack = density_attack_in(density, BASS_ATTACK_BY_DENSITY);
            ev.start = stream_region(ctx, su);
            ev.cutoff = self.cutoff(ctx, su, key - root_key, accented);
            if accented {
                ev.flags |= flags::ACCENT;
            }
            if short {
                ev.flags |= flags::STAB;
            }
            if let Some((prev_step, prev_degree)) = prev {
                if s - prev_step <= SLIDE_MAX_GAP_STEPS
                    && degree != prev_degree
                    && key != events[n - 1].key as i32
                    && rng.chance(su, S_SLIDE, note_param_for(ctx, P_SLIDE, shape) * SLIDE_PROB_MAX * persona.slide)
                {
                    ev.flags |= flags::TIE;
                    ev.glide = slide_glide(note_param_for(ctx, P_SLIDE, shape));
                    // Hold the previous note into this one.
                    events[n - 1].gate = (s - prev_step) as u16 * TICKS_PER_STEP + TIE_OVERLAP_TICKS;
                }
            }
            events[n] = ev;
            n += 1;
            prev = Some((s, degree));
        }
        // Ratchets belong to the stabby side: none at the sustained end. With
        // the groove scheme they are fills: the phrase's last beat and fill
        // bars only.
        if self.rules.cells {
            n = rhythm_cells(&mut events, n, ctx, &rng, variation);
        }
        syncopate(&mut events[..n], param(ctx, P_SYNCOPATION), &rng);
        let ratchet = persona.ratchet * (2.0 * param(ctx, P_STABS)).min(1.0);
        let fill_bar = ctx.fill || ctx.drum_fill;
        let ratchet_at = |step: u8| {
            !self.rules.scheme || fill_bar || (ctx.bar_in_phrase == VARIATION_BAR && step >= RATCHET_FROM_STEP)
        };
        let figure_weights: [f32; 4] = std::array::from_fn(|k| {
            FIGURE_WEIGHTS[k] * lerp(FIGURE_SEED_SCALE.0, FIGURE_SEED_SCALE.1, rng::unit_f32(rng::hash(ctx.seed, ctx.lane, 0, k as u8, S_FIGURE)))
        });
        n = ratchets(
            &mut events,
            n,
            |step, key| ratchet_at(step) && rng.chance(step, S_RATCHET, RATCHET_FILL_BOOST * ratchet * repeat_height(key as i32 - root_key)),
            |step| rng::weighted_pick(rng.at(step, S_FIGURE), &figure_weights).unwrap_or(0),
        );
        shape_notes(&mut events[..n], |e| note_length(ctx, e), param(ctx, P_EXPRESSION), param(ctx, P_SLIDE), |step| rng.unit(step, S_LEGATO));
        // Loops last, on the notes as long as density and length left them
        // (Splice thins the bar after this).
        loops(&mut events[..n], variation * persona.looping, persona.reverses, &rng);
        // The dynamics inside the Variation's window.
        let window = vel_window(variation);
        for e in events[..n].iter_mut().filter(|e| e.vel > 0) {
            e.vel = fit_vel(e.vel as f32, GEN_VEL_CENTER, GEN_VEL_SPREAD / 2.0, window, 1.0);
        }
        for ev in &events[..n] {
            out.push(*ev);
        }
    }

    fn param_names(&self) -> &'static [&'static str] {
        &PARAM_NAMES
    }

    fn material_beats(&self, ctx: &BarCtx) -> Option<u32> {
        self.clip(ctx).map(|c| c.beats as u32)
    }

    fn motif_phrase(&self, seed: u32, lane: u8) -> Option<[u16; 4]> {
        if !self.uses_motifs() {
            return None;
        }
        let m = &self.motifs[motif::pick(&self.motifs, seed, lane, &[])];
        Some(std::array::from_fn(|b| m.onsets(b % m.bars as usize)))
    }

    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        self.defaults
    }
}

// MUSICAL: pitch walk (user, 2026-10-17: at the lowest spread the line moves
// at most a scale step; the top allows big jumps, most still small).
/// Largest step in scale degrees at spread 0 and 1.
const WALK_MAX: (f32, f32) = (1.0, 9.0);
/// Step size falloff (degrees per 1/e) at spread 0 and 1.
const WALK_TAU: (f32, f32) = (0.6, 2.4);
/// Weight of staying on the same note against a one-step move.
const WALK_STAY: f32 = 0.8;
/// The walk stays within these chord-relative degrees (a 6th under the
/// root.. the octave; the register floor may push it up further), turning
/// back at the edges.
const WALK_RANGE: (i32, i32) = (-2, 7);
/// Pull towards the root per degree above it, at spread 0 and 1 (user,
/// 2026-10-17: too many high notes).
const WALK_GRAVITY: (f32, f32) = (0.45, 0.22);

/// Tonal stability of the chord-relative degrees 0..6: the Krumhansl-Kessler
/// minor key profile at the natural-minor degrees, over the tonic's rating
/// (root, 3rd and 5th most stable).
const STABILITY: [f32; 7] = [1.0, 0.556, 0.85, 0.558, 0.75, 0.629, 0.528];
/// Preference of a move by its size in degrees: steps, then the consonant
/// leaps (4th, 5th, octave) well above 6ths, 7ths and 9ths.
const LEAP_PREF: [f32; 10] = [1.0, 1.0, 0.9, 1.0, 1.1, 0.35, 0.2, 0.9, 0.2, 0.3];
/// A leap is a move of at least this many degrees; after one, the next move
/// turns back by step (gap fill): moves on in the same direction and further
/// leaps are this much less likely.
const LEAP_MIN: i32 = 3;
const GAP_FILL_SAME_WAY: f32 = 0.25;
const GAP_FILL_LEAP: f32 = 0.3;

/// The next degree of the walk from `from` (`last` the previous move): up to
/// `max` degrees, small moves far more likely, leaps to consonant intervals
/// and stable tones (`tonal`: how much stability counts, per seed), a leap
/// followed by a step back, within the range. `fit` weighs each target
/// degree on top (Splice counterpoint; 1 = no say).
fn walk_step(from: i32, last: i32, h: u64, spread: f32, tonal: f32, fit: &dyn Fn(i32) -> f32) -> i32 {
    let spread = spread.clamp(0.0, 1.0);
    let max = lerp(WALK_MAX.0, WALK_MAX.1, spread * spread).round() as i32;
    let tau = lerp(WALK_TAU.0, WALK_TAU.1, spread);
    let after_leap = last.abs() >= LEAP_MIN;
    let gravity = lerp(WALK_GRAVITY.0, WALK_GRAVITY.1, spread);
    let mut weights = [0.0f32; 19];
    for (k, w) in weights.iter_mut().enumerate() {
        let d = k as i32 - 9;
        let to = from + d;
        if d.abs() > max || to < WALK_RANGE.0 || to > WALK_RANGE.1 {
            continue;
        }
        let size = d.unsigned_abs() as usize;
        let mut x = (-(size as f32) / tau).exp() * LEAP_PREF[size.min(9)] * if d == 0 { WALK_STAY } else { 1.0 };
        x *= STABILITY[to.rem_euclid(SCALE_LEN) as usize].powf(tonal);
        // Pull back towards the root: lines circle home, high notes are rare.
        x *= (-(to.max(0) as f32) * gravity).exp();
        x *= fit(to);
        if after_leap && d != 0 {
            if d.signum() == last.signum() {
                x *= GAP_FILL_SAME_WAY;
            }
            if d.abs() >= LEAP_MIN {
                x *= GAP_FILL_LEAP;
            }
        }
        *w = x;
    }
    match rng::weighted_pick(h, &weights) {
        Some(k) => from + k as i32 - 9,
        None => from,
    }
}

/// A seed's rhythmic character (per letter seed, the same in every bar).
struct Persona {
    /// Added to `complexity`: more or less syncopation.
    sync: f32,
    /// Slide probability multiplier.
    slide: f32,
    /// Chance per beat of half speed, per note of a ratchet.
    half: f32,
    ratchet: f32,
    /// How much tonal stability steers the walk (exponent on [`STABILITY`]).
    tonal: f32,
    /// How much the seed loops its long notes ([`loops`]).
    looping: f32,
    /// The seed reverses those notes instead of looping them.
    reverses: bool,
}

impl Persona {
    /// No character: the rhythm and slides as the parameters say.
    const PLAIN: Self = Self { sync: 0.0, slide: 1.0, half: 0.0, ratchet: 0.0, tonal: 1.0, looping: 0.65, reverses: false };

    fn new(ctx: &BarCtx) -> Self {
        let u = |k: u8| rng::unit_f32(rng::hash(ctx.seed, ctx.lane, 0, k, S_PERSONA));
        // Squared: most seeds stay plain, a few go far.
        Self {
            sync: lerp(PERSONA_SYNC.0, PERSONA_SYNC.1, u(0)),
            slide: lerp(PERSONA_SLIDE.0, PERSONA_SLIDE.1, u(1)),
            half: PERSONA_HALF_MAX * u(2) * u(2),
            ratchet: PERSONA_RATCHET_MAX * u(3) * u(3),
            tonal: lerp(PERSONA_TONAL.0, PERSONA_TONAL.1, u(4)),
            looping: lerp(PERSONA_LOOP.0, PERSONA_LOOP.1, u(5)),
            reverses: u(6) < PERSONA_REVERSE_SHARE,
        }
    }
}

// MUSICAL (user, 2026-10-17): repeats are mostly an 8th later, some a
// 16th, some syncopated (a dotted 8th), some a triplet; each seed weighs
// them its own way. High notes repeat much less.
/// Repeat offsets in ticks after the note, per figure: 8th, 16th, dotted
/// 8th, 8th triplet (two repeats).
const FIGURES: [&[u16]; 4] = [&[2 * TICKS_PER_STEP], &[TICKS_PER_STEP], &[3 * TICKS_PER_STEP], &[32, 64]];
/// Base weights of the figures, each scaled per seed by 0.4 .. 1.8.
const FIGURE_WEIGHTS: [f32; 4] = [0.5, 0.2, 0.15, 0.15];
const FIGURE_SEED_SCALE: (f32, f32) = (0.4, 1.8);
/// Repeats keep their chance up to this many semitones over the root, then
/// fade (halving about every this many more).
const REPEAT_HIGH_FROM: i32 = 5;
const REPEAT_HIGH_HALVING: f32 = 3.5;

// MUSICAL (user, 2026-10-17: rhythm and fast notes must follow a musical
// scheme, not chance). Bass figures on an 8th grid with a few 16th
// anticipations; levels: 255 the core (with the kick), ~200 the answer,
// ~150 the second answer, ~100 anticipations and pickups. `sync`: how
// syncopated the figure is (complexity picks near it).
/// One bass figure.
#[derive(Clone, Copy)]
struct Groove {
    levels: [u8; 16],
    sync: f32,
}

const fn groove(hits: &[(usize, u8)], sync: f32) -> Groove {
    let mut levels = [0u8; 16];
    let mut i = 0;
    while i < hits.len() {
        levels[hits[i].0] = hits[i].1;
        i += 1;
    }
    Groove { levels, sync }
}

const GROOVES: [Groove; 10] = [
    // Long root, answer on the "and" of 3.
    groove(&[(0, 255), (10, 200), (14, 110)], 0.1),
    // Half bar: 1 and 3, a pickup into the next bar.
    groove(&[(0, 255), (8, 210), (14, 120), (12, 90)], 0.0),
    // Dotted quarters.
    groove(&[(0, 255), (6, 215), (12, 190), (3, 90)], 0.4),
    // Two-step answer: the "a" of 2, then 3's "and".
    groove(&[(0, 255), (7, 200), (10, 170), (13, 100)], 0.6),
    // Rolling 8ths.
    groove(&[(0, 255), (6, 200), (10, 220), (14, 160), (4, 130), (12, 110)], 0.35),
    // Offbeat 8ths.
    groove(&[(2, 255), (6, 190), (10, 230), (14, 170), (0, 120)], 0.7),
    // Syncopated stab.
    groove(&[(0, 255), (3, 210), (6, 150), (11, 200), (14, 105)], 0.8),
    // Push into the backbeat.
    groove(&[(0, 255), (11, 210), (3, 140), (14, 120)], 0.55),
    // Call and answer across the bar.
    groove(&[(0, 255), (5, 180), (8, 210), (13, 140), (15, 90)], 0.5),
    // Off the one: anticipated downbeat.
    groove(&[(0, 230), (6, 255), (10, 190), (15, 140), (3, 100)], 0.9),
];
/// Level a step needs at density 0 and 1 (0: the core only; 0.5: the core
/// and its answer; 1: everything).
const GROOVE_REVEAL: (f32, f32) = (230.0, 70.0);
/// Level of the figure's empty 8ths, and how far a fully stabby line lowers
/// the reveal at full density (stabs leave gaps, so dense stabs fill them).
const GROOVE_FILL_LEVEL: u8 = 75;
/// How far the density lowers the reveal at full density on its own, and on
/// top for a fully stabby line (user, 2026-10-17: fuller).
const GROOVE_FILL_REACH: f32 = 30.0;
const GROOVE_STAB_REACH: f32 = 50.0;
/// Level of the figure's own hits repeated a beat or half a bar later (the
/// first fill a dense, stabby line takes).
const GROOVE_ECHO: f32 = 0.7;
/// Spread of the figure choice around the complexity.
const GROOVE_PICK_WIDTH: f32 = 0.25;
/// Fills: repeats from this step of the phrase's last bar, and their chance
/// boost there.
const RATCHET_FROM_STEP: u8 = 12;
const RATCHET_FILL_BOOST: f32 = 2.5;

/// A seeded figure, mostly ones whose syncopation is near `sync`.
fn pick_groove(h: u64, sync: f32) -> Groove {
    let mut weights = [0.0f32; GROOVES.len()];
    for (w, g) in weights.iter_mut().zip(&GROOVES) {
        *w = (-((g.sync - sync) / GROOVE_PICK_WIDTH).powi(2)).exp() + 0.02;
    }
    GROOVES[rng::weighted_pick(h, &weights).unwrap_or(0)]
}

// MUSICAL (user, 2026-10-17): on a mudpie every note of a letter seed
// starts at the seed's one position, anywhere in the sample; the Morph knob
// moves it within a region around that (the sampler). Per-note positions
// sounded random.
/// Start position of a note on a stream: the letter seed's position.
fn stream_region(ctx: &BarCtx, _step: u8) -> u16 {
    (rng::unit_f32(rng::hash(ctx.seed, ctx.lane, 0, 0, S_REGION)) * u16::MAX as f32) as u16
}

/// Chance factor of repeating a note `above` semitones over the root.
fn repeat_height(above: i32) -> f32 {
    if above <= REPEAT_HIGH_FROM { 1.0 } else { 0.5f32.powf((above - REPEAT_HIGH_FROM) as f32 / REPEAT_HIGH_HALVING) }
}

/// Repeat some notes (ratchets): where `pick(step, key)` says so, a note
/// plays again by the figure `figure(step)` ([`FIGURES`]) when the next note
/// is later than the figure's last repeat. Notes in a slide stay whole.
/// Returns the new count.
// Rhythm cells (user, 2026-10-20): the step line lives on one grid, so a
// seeded share of its beats is replaced by a cell from another one:
// ratchet clusters, triplets, dotted notes, sustains into a pickup. The
// pattern seed picks one family of cells for the whole pattern (a style);
// the filler letters (C, D) play a contrasting family. Variation sets how
// many beats a cell takes.

/// A rhythmic figure over `beats` beats.
#[derive(Debug, Clone, Copy)]
struct Cell {
    beats: u16,
    /// Onsets in ticks from the span start.
    onsets: &'static [u16],
    /// Gate as a share of the time to the next onset.
    gate: f32,
    /// The last note holds to the span end.
    hold_last: bool,
    /// Every note repeats the first pitch (a ratchet), else the span's
    /// pitches cycle.
    repeat: bool,
    /// The last note is an accented pickup.
    pickup: bool,
}

const BEAT: u16 = TICKS_PER_BAR / 4;

// MUSICAL: first draft, tune by ear.
const CELLS: [Cell; 12] = [
    // 0: four 16ths, a ratchet.
    Cell { beats: 1, onsets: &[0, 24, 48, 72], gate: 0.55, hold_last: false, repeat: true, pickup: false },
    // 1: three 32nds into a held note.
    Cell { beats: 1, onsets: &[0, 12, 24], gate: 0.6, hold_last: true, repeat: true, pickup: false },
    // 2: 8th triplets.
    Cell { beats: 1, onsets: &[0, 32, 64], gate: 0.85, hold_last: false, repeat: false, pickup: false },
    // 3: 16th triplets, a fast roll.
    Cell { beats: 1, onsets: &[0, 16, 32, 48, 64, 80], gate: 0.6, hold_last: false, repeat: true, pickup: false },
    // 4: quarter triplets over two beats.
    Cell { beats: 2, onsets: &[0, 64, 128], gate: 0.92, hold_last: false, repeat: false, pickup: false },
    // 5: dotted 8th and a 16th.
    Cell { beats: 1, onsets: &[0, 72], gate: 0.95, hold_last: false, repeat: false, pickup: true },
    // 6: dotted quarter and an 8th.
    Cell { beats: 2, onsets: &[0, 144], gate: 0.95, hold_last: false, repeat: false, pickup: false },
    // 7: a held beat into a 16th pickup.
    Cell { beats: 1, onsets: &[0, 72], gate: 0.97, hold_last: false, repeat: false, pickup: true },
    // 8: two held beats into a 16th pickup.
    Cell { beats: 2, onsets: &[0, 168], gate: 0.97, hold_last: false, repeat: false, pickup: true },
    // 9: one note held over two beats.
    Cell { beats: 2, onsets: &[0], gate: 0.97, hold_last: true, repeat: false, pickup: false },
    // 10: two quarters.
    Cell { beats: 2, onsets: &[0, 96], gate: 0.8, hold_last: false, repeat: false, pickup: false },
    // 11: four 16th triplets into a held note.
    Cell { beats: 1, onsets: &[0, 16, 32, 48], gate: 0.6, hold_last: true, repeat: true, pickup: false },
];

/// Families of cells (weights over [`CELLS`]); family 0 plays the step line
/// as it is.
const FAMILIES: [[f32; 12]; 7] = [
    // Plain.
    [0.0; 12],
    // Straight 16ths: ratchets and pickups.
    [2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 1.0, 1.0, 0.0],
    // Triplets.
    [0.0, 0.0, 3.0, 1.0, 2.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 2.0],
    // Dotted.
    [0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 2.0, 2.0, 1.0, 0.0, 0.0, 0.0],
    // Ratchet clusters.
    [2.0, 3.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
    // Sustained, with pickups.
    [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 3.0, 3.0, 1.0, 0.0],
    // Mixed.
    [1.0; 12],
];
/// Chance of each family for a pattern (plain sometimes, not always).
const FAMILY_WEIGHTS: [f32; 7] = [1.2, 1.0, 1.0, 1.0, 1.0, 1.0, 0.6];
/// Share of a bar's beats a cell takes at the centred Variation, and how
/// Variation scales it (0 .. 1).
const CELL_RATE: f32 = 0.35;
const CELL_RATE_BY_VARIATION: (f32, f32) = (0.2, 1.8);
const CELL_RATE_MAX: f32 = 0.85;

const S_FAMILY: u32 = 0x220;
const S_CELL_AT: u32 = 0x221;
const S_CELL_KIND: u32 = 0x222;

/// The cell family of this bar: the pattern's own in main letters, a
/// different one (by the letter) in filler letters.
fn cell_family(ctx: &BarCtx) -> usize {
    let main = rng::weighted_pick(rng::hash(ctx.style_seed, ctx.lane, 0, 0, S_FAMILY), &FAMILY_WEIGHTS).unwrap_or(0);
    if ctx.class == crate::form::SeedClass::Main {
        return main;
    }
    // Any other family but plain.
    let others = FAMILIES.len() as u32 - 2;
    let k = 1 + rng::below(rng::hash(ctx.seed, ctx.lane, 0, 1, S_FAMILY), others) as usize;
    if k >= main.max(1) { k + 1 } else { k }
}

/// Replace a seeded share of the bar's beats with cells of the bar's family
/// (see [`CELLS`]). A beat without a note stays a rest. The repeating bars of
/// a phrase choose alike; the variation bar re-draws.
fn rhythm_cells(events: &mut [LiveEvent; 2 * STEPS_PER_BAR], n: usize, ctx: &BarCtx, rng: &PhraseRng, variation: f32) -> usize {
    let family = cell_family(ctx);
    let weights = &FAMILIES[family.min(FAMILIES.len() - 1)];
    if weights.iter().all(|&w| w <= 0.0) || n == 0 {
        return n;
    }
    let rate = (CELL_RATE * lerp(CELL_RATE_BY_VARIATION.0, CELL_RATE_BY_VARIATION.1, variation)).min(CELL_RATE_MAX);
    let mut out = [LiveEvent::default(); 2 * STEPS_PER_BAR];
    let mut m = 0usize;
    let mut i = 0usize;
    let mut beat = 0u16;
    while beat < 4 {
        let from = beat * BEAT;
        let step = (from / TICKS_PER_STEP) as u8;
        let in_beat = |t: u16, beats: u16| t >= from && t < from + beats * BEAT;
        let cell = rng.chance(step, S_CELL_AT, rate).then(|| rng::weighted_pick(rng.at(step, S_CELL_KIND), weights)).flatten().map(|k| CELLS[k]);
        let cell = cell.filter(|c| beat + c.beats <= 4);
        // The notes the span holds.
        let span = cell.map_or(1, |c| c.beats);
        let mut j = i;
        while j < n && in_beat(events[j].tick, span) {
            j += 1;
        }
        let notes = &events[i..j];
        match cell {
            Some(c) if !notes.is_empty() && m + c.onsets.len() + (n - j) <= out.len() => {
                let end = from + c.beats * BEAT;
                let template = notes[0];
                for (k, &off) in c.onsets.iter().enumerate() {
                    let mut e = template;
                    e.tick = from + off;
                    let next = c.onsets.get(k + 1).map_or(end, |&o| from + o);
                    let last = k + 1 == c.onsets.len();
                    e.gate = if last && c.hold_last {
                        ((end - e.tick) as f32 * 0.97) as u16
                    } else {
                        (((next - e.tick) as f32 * c.gate) as u16).max(GATE_MIN_TICKS as u16)
                    };
                    if !c.repeat {
                        e.key = notes[k % notes.len()].key;
                    }
                    // A cell sets its own lengths: no stab cuts them short,
                    // and a figure keeps them under the sustained Shape.
                    e.flags &= !flags::STAB;
                    if c.onsets.len() > 1 {
                        e.flags |= flags::FIGURE;
                    }
                    if k > 0 {
                        e.flags &= !(flags::TIE | flags::ACCENT | flags::STAB);
                        e.glide = 0;
                        e.note_id = 0x80 + beat * 16 + k as u16;
                    }
                    if last && c.pickup && k > 0 {
                        e.flags |= flags::ACCENT;
                    }
                    out[m] = e;
                    m += 1;
                }
            }
            _ => {
                for e in notes {
                    out[m] = *e;
                    m += 1;
                }
            }
        }
        i = j;
        beat += span;
    }
    // Anything after the last beat (none in a normal bar).
    while i < n && m < out.len() {
        out[m] = events[i];
        m += 1;
        i += 1;
    }
    *events = out;
    m
}

// MUSICAL: first draft, tune by ear.
/// Share of on-beat notes pushed off the beat at syncopation 1.
const SYNC_MAX: f32 = 0.9;
/// How a pushed note moves, in ticks (− = early): a 16th early (an
/// anticipation), an 8th early, a 16th late.
const SYNC_MOVES: [(i32, f32); 3] = [(-24, 0.5), (-48, 0.25), (24, 0.25)];
const S_SYNC: u32 = 0x223;
const S_SYNC_MOVE: u32 = 0x224;

/// Syncopation (user, 2026-10-20; the Swing knob): a seeded share of the
/// notes on the 8th grid moves off it, mostly ahead of the beat, holding to
/// where it would have ended. The bar's first note stays (the anchor). The
/// repeating bars of a phrase move alike; the bar stays sorted.
fn syncopate(events: &mut [LiveEvent], amount: f32, rng: &PhraseRng) {
    if amount <= 0.0 {
        return;
    }
    let weights = SYNC_MOVES.map(|(_, w)| w);
    for i in 1..events.len() {
        let e = events[i];
        if e.kind != EventKind::NoteOn || e.tick % (2 * TICKS_PER_STEP) != 0 {
            continue;
        }
        let step = (e.tick / TICKS_PER_STEP) as u8;
        if !rng.chance(step, S_SYNC, amount * SYNC_MAX) {
            continue;
        }
        let prev = events[i - 1].tick as i32;
        let next = events.get(i + 1).map_or(TICKS_PER_BAR as i32, |x| x.tick as i32);
        // The seeded move, else the next one that fits between the neighbours.
        let first = rng::weighted_pick(rng.at(step, S_SYNC_MOVE), &weights).unwrap_or(0);
        let fits = |s: i32| {
            let to = e.tick as i32 + s;
            to > prev && to < next && to < TICKS_PER_BAR as i32
        };
        let Some(shift) = (0..SYNC_MOVES.len()).map(|k| SYNC_MOVES[(first + k) % SYNC_MOVES.len()].0).find(|&s| fits(s)) else { continue };
        let to = e.tick as i32 + shift;
        let ev = &mut events[i];
        ev.tick = to as u16;
        ev.gate = (ev.gate as i32 - shift).clamp(GATE_MIN_TICKS as i32, (next - to).max(GATE_MIN_TICKS as i32)) as u16;
        // A pushed note lands as an accent.
        ev.flags |= flags::ACCENT;
        // The note before it ends in time.
        let p = &mut events[i - 1];
        if p.tick as i32 + p.gate as i32 > to && p.flags & flags::TIE == 0 {
            p.gate = (to - p.tick as i32).max(GATE_MIN_TICKS as i32) as u16;
        }
    }
}

fn ratchets(
    events: &mut [LiveEvent; 2 * STEPS_PER_BAR],
    n: usize,
    pick: impl Fn(u8, u8) -> bool,
    figure: impl Fn(u8) -> usize,
) -> usize {
    let mut out = [LiveEvent::default(); 2 * STEPS_PER_BAR];
    let mut m = 0;
    for i in 0..n {
        let e = events[i];
        let tied = e.flags & flags::TIE != 0 || events[..n].get(i + 1).is_some_and(|x| x.flags & flags::TIE != 0);
        let step = (e.tick / TICKS_PER_STEP) as u8;
        let next = events[..n].get(i + 1).map_or(TICKS_PER_BAR, |x| x.tick);
        let offsets = FIGURES[figure(step).min(FIGURES.len() - 1)];
        let last = *offsets.last().unwrap_or(&0);
        let gap = offsets[0];
        let room = next > e.tick + last + gap / 2 && e.tick + last < TICKS_PER_BAR;
        if tied || !room || !pick(step, e.key) || m + 1 + offsets.len() > out.len() {
            if m < out.len() {
                out[m] = e;
                m += 1;
            }
            continue;
        }
        let mut a = e;
        a.gate = a.gate.min(gap.saturating_sub(2).max(1));
        a.flags |= flags::FIGURE;
        out[m] = a;
        m += 1;
        for (k, &off) in offsets.iter().enumerate() {
            let mut b = e;
            b.tick = e.tick + off;
            b.gate = e.gate.min(gap);
            b.note_id = e.note_id + (STEPS_PER_BAR * (k + 1)) as u16;
            b.flags &= !flags::ACCENT;
            b.flags |= flags::FIGURE;
            out[m] = b;
            m += 1;
        }
    }
    *events = out;
    m
}

/// The Length a note reads: its own move (step and height) around the
/// lane's, fading out towards full length so long lines stay held.
// MUSICAL: first draft, tune by ear (user: loops are part of the seed like
// ratchets and triplets, Variation sets how often, only long notes loop;
// 2026-10-20: the short loops sounded like a machine gun, a quarter triplet
// at the least, a beat at the most, as played at the Rate's half speed
// centre, which doubles them).
/// Shortest note (ticks) that can loop: two rounds of the shortest section
/// after the earliest start.
const LOOP_MIN_GATE: u16 = LOOP_STARTS[0] as u16 + 2 * LOOP_LENGTHS[0].0 as u16;
/// Share of the long notes that loop at full Variation on the loopiest seed.
const LOOP_SHARE_MAX: f32 = 0.7;
/// A seed's loop tendency range.
const PERSONA_LOOP: (f32, f32) = (0.3, 1.0);
// MUSICAL: tune by ear (user, 2026-10-20: reversed segments besides the
// loops, one or the other per seed).
/// Share of the seeds that reverse their long notes instead of looping them.
const PERSONA_REVERSE_SHARE: f32 = 0.4;
/// Loop lengths in rendered ticks (an 8th triplet, a dotted 16th, an 8th;
/// played at Normal Rate a quarter triplet, a dotted 8th, a quarter),
/// shortest first, and their weights.
const LOOP_LENGTHS: [(u8, f32); 3] = [(32, 1.0), (36, 0.8), (48, 1.2)];
/// Where the section starts after the note (ticks; at least a 32nd, so the
/// seam can fade in from before it).
const LOOP_STARTS: [u8; 4] = [12, 24, 48, 96];
/// Share of the loops that play back and forth.
const PING_PONG_SHARE: f32 = 0.35;
const S_LOOP: u32 = 0x225;
const S_LOOP_LEN: u32 = 0x226;
const S_LOOP_AT: u32 = 0x227;
const S_LOOP_PONG: u32 = 0x228;

/// Loop some long notes (D43): each draws, per seed and step, whether it
/// loops (`share`), the section's length and start so that it fits at
/// least two rounds into the note, and whether it plays back and forth. A
/// `reverse` seed plays those notes backwards instead (their own stretch of
/// the sample, end to start).
fn loops(events: &mut [LiveEvent], share: f32, reverse: bool, rng: &PhraseRng) {
    if share <= 0.0 {
        return;
    }
    let chance = LOOP_SHARE_MAX * share.clamp(0.0, 1.0).powf(1.5);
    for ev in events.iter_mut().filter(|e| e.kind == EventKind::NoteOn && e.gate >= LOOP_MIN_GATE && !e.has(flags::TIE)) {
        let step = (ev.tick / TICKS_PER_STEP) as u8;
        if !rng.chance(step, S_LOOP, chance) {
            continue;
        }
        if reverse {
            ev.flags |= flags::REVERSE;
            continue;
        }
        let gate = ev.gate;
        let fits = |len: u8, at: u8| at as u16 + 2 * len as u16 <= gate;
        let weights: [f32; LOOP_LENGTHS.len()] = std::array::from_fn(|k| if fits(LOOP_LENGTHS[k].0, LOOP_STARTS[0]) { LOOP_LENGTHS[k].1 } else { 0.0 });
        let Some(k) = rng::weighted_pick(rng.at(step, S_LOOP_LEN), &weights) else { continue };
        let len = LOOP_LENGTHS[k].0;
        let starts = LOOP_STARTS.iter().filter(|&&a| fits(len, a)).count().max(1);
        ev.loop_len = len;
        ev.loop_at = LOOP_STARTS[(rng.at(step, S_LOOP_AT) % starts as u64) as usize];
        if rng.chance(step, S_LOOP_PONG, PING_PONG_SHARE) {
            ev.flags |= flags::PING_PONG;
        }
    }
}

fn note_length(ctx: &BarCtx, ev: &LiveEvent) -> f32 {
    let shape = NoteShape { step: ev.step, gate: None, pitch_class: Some((ev.key as i32 - ctx.harmony.key.root as i32).rem_euclid(12) as u8) };
    held_length(param(ctx, P_LENGTH), note_param_for(ctx, P_LENGTH, shape))
}

/// A note's Length from the lane's `base` and its `moved` value: the move
/// fades out over the top quarter, none at full length.
pub fn held_length(base: f32, moved: f32) -> f32 {
    base + (moved - base) * (4.0 * (1.0 - base)).clamp(0.0, 1.0)
}

/// A gate at a Length below the middle: towards a stab.
fn shortened(gate: u16, length: f32) -> u16 {
    let stab = gate.min(STAB_TICKS);
    stab + ((gate - stab) as f32 * length.clamp(0.0, 0.5) * 2.0) as u16
}

/// `length` and `expression` (0.5 = unchanged). Length below 0.5 shortens
/// notes towards stabs, above 0.5 holds them towards the next onset, from
/// [`LEGATO_FROM`] a rising share of held notes ties into it and glides. Expression scales each note's
/// filter offset (0 = none, 1 = double).
fn shape_notes(events: &mut [LiveEvent], length_of: impl Fn(&LiveEvent) -> f32, expression: f32, slide: f32, roll: impl Fn(u8) -> f32) {
    let glide = slide_glide(slide);
    // Slide above its middle ties more held notes (liquid).
    let liquid = LEGATO_SLIDE * (2.0 * slide - 1.0).max(0.0);
    if expression != 0.5 {
        for ev in events.iter_mut() {
            ev.cutoff = (ev.cutoff as f32 * expression * 2.0).round().clamp(-127.0, 127.0) as i8;
        }
    }
    for i in 0..events.len() {
        let length = length_of(&events[i]);
        if length == 0.5 {
            continue;
        }
        let next = events.get(i + 1).map_or(TICKS_PER_BAR, |e| e.tick);
        // A slid-from note already holds into the next one.
        if events.get(i + 1).is_some_and(|e| e.flags & flags::TIE != 0) {
            continue;
        }
        let has_next = i + 1 < events.len();
        let into_figure = events.get(i + 1).is_some_and(|e| e.flags & flags::FIGURE != 0);
        let ev = &mut events[i];
        let gap = next.saturating_sub(ev.tick);
        if ev.flags & flags::FIGURE != 0 {
            // A ratchet or a cell plays as written.
            continue;
        }
        if length >= 0.5 && into_figure {
            // Held up to the figure, which strikes on its own.
            let to = gap.saturating_sub(2);
            if to > ev.gate {
                ev.gate += ((to - ev.gate) as f32 * (length - 0.5) * 2.0) as u16;
            }
            continue;
        }
        if length < 0.5 {
            ev.gate = shortened(ev.gate, length);
        } else if has_next && ev.flags & flags::STAB == 0 && roll((ev.tick / TICKS_PER_STEP) as u8) < legato_share(length) + liquid {
            ev.gate = gap + TIE_OVERLAP_TICKS;
            let next = &mut events[i + 1];
            next.flags |= flags::TIE;
            next.glide = glide;
        } else if gap > ev.gate {
            ev.gate += ((gap - ev.gate) as f32 * (length - 0.5) * 2.0) as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::generator::test_util::{assert_well_formed, ctx, notes, render};
    use crate::harmony::scale_for;
    use crate::TICKS_PER_BAR;

    /// Two-step backbone: kicks on steps 0 and 10, snares on 4 and 12.
    const TWO_STEP: [u8; 16] = [120, 0, 0, 0, 80, 0, 0, 0, 0, 0, 118, 0, 80, 0, 0, 0];
    const EMPTY: [u8; 16] = [0; 16];

    /// The plain generator (no seed character, pool pitches), for the rule tests.
    /// The step line alone: the cells have their own tests.
    fn test_rules() -> BassRules {
        BassRules { cells: false, ..BassRules::default() }
    }

    fn generator() -> BassGenerator {
        BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, walk: false, scheme: false, ..test_rules() }, Vec::new())
    }

    fn params(edit: impl FnOnce(&mut [f32; MAX_GEN_PARAMS])) -> [f32; MAX_GEN_PARAMS] {
        let mut p = PARAM_DEFAULTS;
        edit(&mut p);
        p
    }

    fn bar(g: &BassGenerator, seed: u32, bar: u8, p: &[f32; MAX_GEN_PARAMS], mask: &[u8; 16]) -> Vec<LiveEvent> {
        let mut c = ctx(seed, bar, p, mask);
        c.lane = 1;
        // Stream positions vary on every note by design; the musical
        // content is everything else.
        render(g, &c).as_slice().iter().map(|e| LiveEvent { start: 0, ..*e }).collect()
    }

    #[test]
    fn variation_loops_or_reverses_some_long_notes_and_only_long_ones() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new());
        let count = |variation: f32| {
            let p = params(|p| {
                p[P_VARIATION] = variation;
                p[P_LENGTH] = 1.0;
                p[P_STABS] = 0.0;
            });
            let (mut looped, mut pong, mut reversed) = (0, 0, 0);
            for seed in 0..40 {
                let mut kinds = [false; 2];
                for b in 0..8 {
                    for e in bar(&g, seed, b, &p, &EMPTY).iter().filter(|e| e.kind == EventKind::NoteOn) {
                        let rev = e.has(flags::REVERSE);
                        if e.loop_len > 0 {
                            looped += 1;
                            pong += e.has(flags::PING_PONG) as u32;
                            assert!(!rev && e.gate >= LOOP_MIN_GATE && e.loop_at >= 12 && e.loop_at as u16 + 2 * e.loop_len as u16 <= e.gate, "{e:?}");
                            kinds[0] = true;
                        } else if rev {
                            reversed += 1;
                            assert!(e.gate >= LOOP_MIN_GATE, "{e:?}");
                            kinds[1] = true;
                        }
                    }
                }
                assert!(!(kinds[0] && kinds[1]), "seed {seed} loops or reverses, not both");
            }
            (looped, pong, reversed)
        };
        assert_eq!(count(0.0), (0, 0, 0), "no loops or reverses without Variation");
        let (looped, pong, reversed) = count(1.0);
        assert!(looped > 10 && pong > 0 && pong < looped && reversed > 5, "{looped} loops, {pong} back and forth, {reversed} reversed");
        assert!(count(0.5).0 < looped);
    }

    fn steps(ev: &[LiveEvent]) -> Vec<usize> {
        ev.iter().map(|e| (e.tick / TICKS_PER_STEP) as usize).collect()
    }

    #[test]
    fn deterministic_and_well_formed() {
        let g = generator();
        for seed in 0..60u32 {
            for b in 0..4u8 {
                let p = params(|p| {
                    p[P_DENSITY] = (seed % 5) as f32 / 4.0;
                    p[P_COMPLEXITY] = (seed % 4) as f32 / 3.0;
                    p[P_KICK_MODE] = (seed % 3) as f32 / 2.0;
                    p[P_REGISTER] = (seed % 3) as f32 / 2.0;
                    p[P_SLIDE] = 1.0;
                });
                let mut c = ctx(seed, b, &p, &TWO_STEP);
                c.lane = 1;
                let one = render(&g, &c);
                assert_eq!(one.as_slice(), render(&g, &c).as_slice());
                assert_well_formed(&one);
                for e in one.as_slice() {
                    assert_eq!(e.kind, EventKind::NoteOn);
                    assert!(e.gate > 0 && e.note_id > 0);
                }
                // One note per step at most, ids unique.
                let mut ids: Vec<u16> = one.as_slice().iter().map(|e| e.note_id).collect();
                ids.dedup();
                assert_eq!(ids.len(), one.len());
            }
        }
    }

    #[test]
    fn notes_are_in_scale_in_a_bass_register() {
        let g = generator();
        for root in 0..12u8 {
            for minor in [true, false] {
                let key = MusicalKey::new(root, minor);
                for chord in 0..7u8 {
                    let p = params(|p| p[P_COMPLEXITY] = 1.0);
                    let mut c = ctx(root as u32 * 7 + chord as u32, 3, &p, &TWO_STEP);
                    c.lane = 1;
                    c.harmony.key = key;
                    c.harmony.chord_degree = chord;
                    for e in render(&g, &c).as_slice() {
                        let pc = (e.key as i32 - root as i32).rem_euclid(12) as u8;
                        assert!(scale_for(key).contains(&pc), "{} not in {key}", e.key);
                        assert!((24..=28 + 11 + BASS_SPAN as u8 + 19).contains(&e.key), "key {} out of register", e.key);
                    }
                }
            }
        }
    }

    #[test]
    fn register_shifts_by_octaves() {
        let g = generator();
        let at = |r: f32| bar(&g, 5, 0, &params(|p| p[P_REGISTER] = r), &TWO_STEP);
        let (low, mid, high) = (at(0.0), at(0.5), at(1.0));
        assert_eq!(steps(&low), steps(&mid));
        for ((l, m), h) in low.iter().zip(&mid).zip(&high) {
            assert_eq!(m.key - l.key, 12);
            assert_eq!(h.key - m.key, 12);
        }
    }

    #[test]
    fn root_heavy() {
        let g = generator();
        let (mut root, mut total) = (0, 0);
        for seed in 0..200 {
            for e in bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP) {
                total += 1;
                // A minor, chord on the root: pitch class A.
                root += (e.key % 12 == 9) as usize;
            }
        }
        let share = root as f32 / total as f32;
        assert!(total > 400);
        assert!(share > 0.55, "root share {share}");
        // The first note of every bar is the root.
        for seed in 0..50 {
            assert_eq!(bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP)[0].key % 12, 9);
        }
    }

    #[test]
    fn spread_sets_the_odd_block_jump_size() {
        let g = generator();
        // Largest distance above the bar's lowest note, over many seeds.
        let widest = |spread: f32| {
            (0..200)
                .map(|seed| {
                    let p = params(|p| {
                        p[P_SPREAD] = spread;
                        p[P_DENSITY] = 0.8;
                    });
                    let keys: Vec<i32> = bar(&g, seed, 0, &p, &TWO_STEP).iter().map(|e| e.key as i32).collect();
                    keys.iter().max().unwrap_or(&0) - keys.iter().min().unwrap_or(&0)
                })
                .max()
                .unwrap()
        };
        assert!(widest(0.0) <= 22, "no jumps: around the root, {}", widest(0.0));
        assert!(widest(1.0) >= 19, "full spread reaches +19: {}", widest(1.0));
        // Spread moves notes off the root more often (above and below).
        let off_root = |spread: f32| -> usize {
            let p = params(|p| p[P_SPREAD] = spread);
            (0..100).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).filter(|e| e.key % 12 != 9).count()
        };
        assert!(off_root(0.0) * 2 < off_root(1.0), "{} vs {}", off_root(0.0), off_root(1.0));
        let below = (0..300)
            .flat_map(|seed| bar(&g, seed, 0, &params(|p| p[P_SPREAD] = 1.0), &TWO_STEP))
            .filter(|e| (e.key as i32) < 33)
            .count();
        assert!(below > 20, "notes under the root (A1 = 33): {below}");
        // Jumps are blocks: most notes still sit within an octave of the root.
        let p = params(|p| p[P_SPREAD] = 1.0);
        let ev: Vec<LiveEvent> = (0..200).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
        let near = ev.iter().filter(|e| (e.key as i32 - 33).abs() <= 12).count();
        assert!(near * 10 > ev.len() * 7, "{near} of {}", ev.len());
    }

    #[test]
    fn root_rule_off_spreads_pitches() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { root: 0.0, walk: false, scheme: false, ..test_rules() }, Vec::new());
        let (mut root, mut total) = (0, 0);
        for seed in 0..200 {
            for e in bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP) {
                total += 1;
                root += (e.key % 12 == 9) as usize;
            }
        }
        assert!((root as f32 / total as f32) < 0.3);
    }

    #[test]
    fn follows_the_chord() {
        let g = generator();
        let mut c = ctx(11, 0, &PARAM_DEFAULTS, &TWO_STEP);
        c.lane = 1;
        let on_root = render(&g, &c);
        c.harmony.chord_degree = 5; // VI in A minor = F
        let on_six = render(&g, &c);
        assert_eq!(steps(on_root.as_slice()), steps(on_six.as_slice()));
        assert_eq!(on_root.as_slice()[0].key % 12, 9);
        assert_eq!(on_six.as_slice()[0].key % 12, 5);
    }

    #[test]
    fn key_change_keeps_the_rhythm() {
        let g = generator();
        for seed in 0..30 {
            let p = params(|p| p[P_SLIDE] = 1.0);
            let mut c = ctx(seed, 3, &p, &TWO_STEP);
            c.lane = 1;
            let am = render(&g, &c);
            c.harmony.key = MusicalKey::new(4, true); // E minor
            let em = render(&g, &c);
            assert_eq!(am.len(), em.len());
            let mut moved = false;
            for (a, e) in am.as_slice().iter().zip(em.as_slice()) {
                assert_eq!((a.tick, a.gate, a.vel, a.flags, a.note_id), (e.tick, e.gate, e.vel, e.flags, e.note_id));
                // Same degree a fourth lower or a fifth higher.
                let shift = (e.key as i32 - a.key as i32).rem_euclid(12);
                assert_eq!(shift, 7);
                moved |= a.key != e.key;
            }
            assert!(moved);
        }
    }

    #[test]
    fn kick_modes() {
        let g = generator();
        let kicks = [0usize, 10];
        for seed in 0..100 {
            for b in 0..4 {
                for density in [0.3, 0.6, 1.0] {
                    let avoid = params(|p| p[P_DENSITY] = density);
                    let s = steps(&bar(&g, seed, b, &avoid, &TWO_STEP));
                    assert!(kicks.iter().all(|k| !s.contains(k)), "co-onset in avoid mode (seed {seed})");
                    let double = params(|p| {
                        p[P_DENSITY] = density;
                        p[P_KICK_MODE] = 1.0;
                    });
                    let s = steps(&bar(&g, seed, b, &double, &TWO_STEP));
                    assert!(kicks.iter().all(|k| s.contains(k)), "kick not doubled (seed {seed})");
                }
            }
        }
        // Free mode: the downbeat is simply the most important step.
        let free = params(|p| p[P_KICK_MODE] = 0.5);
        assert!(steps(&bar(&g, 1, 0, &free, &TWO_STEP)).contains(&0));
    }

    #[test]
    fn prefers_drum_gaps() {
        // Snares on every odd step: with gap-following, notes move off them.
        let busy: [u8; 16] = std::array::from_fn(|s| if s % 2 == 1 { 80 } else { 0 });
        let count_on_odd = |g: &BassGenerator| {
            (0..200)
                .flat_map(|seed| steps(&bar(g, seed, 0, &params(|p| p[P_COMPLEXITY] = 1.0), &busy)))
                .filter(|s| s % 2 == 1)
                .count()
        };
        let follow = generator();
        let ignore = BassGenerator::new(PARAM_DEFAULTS, BassRules { gaps: 0.0, scheme: false, ..test_rules() }, Vec::new());
        assert!(count_on_odd(&follow) < count_on_odd(&ignore));
    }

    #[test]
    fn density_is_monotonic() {
        let g = generator();
        for seed in 0..30 {
            let mut prev: Vec<usize> = Vec::new();
            for d in 0..=10 {
                let now = steps(&bar(&g, seed, 0, &params(|p| p[P_DENSITY] = d as f32 / 10.0), &TWO_STEP));
                assert!(prev.iter().all(|s| now.contains(s)));
                prev = now;
            }
            assert!(prev.len() >= 8);
        }
        assert!(bar(&g, 1, 0, &params(|p| p[P_DENSITY] = 0.0), &TWO_STEP).is_empty());
    }

    #[test]
    fn phrase_shape() {
        let g = generator();
        for seed in 0..60 {
            for density in [0.2, 0.5, 1.0] {
                let p = params(|p| p[P_DENSITY] = density);
                let b0 = bar(&g, seed, 0, &p, &TWO_STEP);
                assert_eq!(b0, bar(&g, seed, 1, &p, &TWO_STEP));
                assert_eq!(b0, bar(&g, seed, 2, &p, &TWO_STEP));
                assert_ne!(b0, bar(&g, seed, 3, &p, &TWO_STEP), "seed {seed} density {density}");
                let still = params(|p| {
                    p[P_DENSITY] = density;
                    p[P_VARIATION] = 0.0;
                });
                assert_eq!(bar(&g, seed, 0, &still, &TWO_STEP), bar(&g, seed, 3, &still, &TWO_STEP));
            }
        }
    }

    #[test]
    fn spread_sizes_the_walk() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..test_rules() }, Vec::new());
        let intervals = |spread: f32| -> Vec<i32> {
            let p = params(|p| {
                p[P_SPREAD] = spread;
                p[P_DENSITY] = 0.7;
            });
            // Bar 0: not the variation bar, so no turnaround.
            (0..200)
                .flat_map(|seed| {
                    let ev = bar(&g, seed, 0, &p, &TWO_STEP);
                    ev.windows(2).map(|w| (w[1].key as i32 - w[0].key as i32).abs()).collect::<Vec<_>>()
                })
                .collect()
        };
        let low = intervals(0.0);
        assert!(low.iter().all(|&i| i <= 2), "at most a scale step: {:?}", low.iter().max());
        let high = intervals(1.0);
        assert!(high.iter().any(|&i| i >= 7), "big jumps exist");
        let small = high.iter().filter(|&&i| i <= 3).count() as f32 / high.len() as f32;
        assert!(small > 0.5, "most moves stay small: {small}");
    }

    #[test]
    fn leaps_favour_consonant_intervals_and_turn_back() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..test_rules() }, Vec::new());
        let p = params(|p| {
            p[P_SPREAD] = 1.0;
            p[P_DENSITY] = 0.8;
        });
        let (mut consonant, mut dissonant, mut turned, mut leaps) = (0, 0, 0, 0);
        for seed in 0..400 {
            let keys: Vec<i32> = bar(&g, seed, 0, &p, &TWO_STEP).iter().map(|e| e.key as i32).collect();
            for w in keys.windows(2) {
                match (w[1] - w[0]).abs() {
                    5 | 7 | 12 => consonant += 1,
                    6 | 8..=11 | 13..=16 => dissonant += 1,
                    _ => {}
                }
            }
            for w in keys.windows(3) {
                let (a, b) = (w[1] - w[0], w[2] - w[1]);
                if a.abs() >= 5 && b != 0 {
                    leaps += 1;
                    turned += (a.signum() != b.signum()) as usize;
                }
            }
        }
        assert!(consonant > dissonant, "{consonant} vs {dissonant}");
        assert!(turned * 10 > leaps * 6, "gap fill: {turned} of {leaps}");
    }

    #[test]
    fn high_notes_repeat_less() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { scheme: false, ..test_rules() }, Vec::new());
        let p = params(|p| {
            p[P_SPREAD] = 1.0;
            p[P_DENSITY] = 0.6;
        });
        // Repeat rate of notes low (within a 4th of the root A1 = 33) and high (an octave up).
        let (mut low, mut low_rep, mut high, mut high_rep) = (0, 0, 0, 0);
        for seed in 0..600 {
            for e in bar(&g, seed, 0, &p, &TWO_STEP) {
                let repeat = e.note_id > STEPS_PER_BAR as u16;
                match e.key as i32 - 33 {
                    0..=5 => {
                        low += 1;
                        low_rep += repeat as usize;
                    }
                    11..=20 => {
                        high += 1;
                        high_rep += repeat as usize;
                    }
                    _ => {}
                }
            }
        }
        let (lr, hr) = (low_rep as f32 / low as f32, high_rep as f32 / high.max(1) as f32);
        assert!(hr < lr * 0.5, "low {lr} high {hr}");
    }

    #[test]
    fn grooves_follow_the_vocabulary_and_fill_only_at_phrase_ends() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..test_rules() }, Vec::new());
        let count = |density: f32| -> f32 {
            let p = params(|p| p[P_DENSITY] = density);
            (0..200).map(|seed| bar(&g, seed, 0, &p, &EMPTY).len()).sum::<usize>() as f32 / 200.0
        };
        let (low, mid, high) = (count(0.02), count(0.5), count(1.0));
        assert!(low >= 0.9 && low <= 1.6 && (3.0..5.5).contains(&mid) && high > mid && high <= 10.0, "{low} {mid} {high}");
        // Dense and stabby: the gaps fill sooner (at full density everything
        // plays either way).
        let stabby = |stabs: f32| -> f32 {
            let p = params(|p| {
                p[P_DENSITY] = 0.75;
                p[P_STABS] = stabs;
            });
            (0..200).map(|seed| bar(&g, seed, 0, &p, &EMPTY).len()).sum::<usize>() as f32 / 200.0
        };
        assert!(stabby(1.0) > stabby(0.5) + 1.0, "{} vs {}", stabby(1.0), stabby(0.5));
        // Every onset is a step of the figure vocabulary; most on the 8th grid.
        let p = params(|p| p[P_DENSITY] = 0.7);
        let ev: Vec<LiveEvent> = (0..200).flat_map(|seed| bar(&g, seed, 0, &p, &EMPTY)).collect();
        let on_8ths = ev.iter().filter(|e| (e.tick / TICKS_PER_STEP) % 2 == 0).count();
        assert!(on_8ths * 10 > ev.len() * 7, "{on_8ths} of {}", ev.len());
        // Repeats only as fills: none in bar 0, some at the phrase's end.
        let stab = params(|p| {
            p[P_STABS] = 1.0;
            p[P_DENSITY] = 0.8;
        });
        let g = BassGenerator::new(PARAM_DEFAULTS, test_rules(), Vec::new());
        let reps = |bar_in: u8| (0..300).flat_map(|seed| bar(&g, seed, bar_in, &stab, &EMPTY)).filter(|e| e.note_id > STEPS_PER_BAR as u16).count();
        assert_eq!(reps(0), 0);
        assert!(reps(VARIATION_BAR) > 0);
    }

    #[test]
    fn stabs_set_the_share_of_stab_notes() {
        let g = generator();
        let share = |stabs: f32| {
            let p = params(|p| p[P_STABS] = stabs);
            let ev: Vec<LiveEvent> = (0..100).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
            ev.iter().filter(|e| e.flags & flags::STAB != 0).count() as f32 / ev.len() as f32
        };
        let (lo, mid, hi) = (share(0.0), share(0.5), share(1.0));
        assert!(lo < 0.15 && hi > 0.85 && lo < mid && mid < hi, "{lo} {mid} {hi}");
    }

    #[test]
    fn seeds_carry_their_own_rhythmic_character() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { scheme: false, ..test_rules() }, Vec::new());
        let p = params(|p| p[P_DENSITY] = 0.7);
        let (mut ratchets, mut half_beats, mut slides) = (0, 0, Vec::new());
        let mut figures_seen = false;
        for seed in 0..300 {
            let ev = bar(&g, seed, 0, &p, &EMPTY);
            ratchets += ev.iter().filter(|e| e.note_id > STEPS_PER_BAR as u16).count();
            figures_seen |= ev.iter().filter(|e| e.tick % TICKS_PER_STEP != 0).count() > 0;
            for beat in 0..4u16 {
                let n = ev.iter().filter(|e| e.tick / (4 * TICKS_PER_STEP) == beat).count();
                half_beats += (n == 1) as usize;
            }
            slides.push(ev.iter().filter(|e| e.flags & flags::TIE != 0).count());
            // Ratchets keep the bar well formed.
            for w in ev.windows(2) {
                assert!(w[0].tick < w[1].tick && w[0].tick + w[0].gate <= w[1].tick + 2 || w[1].flags & flags::TIE != 0);
            }
        }
        assert!(ratchets > 20, "{ratchets}");
        assert!(figures_seen, "triplet repeats on some seeds");
        assert!(half_beats > 50, "{half_beats}");
        // Some seeds slide a lot, many barely.
        assert!(slides.iter().max().unwrap() >= &3 && slides.iter().filter(|&&s| s == 0).count() > 50);
    }

    #[test]
    fn length_and_expression_shape_the_notes() {
        let g = generator();
        let mean_gate = |length: f32| -> f32 {
            let p = params(|p| p[P_LENGTH] = length);
            let ev: Vec<LiveEvent> = (0..50).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
            ev.iter().map(|e| e.gate as f32).sum::<f32>() / ev.len() as f32
        };
        assert!(mean_gate(0.0) < mean_gate(0.5) && mean_gate(0.5) < mean_gate(0.8), "{} {} {}", mean_gate(0.0), mean_gate(0.5), mean_gate(0.8));
        // Legato: every held note holds into the next one, which glides
        // (stabs stay stabs).
        let legato = params(|p| p[P_LENGTH] = 1.0);
        for seed in 0..50 {
            let ev = bar(&g, seed, 0, &legato, &TWO_STEP);
            for w in ev.windows(2).filter(|w| w[0].flags & flags::STAB == 0) {
                assert!(w[0].tick + w[0].gate > w[1].tick && w[1].flags & flags::TIE != 0 && w[1].glide > 0);
            }
        }
        // Part way up, part of the held notes tie.
        let some = params(|p| p[P_LENGTH] = 0.875);
        let (mut tied, mut held) = (0, 0);
        for seed in 0..100 {
            let ev = bar(&g, seed, 0, &some, &TWO_STEP);
            for w in ev.windows(2).filter(|w| w[0].flags & flags::STAB == 0) {
                held += 1;
                tied += (w[1].flags & flags::TIE != 0) as usize;
            }
        }
        let share = tied as f32 / held as f32;
        assert!((0.35..0.75).contains(&share), "{share}");
        // Expression scales the filter offsets; 0.5 changes nothing.
        let flat = params(|p| p[P_EXPRESSION] = 0.0);
        let double = params(|p| p[P_EXPRESSION] = 1.0);
        for seed in 0..20 {
            let base = bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP);
            assert!(bar(&g, seed, 0, &flat, &TWO_STEP).iter().all(|e| e.cutoff == 0));
            for (a, b) in base.iter().zip(bar(&g, seed, 0, &double, &TWO_STEP)) {
                assert_eq!((a.cutoff as i32 * 2).clamp(-127, 127), b.cutoff as i32);
            }
        }
    }

    #[test]
    fn slides_tie_into_the_next_note() {
        let g = generator();
        let p = params(|p| {
            p[P_SLIDE] = 1.0;
            p[P_DENSITY] = 0.8;
        });
        let mut ties = 0;
        for seed in 0..100 {
            let ev = bar(&g, seed, 0, &p, &EMPTY);
            for w in ev.windows(2) {
                if w[1].flags & flags::TIE != 0 {
                    ties += 1;
                    assert!(w[1].glide > 0);
                    assert_ne!(w[0].key, w[1].key);
                    // The previous note is still held when the tie arrives.
                    assert!(w[0].tick + w[0].gate > w[1].tick || w[0].tick + w[0].gate == TICKS_PER_BAR);
                }
            }
            assert!(ev[0].flags & flags::TIE == 0);
        }
        assert!(ties > 20);
        let none = params(|p| p[P_SLIDE] = 0.0);
        assert!((0..50).all(|seed| bar(&g, seed, 0, &none, &EMPTY).iter().all(|e| e.flags & flags::TIE == 0)));
    }

    #[test]
    fn accents_and_note_length_from_density() {
        let g = generator();
        let (mut accents, mut total) = (0, 0);
        for seed in 0..200 {
            for e in bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP) {
                total += 1;
                if e.flags & flags::ACCENT != 0 {
                    accents += 1;
                    assert!(e.vel > 105);
                }
            }
        }
        let share = accents as f32 / total as f32;
        // Accents mostly on the strong steps, rarer than before (user,
        // 2026-10-21: the velocity far less random).
        assert!((0.08..0.3).contains(&share), "accent share {share}");
        // Sparse lines hold their notes longer than dense ones (per note).
        let mean_gate = |d: f32| -> f32 {
            let p = params(|p| {
                p[P_DENSITY] = d;
                p[P_SLIDE] = 0.0;
            });
            let ev: Vec<LiveEvent> = (0..40).flat_map(|seed| bar(&g, seed, 0, &p, &TWO_STEP)).collect();
            ev.iter().map(|e| e.gate as f32).sum::<f32>() / ev.len() as f32
        };
        assert!(mean_gate(0.2) > mean_gate(0.5) && mean_gate(0.5) > mean_gate(0.9), "{} {} {}", mean_gate(0.2), mean_gate(0.5), mean_gate(0.9));
    }

    #[test]
    fn seeded_filter_moves_per_note() {
        let g = generator();
        let span = (FILTER_HALF_OCTAVES / CUTOFF_OFFSET_OCTAVES * 127.0).round() as i8 + 1;
        let mut distinct = std::collections::BTreeSet::new();
        for seed in 0..40 {
            let ev = bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP);
            assert!(ev.iter().all(|e| e.cutoff.abs() <= span));
            distinct.extend(ev.iter().map(|e| e.cutoff));
            // The motif repeats bar to bar.
            let again: Vec<i8> = bar(&g, seed, 1, &PARAM_DEFAULTS, &TWO_STEP).iter().map(|e| e.cutoff).collect();
            assert_eq!(again, ev.iter().map(|e| e.cutoff).collect::<Vec<_>>());
        }
        assert!(distinct.len() > 20, "cutoffs vary by seed and note: {distinct:?}");
        // Denser lines sit brighter on average.
        let mean = |d: f32| {
            let ev: Vec<LiveEvent> = (0..40).flat_map(|s| bar(&g, s, 0, &params(|p| p[P_DENSITY] = d), &TWO_STEP)).collect();
            ev.iter().map(|e| e.cutoff as f32).sum::<f32>() / ev.len() as f32
        };
        assert!(mean(0.9) > mean(0.1) + 10.0);
    }

    #[test]
    fn idiom_cell_sets_the_rhythm() {
        let mut levels = [0u8; 16];
        for (s, l) in [(0, 255), (3, 200), (6, 150), (11, 90)] {
            levels[s] = l;
        }
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, walk: false, scheme: false, ..test_rules() }, vec![LevelMap::bar(levels)]);
        let free = |d: f32| {
            params(|p| {
                p[P_DENSITY] = d;
                p[P_KICK_MODE] = 0.5;
                p[P_COMPLEXITY] = 0.0;
                p[P_VARIATION] = 0.0;
            })
        };
        // Density reveals the cell's steps by level; the floor
        // (RHYTHM_DENSITY) keeps a sparse bass grooving.
        assert_eq!(steps(&bar(&g, 1, 0, &free(0.9), &EMPTY)), [0, 3, 6, 11]);
        let mid = steps(&bar(&g, 1, 0, &free(0.5), &EMPTY));
        assert!(mid.len() >= 2 && mid[..2] == [0, 3], "{mid:?}");
        let low = steps(&bar(&g, 1, 0, &free(0.05), &EMPTY));
        assert!(!low.is_empty() && low.len() <= mid.len() && low[0] == 0, "{low:?}");
        // Never outside the cell, even at full density and complexity.
        let wild = params(|p| {
            p[P_DENSITY] = 1.0;
            p[P_COMPLEXITY] = 1.0;
            p[P_KICK_MODE] = 0.5;
            p[P_VARIATION] = 0.0;
        });
        for seed in 0..20 {
            assert_eq!(steps(&bar(&g, seed, 0, &wild, &EMPTY)), [0, 3, 6, 11]);
        }
    }

    // ── Motif mode ──

    fn motif_generator(names: &[&str]) -> BassGenerator {
        let motifs = crate::generator::test_util::starter_motifs().into_iter().filter(|m| names.contains(&m.name.as_str())).collect();
        BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new()).with_motifs(motifs)
    }

    #[test]
    fn motif_mode_plays_the_clip_unchanged_bar_to_bar() {
        let g = motif_generator(&["bass-332-bounce"]);
        let still = params(|p| p[P_VARIATION] = 0.0);
        for seed in 0..20 {
            let mut c = ctx(seed, 0, &still, &TWO_STEP);
            c.lane = 1;
            // Per-note jitter may move accent, slide and filter, never pitch or rhythm.
            c.jitter = [0.4; MAX_GEN_PARAMS];
            let first: Vec<(u16, u8)> = render(&g, &c).as_slice().iter().map(|e| (e.tick, e.key)).collect();
            assert_eq!(steps_of(&first), [0, 3, 6, 8, 11, 14]);
            assert_eq!(first[0].1 % 12, 9, "root first");
            for b in 1..4 {
                c.bar_in_phrase = b;
                let now: Vec<(u16, u8)> = render(&g, &c).as_slice().iter().map(|e| (e.tick, e.key)).collect();
                assert_eq!(now, first, "seed {seed} bar {b}");
            }
        }
    }

    fn steps_of(ev: &[(u16, u8)]) -> Vec<usize> {
        ev.iter().map(|e| (e.0 / TICKS_PER_STEP) as usize).collect()
    }

    #[test]
    fn motif_mode_sets_cutoff_and_stream_start_and_follows_the_chord() {
        let g = motif_generator(&["bass-roller-walk", "bass-stab-hold", "bass-syncopated"]);
        let mut cutoffs = std::collections::BTreeSet::new();
        for seed in 0..30 {
            let mut c = ctx(seed, 0, &PARAM_DEFAULTS, &TWO_STEP);
            c.lane = 1;
            let on_root = render(&g, &c);
            assert_well_formed(&on_root);
            assert!(!on_root.is_empty());
            cutoffs.extend(on_root.as_slice().iter().map(|e| e.cutoff));
            assert!(on_root.as_slice().iter().all(|e| e.kind == EventKind::NoteOn && e.attack > 0));
            c.harmony.chord_degree = 5;
            let on_six = render(&g, &c);
            assert_eq!(on_root.len(), on_six.len());
            assert_eq!(on_six.as_slice()[0].key % 12, 5, "F under chord VI");
        }
        assert!(cutoffs.len() > 5, "the filter still moves per note: {cutoffs:?}");
        let p = g.motif_phrase(3, 1).unwrap();
        assert!(p.iter().all(|&m| m != 0));
        assert!(BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new()).motif_phrase(3, 1).is_none());
    }

    #[test]
    fn motifs_can_be_switched_off_by_the_rules() {
        let motifs = crate::generator::test_util::starter_motifs();
        let off = BassGenerator::new(PARAM_DEFAULTS, BassRules { motifs: false, persona: false, walk: false, scheme: false, ..test_rules() }, Vec::new()).with_motifs(motifs);
        assert!(off.motif_phrase(1, 1).is_none());
        assert_eq!(bar(&off, 1, 0, &PARAM_DEFAULTS, &TWO_STEP), bar(&generator(), 1, 0, &PARAM_DEFAULTS, &TWO_STEP));
    }

    /// A bar of the full generator (cells on) for pattern seed `style`.
    fn cell_bar(style: u32, class: crate::form::SeedClass, bar_in: u8, variation: f32) -> Vec<LiveEvent> {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules::default(), Vec::new());
        let p = params(|p| {
            p[P_DENSITY] = 0.7;
            p[P_VARIATION] = variation;
        });
        let mut c = ctx(style.wrapping_mul(31), bar_in, &p, &EMPTY);
        c.lane = 1;
        c.style_seed = style;
        c.class = class;
        render(&g, &c).as_slice().to_vec()
    }

    #[test]
    fn seeds_play_triplets_dotted_notes_ratchets_and_sustains() {
        use crate::form::SeedClass::Main;
        let (mut triplet, mut dotted, mut fast, mut held, mut plain) = (0, 0, 0, 0, 0);
        for style in 0..100 {
            let ev: Vec<LiveEvent> = (0..4).flat_map(|b| cell_bar(style, Main, b, 0.5)).collect();
            assert!(ev.len() <= 4 * 2 * STEPS_PER_BAR && ev.windows(2).all(|w| w[0].tick <= w[1].tick || w[1].tick < 96));
            let has = |f: &dyn Fn(&LiveEvent) -> bool| ev.iter().any(f);
            triplet += has(&|e| e.tick % 32 == 0 && e.tick % TICKS_PER_STEP != 0) as u32;
            dotted += has(&|e| e.tick % BEAT == 72 && e.flags & flags::ACCENT != 0) as u32;
            fast += has(&|e| e.tick % 12 == 0 && e.tick % TICKS_PER_STEP != 0 || e.tick % 16 == 0 && e.tick % 32 != 0) as u32;
            held += has(&|e| e.gate >= BEAT + BEAT / 2) as u32;
            plain += (cell_family(&ctx_for(style, Main)) == 0) as u32;
        }
        // Each kind in a good share of seeds, the plain step line sometimes.
        for (name, k) in [("triplet", triplet), ("dotted", dotted), ("fast", fast), ("held", held)] {
            assert!(k >= 12, "{name}: {k} of 100");
        }
        assert!((5..40).contains(&plain), "plain: {plain}");
    }

    fn ctx_for(style: u32, class: crate::form::SeedClass) -> BarCtx<'static> {
        static P: [f32; MAX_GEN_PARAMS] = PARAM_DEFAULTS;
        let mut c = ctx(style.wrapping_mul(31), 0, &P, &EMPTY);
        c.lane = 1;
        c.style_seed = style;
        c.class = class;
        c
    }

    #[test]
    fn slide_sets_the_glide_time_up_to_a_dotted_16th() {
        let secs = |s: f32| crate::sampler::glide_secs(slide_glide(s));
        assert!((secs(0.0) - SLIDE_SECS.0).abs() < 0.003, "{}", secs(0.0));
        assert!((secs(1.0) - SLIDE_SECS.1).abs() < 0.01, "{}", secs(1.0));
        assert!(secs(0.5) > secs(0.0) && secs(1.0) > secs(0.5));
        // The line's slides carry it.
        let g = generator();
        let p = params(|p| {
            p[P_SLIDE] = 1.0;
            p[P_DENSITY] = 0.8;
        });
        let glides: Vec<u8> = (0..100).flat_map(|seed| bar(&g, seed, 0, &p, &EMPTY)).filter(|e| e.flags & flags::TIE != 0).map(|e| e.glide).collect();
        assert!(!glides.is_empty() && glides.iter().all(|&v| crate::sampler::glide_secs(v) > 0.15), "{glides:?}");
    }

    #[test]
    fn syncopation_moves_notes_off_the_beat() {
        let g = generator();
        let off_beat = |sync: f32| {
            let p = params(|p| {
                p[P_DENSITY] = 0.7;
                p[P_SYNCOPATION] = sync;
            });
            let mut off = 0;
            for seed in 0..200 {
                let ev = bar(&g, seed, 0, &p, &EMPTY);
                assert!(ev.windows(2).all(|w| w[0].tick < w[1].tick), "sorted");
                assert_eq!(ev.first().map(|e| e.tick), bar(&g, seed, 0, &params(|p| p[P_DENSITY] = 0.7), &EMPTY).first().map(|e| e.tick), "the anchor stays");
                off += ev.iter().filter(|e| e.tick % (2 * TICKS_PER_STEP) != 0).count();
            }
            off
        };
        let (none, some, full) = (off_beat(0.0), off_beat(0.5), off_beat(1.0));
        assert!(some > none + none / 10 && full > some + some / 10, "{none} {some} {full}");
    }

    #[test]
    fn letters_keep_the_family_and_fillers_contrast() {
        use crate::form::SeedClass::{Filler, Main};
        for style in 0..60 {
            let main = cell_family(&ctx_for(style, Main));
            let mut other = ctx_for(style, Main);
            other.seed ^= 0xABCD;
            assert_eq!(cell_family(&other), main, "one family across the main letters");
            let mut c = ctx_for(style, Filler);
            let fills: std::collections::BTreeSet<usize> = (0..8u32)
                .map(|k| {
                    c.seed = k.wrapping_mul(977);
                    cell_family(&c)
                })
                .collect();
            assert!(!fills.contains(&main) && !fills.contains(&0), "style {style}: {main} vs {fills:?}");
            assert!(fills.len() > 1, "filler letters differ");
        }
    }

    #[test]
    fn variation_sets_how_much_the_cells_take() {
        use crate::form::SeedClass::Main;
        let off_grid = |v: f32| {
            (0..100)
                .flat_map(|s| cell_bar(s, Main, 0, v))
                .filter(|e| e.tick % TICKS_PER_STEP != 0 || e.gate >= 2 * BEAT - 12)
                .count()
        };
        let (low, high) = (off_grid(0.0), off_grid(1.0));
        assert!(high > 2 * low, "{low} {high}");
    }

    #[test]
    fn golden_bars() {
        // Pinned output: a change here changes how saved seeds sound.
        let g = generator();
        let got: Vec<String> = [1u32, 2, 3].iter().map(|&s| notes(&bar(&g, s, 0, &PARAM_DEFAULTS, &TWO_STEP))).collect();
        assert_eq!(got, GOLDEN, "\n{got:#?}");
    }

    const GOLDEN: [&str; 3] = [
        // 2026-10-17: the lower density floor (about 4 notes a bar in the middle).
        "6:33 8:33 14:33",
        "6:33 8:43 14:40",
        "6:33 8:33 11:33 14:33",
    ];

    #[test]
    fn helpers() {
        assert_eq!(root_octave(MusicalKey::new(9, true), BASS_ROOT_MIN), 1); // A1 = 33
        assert_eq!(root_octave(MusicalKey::new(4, true), BASS_ROOT_MIN), 1); // E1 = 28
        assert_eq!(root_octave(MusicalKey::new(3, true), BASS_ROOT_MIN), 2); // D#2 = 39
        assert_eq!([register_shift(0.0), register_shift(0.5), register_shift(1.0)], [-1, 0, 1]);
        assert_eq!(KickMode::from_param(0.0), KickMode::Avoid);
        assert_eq!(KickMode::from_param(0.5), KickMode::Free);
        assert_eq!(KickMode::from_param(1.0), KickMode::Double);
    }

    /// A clip set holding the table clip `c` for every letter.
    fn one_clip(c: &crate::clips::tables::ClipTable) -> Arc<ClipSet> {
        let bytes = crate::motif::midi::write(&c.clip(), c.name);
        let clip = BassClip::from_midi(c.name, &bytes).unwrap();
        Arc::new(ClipSet { letters: std::array::from_fn(|_| vec![clip.clone()]) })
    }

    /// Clip mode with the per-note moves off.
    fn clip_params(edit: impl FnOnce(&mut [f32; MAX_GEN_PARAMS])) -> [f32; MAX_GEN_PARAMS] {
        params(|p| {
            p[P_SOURCE] = 1.0;
            p[P_VARIATION] = 0.0;
            p[P_SPREAD] = 0.0;
            p[P_ACCENT] = 0.0;
            p[P_SLIDE] = 0.0;
            edit(p);
        })
    }

    fn clip_events(g: &BassGenerator, p: &[f32; MAX_GEN_PARAMS], material_bar: u16, harmony: crate::harmony::HarmonyCtx) -> Vec<LiveEvent> {
        let mask = [0u8; 16];
        let mut c = ctx(5, (material_bar % 4) as u8, p, &mask);
        c.lane = 1;
        c.role = crate::role::Role::Bass;
        c.material_bar = material_bar;
        c.harmony = harmony;
        render(g, &c).as_slice().to_vec()
    }

    #[test]
    fn clip_mode_plays_the_clip_as_written_at_the_middle() {
        let table = &crate::clips::tables::A[0];
        let clips = one_clip(table);
        let g = generator().with_clips(clips.clone());
        let clip = &clips.letters[0][0];
        let p = clip_params(|_| {});
        let key = crate::generator::test_util::a_minor();
        for b in 0..8u16 {
            let got = clip_events(&g, &p, b, key);
            let lo = b as u32 * TICKS_PER_BAR as u32;
            let written: Vec<&crate::clips::ClipNote> = clip.notes.iter().filter(|n| n.tick >= lo && n.tick < lo + TICKS_PER_BAR as u32).collect();
            assert_eq!(got.iter().map(|e| e.tick as u32 + lo).collect::<Vec<_>>(), written.iter().map(|n| n.tick).collect::<Vec<_>>(), "bar {b}: the clip's rhythm");
            for (e, n) in got.iter().zip(&written) {
                assert_eq!(e.gate as u32, n.len.min(lo + TICKS_PER_BAR as u32 - n.tick), "bar {b}: as long as written");
                // The root on the key's (A), the rest as written over it.
                assert_eq!((e.key as i32 - got[0].key as i32), n.semis as i32 - written[0].semis as i32, "bar {b}: the intervals stay");
            }
            assert_eq!(got[0].key % 12, 9, "bar {b} opens on the key's root");
        }
        // The source param picks the generated line.
        let generated = clip_events(&g, &clip_params(|p| p[P_SOURCE] = 0.0), 0, key);
        assert_ne!(generated.iter().map(|e| (e.tick, e.key)).collect::<Vec<_>>(), clip_events(&g, &p, 0, key).iter().map(|e| (e.tick, e.key)).collect::<Vec<_>>());
        let mask = [0u8; 16];
        let mut c = ctx(5, 0, &p, &mask);
        c.lane = 1;
        assert_eq!(g.material_beats(&c), Some(32));
        let off = clip_params(|p| p[P_SOURCE] = 0.0);
        let c = ctx(5, 0, &off, &mask);
        assert_eq!(g.material_beats(&c), None);
    }

    #[test]
    fn clip_notes_land_in_every_key_and_mode() {
        use crate::harmony::{HarmonyCtx, MAJOR_MODES, MINOR_MODES};
        for table in [&crate::clips::tables::D[0], &crate::clips::tables::D[2], &crate::clips::tables::D[9], &crate::clips::tables::C[0]] {
            let g = generator().with_clips(one_clip(table));
            let p = clip_params(|_| {});
            for root in 0..12u8 {
                for minor in [true, false] {
                    let modes = if minor { MINOR_MODES.len() } else { MAJOR_MODES.len() };
                    for mode in 0..modes as u8 {
                        let h = HarmonyCtx { key: MusicalKey::new(root, minor), chord_degree: 0, progression: 0, mode };
                        let scale = h.scale();
                        for b in 0..8 {
                            for e in clip_events(&g, &p, b, h) {
                                let pc = (e.key as i32 - root as i32).rem_euclid(12) as u8;
                                assert!(scale.contains(&pc), "{}: key {} not in {root} {minor} mode {mode}", table.name, e.key);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn density_thins_the_clip_and_splits_its_long_notes() {
        let g = generator().with_clips(one_clip(&crate::clips::tables::A[3]));
        let key = crate::generator::test_util::a_minor();
        let count = |d: f32| (0..8).map(|b| clip_events(&g, &clip_params(|p| p[P_DENSITY] = d), b, key).len()).sum::<usize>();
        let (low, mid, high) = (count(0.1), count(0.5), count(1.0));
        assert!(low < mid && mid < high, "thinned, as written, split: {low} {mid} {high}");
        // Thinned notes keep the beats.
        for b in 0..8 {
            for e in clip_events(&g, &clip_params(|p| p[P_DENSITY] = 0.0001), b, key).iter().skip(1) {
                assert_eq!(e.tick % (4 * TICKS_PER_STEP), 0, "on a beat: {}", e.tick);
            }
        }
        // Split notes repeat the written pitches.
        for b in 0..8 {
            let written: std::collections::BTreeSet<u8> = clip_events(&g, &clip_params(|_| {}), b, key).iter().map(|e| e.key).collect();
            let split: std::collections::BTreeSet<u8> = clip_events(&g, &clip_params(|p| p[P_DENSITY] = 1.0), b, key).iter().map(|e| e.key).collect();
            assert_eq!(written, split, "bar {b}");
        }
    }

    #[test]
    fn stabs_and_syncopation_reshape_the_clip() {
        let g = generator().with_clips(one_clip(&crate::clips::tables::B[1]));
        let key = crate::generator::test_util::a_minor();
        let gates = |p: [f32; MAX_GEN_PARAMS]| (0..8).flat_map(|b| clip_events(&g, &p, b, key)).map(|e| e.gate as u32).sum::<u32>();
        let mid = gates(clip_params(|_| {}));
        assert!(gates(clip_params(|p| p[P_STABS] = 1.0)) < mid, "stabs shorten held notes");
        assert!(gates(clip_params(|p| p[P_STABS] = 0.0)) > mid, "and hold the stabs");
        let ticks = |p: [f32; MAX_GEN_PARAMS]| (0..8).flat_map(|b| clip_events(&g, &p, b, key)).map(|e| e.tick).collect::<Vec<_>>();
        assert_ne!(ticks(clip_params(|p| p[P_SYNCOPATION] = 1.0)), ticks(clip_params(|_| {})), "syncopation pushes notes");
    }

    fn clip_events_seeded(g: &BassGenerator, p: &[f32; MAX_GEN_PARAMS], seed: u32, material_bar: u16) -> Vec<LiveEvent> {
        let mask = [0u8; 16];
        let mut c = ctx(seed, (material_bar % 4) as u8, p, &mask);
        c.lane = 1;
        c.role = crate::role::Role::Bass;
        c.material_bar = material_bar;
        c.harmony = crate::generator::test_util::a_minor();
        render(g, &c).as_slice().to_vec()
    }

    fn vel_range(events: &[LiveEvent]) -> u8 {
        let v = events.iter().filter(|e| e.vel > 0).map(|e| e.vel);
        v.clone().max().unwrap_or(0) - v.min().unwrap_or(0)
    }

    #[test]
    fn clip_velocities_follow_the_written_dynamics_inside_the_window() {
        let table = &crate::clips::tables::A[3];
        let clips = one_clip(table);
        let g = generator().with_clips(clips.clone());
        let clip = &clips.letters[0][0];
        let at = |accent: f32, variation: f32| clip_params(|p| {
            p[P_ACCENT] = accent;
            p[P_VARIATION] = variation;
        });
        for b in 0..8u16 {
            let got = clip_events_seeded(&g, &at(ACCENT_MID, 1.0), 5, b);
            let lo = b as u32 * TICKS_PER_BAR as u32;
            let written: Vec<u8> = clip.notes.iter().filter(|n| n.tick >= lo && n.tick < lo + TICKS_PER_BAR as u32).map(|n| n.vel).collect();
            assert_eq!(got.len(), written.len());
            // The written order of loud and soft, the same for every seed.
            for i in 0..got.len() {
                for j in 0..got.len() {
                    if written[i] > written[j] {
                        assert!(got[i].vel >= got[j].vel, "bar {b}: {:?} vs {written:?}", got.iter().map(|e| e.vel).collect::<Vec<_>>());
                    }
                }
            }
            assert_eq!(got.iter().map(|e| e.vel).collect::<Vec<_>>(), clip_events_seeded(&g, &at(ACCENT_MID, 1.0), 77, b).iter().map(|e| e.vel).collect::<Vec<_>>(), "no seed in the dynamics");
            assert!(vel_range(&got) as f32 <= VEL_WINDOW.1 + 1.0, "bar {b}: within the window");
            // Accent 0 plays them flat.
            assert!(vel_range(&clip_events_seeded(&g, &at(0.0, 1.0), 5, b)) <= 1);
            let wide = clip_events_seeded(&g, &at(1.0, 1.0), 5, b);
            assert_eq!(wide.iter().map(|e| e.vel).collect::<Vec<_>>(), got.iter().map(|e| e.vel).collect::<Vec<_>>(), "bar {b}: a full accent no wider than the window");
            // Variation 0 narrows the window.
            assert!(vel_range(&clip_events_seeded(&g, &at(ACCENT_MID, 0.0), 5, b)) as f32 <= VEL_WINDOW.0 + 1.0);
        }
        let range = |v: f32| (0..8).map(|b| vel_range(&clip_events_seeded(&g, &at(ACCENT_MID, v), 5, b))).max().unwrap();
        assert!(range(0.0) < range(1.0), "Variation widens the dynamics");
    }

    #[test]
    fn both_sources_keep_the_velocities_inside_the_window() {
        let key = crate::generator::test_util::a_minor();
        let g = generator().with_clips(ClipSet::builtin());
        for source in [0.0, 1.0] {
            let mut widest = [0u8; 2];
            for (k, variation) in [0.0f32, 1.0].into_iter().enumerate() {
                for seed in 0..40u32 {
                    let p = params(|p| {
                        p[P_SOURCE] = source;
                        p[P_VARIATION] = variation;
                        p[P_ACCENT] = 1.0;
                        p[P_DENSITY] = 0.8;
                    });
                    for b in 0..4u16 {
                        let mask = [0u8; 16];
                        let mut c = ctx(seed, b as u8, &p, &mask);
                        c.lane = 1;
                        c.role = crate::role::Role::Bass;
                        c.material_bar = b;
                        c.harmony = key;
                        let r = vel_range(render(&g, &c).as_slice());
                        assert!(r as f32 <= vel_window(variation) + 1.0, "source {source}, variation {variation}, seed {seed}: {r}");
                        widest[k] = widest[k].max(r);
                    }
                }
            }
            assert!(widest[0] < widest[1], "source {source}: Variation widens the dynamics {widest:?}");
        }
    }

    #[test]
    fn short_length_and_full_density_play_short_subdivided_hits() {
        // User, 2026-10-21: at low Shape and high Density most hits still
        // sounded held. A held clip at a short Length and full Density: most
        // 16ths sound and most notes are short.
        let key = crate::generator::test_util::a_minor();
        let g = generator().with_clips(one_clip(&crate::clips::tables::A[7]));
        let (mut on, mut short, mut all) = (0, 0, 0);
        for b in 0..8 {
            let mut seen = [false; 16];
            for e in clip_events(&g, &clip_params(|p| {
                p[P_DENSITY] = 1.0;
                p[P_LENGTH] = 0.15;
            }), b, key) {
                seen[(e.tick / TICKS_PER_STEP) as usize] = true;
                all += 1;
                short += (e.gate <= 2 * TICKS_PER_STEP) as u32;
            }
            on += seen.iter().filter(|&&x| x).count();
        }
        assert!(on as f32 / 128.0 >= 0.6, "most 16ths sound: {on} of 128");
        assert!(short * 4 >= all * 3, "most hits short: {short} of {all}");
    }

    #[test]
    fn full_density_fills_the_clip_and_follows_the_groove() {
        let key = crate::generator::test_util::a_minor();
        // A sparse clip: two held notes a bar.
        let g = generator().with_clips(one_clip(&crate::clips::tables::A[7]));
        let steps = |d: f32| {
            let mut on = 0;
            for b in 0..8 {
                let mut seen = [false; 16];
                for e in clip_events(&g, &clip_params(|p| p[P_DENSITY] = d), b, key) {
                    seen[(e.tick / TICKS_PER_STEP) as usize] = true;
                }
                on += seen.iter().filter(|&&x| x).count();
            }
            on as f32 / (8.0 * 16.0)
        };
        let (mid, full) = (steps(0.5), steps(1.0));
        assert!(full >= 0.6 && full > 3.0 * mid, "full Density sounds most 16ths: {mid} -> {full}");
        // Fills (in the rests of a half-empty clip) sit on the Groove's grid:
        // triplets at Groove 0, and not only there in its middle.
        let g = generator().with_clips(one_clip(&crate::clips::tables::C[2]));
        let fills = |groove: f32, variation: f32| -> Vec<LiveEvent> {
            (0..8).flat_map(|b| clip_events(&g, &clip_params(|p| {
                p[P_DENSITY] = 0.8;
                p[P_SYNCOPATION] = groove;
                p[P_VARIATION] = variation;
            }), b, key)).filter(|e| e.note_id >= 200).collect()
        };
        let triplet = fills(0.0, 0.0);
        assert!(triplet.len() >= 8, "the rests fill: {}", triplet.len());
        assert!(triplet.iter().all(|e| e.tick % 32 == 0), "on the triplet grid: {:?}", triplet.iter().map(|e| e.tick).collect::<Vec<_>>());
        assert!(fills(0.5, 0.0).iter().any(|e| e.tick % 32 != 0), "straight in the Groove's middle");
        // Low Variation repeats the note it fills from; high varies some.
        let keys = |variation: f32| fills(0.0, variation).iter().map(|e| e.key).collect::<std::collections::BTreeSet<_>>().len();
        assert!(keys(1.0) > keys(0.0), "Variation varies the fills: {} vs {}", keys(0.0), keys(1.0));
        assert_eq!(fills(0.0, 1.0).iter().map(|e| (e.tick, e.key)).collect::<Vec<_>>(), fills(0.0, 1.0).iter().map(|e| (e.tick, e.key)).collect::<Vec<_>>(), "deterministic");
    }
}
