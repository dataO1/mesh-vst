//! Bass generator (design §6.3 rule A6; research §7.1.2, §7.1.7), as the
//! bass lane's phases of the render chain (D57, [`crate::chain`]):
//!
//! - **R1 Skeleton** ([`LaneGenerator::render_bar`]): the rhythm only. The
//!   generated line takes a seeded figure of the bass vocabulary
//!   ([`GROOVES`], picked by Complexity), or the step importance (metric,
//!   raised on the steps the drums leave empty); Density reveals its notes.
//!   Kick steps are avoided, left alone or doubled (`kick_mode`). A clip
//!   (D50) plays its bar as written, its pitches fixed in the key and the
//!   chord, Density below its middle thinning its weak notes. Every note
//!   starts on the root with a base length to the next onset and its
//!   accent.
//! - **R5 Fill** ([`LaneGenerator::fill`]): Density's clusters
//!   ([`crate::chain::fill`]).
//! - **R6 Length** ([`LaneGenerator::lengths`]): stab or held by a chain
//!   over the notes (stabs cluster), a held note's start or end sometimes
//!   a stab, then Length: shorter towards stabs, longer towards the next
//!   note, tied into it towards the top.
//! - **R8 Melody** ([`LaneGenerator::melody`]): the pitches. A walk from
//!   the root (steps, consonant leaps, gravity home, the register), odd
//!   blocks by Spread, Splice's counterpoint against a pitched host; a
//!   recursion group walks a short line of its own from where its parent
//!   would have been. A clip's notes keep their pitches, Spread lifting
//!   weak ones an octave; its fills repeat the clip note before them.
//! - **R11 Expression** ([`LaneGenerator::expression`]): velocities inside
//!   Variation's window, slides, the note filter (Expression scales it),
//!   the attack by Shape, the stream position, the mudpie loops.
//!
//! Performance parameters, in order: `density`, `complexity`, `variation`,
//! `kick_mode` (< 1/3 avoid the kick, < 2/3 ignore it, else double it),
//! `register` (octave down / base / up), `slide`, `accent`, `spread`
//! (the walk's reach and the odd blocks' jumps: 0 = no jumps, then a 5th,
//! an octave, an octave and a 3rd, up to an octave and a 5th), `length`
//! (Shape), `expression`, `stabs` (0.5 = as the density says), `syncopation`
//! (the Groove knob: the chain's recursion and groove read it), `source`.
//!
//! Every draw follows the letter seed and the note's written step (D57: no
//! phrase rule, a letter plays alike in every bar).
//!
//! Construction options: `rule_a5` (velocity tiers), `rule_a6`
//! (gap-following and root weighting), `persona`, `walk`, `scheme`. All
//! default to 1.

use std::collections::BTreeMap;

use super::blocks::{
    lerp, metric_weight, note, note_param, note_param_for, option, param, reveals, shape_attack, step_motif, step_tick, velocity, NoteShape, PhraseRng, Tier,
};
use super::layout::MASK_KICK_MIN;
use super::{BarCtx, LaneGenerator, MAX_GEN_PARAMS};
use crate::clips::{self, BassClip, ClipSet};
use crate::event::{flags, origin, BarEvents, EventKind, LiveEvent};
use crate::harmony::SCALE_LEN;
use crate::key::MusicalKey;
use crate::rng;
use crate::sampler::CUTOFF_OFFSET_OCTAVES;
use crate::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};
use std::sync::Arc;

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
/// The Bass source (D50): below 0.5 the generated line, from 0.5 the clips.
const P_SOURCE: usize = 12;

// MUSICAL: first draft, tune by ear.
pub const PARAM_DEFAULTS: [f32; MAX_GEN_PARAMS] = [0.5, 0.4, 0.5, 0.0, 0.5, 0.5, 0.5, 0.3, 0.5, 0.5, 0.5, 0.0, 0.0];

const S_RHYTHM: u32 = 0x201;
const S_PITCH: u32 = 0x202;
const S_ACCENT: u32 = 0x203;
const S_SLIDE: u32 = 0x204;
const S_VEL: u32 = 0x205;
const S_LENGTH: u32 = 0x20B;
const S_PERSONA: u32 = 0x210;
const S_LEGATO: u32 = 0x213;
const S_GROOVE: u32 = 0x215;
const S_REGION: u32 = 0x216;
const S_PARTIAL: u32 = 0x217;
const S_GROUP: u32 = 0x218;
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
/// (user, 2026-10-17: at normal speed the middle plays about 4 notes a bar;
/// 2026-10-23, D57: half that pace is the centre, on the 16ths).
const RHYTHM_DENSITY: (f32, f32) = (0.15, 0.85);
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
// MUSICAL: first draft, tune by ear (user, 2026-10-17: each seed its own
// rhythmic character). Seed personality, drawn once per letter seed:
/// Syncopation offset added to `complexity`.
const PERSONA_SYNC: (f32, f32) = (-0.2, 0.35);
/// Slide probability multiplier.
const PERSONA_SLIDE: (f32, f32) = (0.3, 2.5);
/// How much tonal stability steers the pitch walk (so seeds differ: some
/// circle the chord tones, some wander the scale).
const PERSONA_TONAL: (f32, f32) = (0.3, 2.0);

// MUSICAL: tune by ear (user, 2026-10-23: stabs cluster, a lone stab is
// rarer; a held note's start or end may be a stab or two). The chance a
// stab follows a stab (at least the share), the most of the held notes
// that turn a part into stabs (at full stab share), their shortest length
// (ticks) and how many stabs a part takes at most.
const STAB_STICK: f32 = 0.75;
const PARTIAL_SHARE: f32 = 0.3;
const PARTIAL_MIN_GATE: u16 = 3 * TICKS_PER_STEP;
const PARTIAL_STABS: u16 = 2;

// MUSICAL: first draft, tune by ear (D50). Clip mode.
/// Metric weight a note needs at density 0 (the beats, 0.7, stay).
const CLIP_THIN_MAX: f32 = 0.7;
/// Weak notes (metric weight under this) may lift an octave, at this
/// chance at Spread 1.
const CLIP_LIFT_BELOW: f32 = 0.7;
const CLIP_LIFT_MAX: f32 = 0.45;
/// Written notes shorter than this are stabs.
const CLIP_SHORT_TICKS: u16 = 2 * TICKS_PER_STEP;
/// Written velocity from which a clip note is an accent.
const CLIP_ACCENT_VEL: u8 = 116;
// MUSICAL: tune by ear (user, 2026-10-21: Variation sets how varied the
// fills are). The chance at Variation 1 a clip's fill takes the octave or
// a scale neighbour of the clip note it repeats; the most of the fill
// runs that hold one note (at Stabs 0 and Shape 1; none at Stabs 1 or
// Shape 0).
const CLIP_FILL_ALT: f32 = 0.4;
const CLIP_FILL_HOLD: f32 = 0.5;
const S_CLIP_FILL_ALT: u32 = 0x234;
const S_CLIP_LIFT: u32 = 0x230;
const S_CLIP_STAB: u32 = 0x231;

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
const GEN_VEL_CENTER: f32 = (super::blocks::VEL_ACCENT as f32 + super::blocks::VEL_NORMAL as f32) / 2.0;
const GEN_VEL_SPREAD: f32 = (super::blocks::VEL_ACCENT - super::blocks::VEL_NORMAL) as f32;
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
    /// A5: velocity tier depth.
    pub tiers: f32,
    /// A6: strength of gap-following (rhythm).
    pub gaps: f32,
    /// A6: share of notes drawn from the root-heavy pool (the rest are any
    /// scale degree).
    pub root: f32,
    /// Each seed's rhythmic character ([`Persona`]): syncopation, slides,
    /// the walk's tonality, loops.
    pub persona: bool,
    /// Pitch as a walk from the previous note, its steps sized by `spread`
    /// ([`walk_step`]); off: each note drawn from the root-heavy pool.
    pub walk: bool,
    /// Rhythm from the built-in groove vocabulary ([`GROOVES`]); off: the
    /// step-importance rhythm.
    pub scheme: bool,
}

impl Default for BassRules {
    fn default() -> Self {
        Self { tiers: 1.0, gaps: 1.0, root: 1.0, persona: true, walk: true, scheme: true }
    }
}

impl BassRules {
    pub fn from_spec(params: &BTreeMap<String, f32>) -> Self {
        let a6 = option(params, "rule_a6", 1.0);
        Self {
            tiers: option(params, "rule_a5", 1.0),
            gaps: a6,
            root: a6,
            persona: option(params, "persona", 1.0) >= 0.5,
            walk: option(params, "walk", 1.0) >= 0.5,
            scheme: option(params, "scheme", 1.0) >= 0.5,
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
    /// The bass clips per letter (D50), played when the source is Clips.
    clips: Arc<ClipSet>,
}

impl BassGenerator {
    pub fn new(defaults: [f32; MAX_GEN_PARAMS], rules: BassRules) -> Self {
        Self { defaults, rules, clips: Arc::new(ClipSet::default()) }
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

    /// The octave the line plays in (the key's root at the register).
    fn octave(ctx: &BarCtx) -> i32 {
        root_octave(ctx.harmony.key, BASS_ROOT_MIN) + register_shift(param(ctx, P_REGISTER))
    }

    /// The home root of the bar's register (the line's first note).
    fn home(ctx: &BarCtx) -> i32 {
        ctx.harmony.resolve_in_key(0, Self::octave(ctx)) as i32
    }

    /// Clip mode's skeleton (D50): bar `ctx.material_bar` of the letter's
    /// clip as written, its root on the key's root and its notes in the
    /// key's scale, moved with the chord by scale steps, the pitches fixed.
    /// Density below its middle thins the weak notes (the bar's first note
    /// and the beats stay); a drum host's kicks thin it (Avoid) or double
    /// it with the root.
    fn clip_skeleton(&self, ctx: &BarCtx, clip: &BassClip, out: &mut BarEvents) {
        let bar_ticks = TICKS_PER_BAR as u32;
        let bars = clip.ticks().div_ceil(bar_ticks).max(1);
        let lo = (ctx.material_bar as u32 % bars) * bar_ticks;
        let hi = (lo + bar_ticks).min(clip.ticks());
        let variation = param(ctx, P_VARIATION);
        let home = Self::home(ctx);
        let mut fold = 0;
        while home + fold + clip.lowest() < MOTIF_MIN_KEY {
            fold += 12;
        }
        let to_key = |semis: i32, degree: i32| clip_key(ctx, home + fold, semis, degree);
        let is_kick = |s: usize| ctx.drum_mask[s.min(STEPS_PER_BAR - 1)] >= MASK_KICK_MIN;
        let mode = KickMode::from_param(param(ctx, P_KICK_MODE));
        // The written dynamics, fitted into the Variation's window around the
        // clip's mean; the accent sets how far they reach (no random accents).
        let window = vel_window(variation);
        let (lo_vel, hi_vel, sum) = clip.notes.iter().fold((127u8, 0u8, 0u32), |(l, h, s), n| (l.min(n.vel), h.max(n.vel), s + n.vel as u32));
        let center = sum as f32 / clip.notes.len().max(1) as f32;
        let half = ((hi_vel - lo_vel.min(hi_vel)) as f32 / 2.0).max(1.0);
        let depth = accent_depth(param(ctx, P_ACCENT));
        let mut first = true;
        for (n, cn) in clip.notes.iter().filter(|x| (lo..hi).contains(&x.tick)).enumerate() {
            let tick = (cn.tick - lo) as u16;
            let step = ((tick + TICKS_PER_STEP / 2) / TICKS_PER_STEP).min(STEPS_PER_BAR as u16 - 1) as u8;
            let weight = metric_weight(step as usize);
            let d = note_param(ctx, P_DENSITY, step);
            let need = CLIP_THIN_MAX * (1.0 - d / 0.5).max(0.0);
            if !first && weight < need {
                continue;
            }
            if !first && mode == KickMode::Avoid && is_kick(step as usize) && weight < 1.0 {
                continue;
            }
            let key = to_key(cn.semis as i32, cn.degree as i32).clamp(0, 127);
            let gate = cn.len.min(hi - cn.tick).min(u16::MAX as u32) as u16;
            let mut ev = note(tick, key as u8, fit_vel(cn.vel as f32, center, half, window, depth), gate.max(1), n as u16 + 1);
            ev.step = step;
            ev.origin = origin::SKELETON | origin::FIXED;
            if cn.vel >= CLIP_ACCENT_VEL {
                ev.flags |= flags::ACCENT;
            }
            if gate < CLIP_SHORT_TICKS {
                ev.flags |= flags::STAB;
            }
            out.push(ev);
            first = false;
        }
        // A drum host's kicks on empty steps double with the root.
        if mode == KickMode::Double {
            for s in (0..STEPS_PER_BAR).filter(|&s| is_kick(s)) {
                let t = (s as u16) * TICKS_PER_STEP;
                if !out.as_slice().iter().any(|e| e.tick <= t && e.tick + e.gate > t) {
                    let k = to_key(0, 0).clamp(0, 127);
                    let mut ev = note(t, k as u8, VEL_ACCENT_CLIP, TICKS_PER_STEP, 0x40 + s as u16);
                    ev.flags |= flags::STAB;
                    ev.origin = origin::SKELETON | origin::FIXED;
                    out.push(ev);
                }
            }
        }
        out.sort();
    }

    /// The generated skeleton: a seeded figure of the vocabulary (or the
    /// step importance), revealed by Density, every note on the root, held
    /// to the next one, its accent drawn by its place.
    fn generated_skeleton(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let density = param(ctx, P_DENSITY);
        let complexity = param(ctx, P_COMPLEXITY);
        let persona = if self.rules.persona { Persona::new(ctx) } else { Persona::PLAIN };
        let mode = KickMode::from_param(param(ctx, P_KICK_MODE));
        let rng = PhraseRng::new(ctx);
        // The built-in groove scheme (user, 2026-10-17): a seeded figure from
        // the bass vocabulary, picked by syncopation (complexity).
        let groove = self.rules.scheme.then(|| pick_groove(rng.base.at(0, S_GROOVE), (complexity + persona.sync).clamp(0.0, 1.0)));
        let is_kick = |s: usize| ctx.drum_mask[s] >= MASK_KICK_MIN;
        // Each step reads its own density (per-note variation).
        let step_density = |d: f32| lerp(RHYTHM_DENSITY.0, RHYTHM_DENSITY.1, d.powf(RHYTHM_CURVE));
        let thin = THIN_PENALTY * (1.0 - density / THIN_DENSITY_MAX).max(0.0);
        let mut onsets = 0u16;
        if let Some(gv) = groove {
            // Density reveals the figure's levels: the core first, then the
            // answers, the anticipations last. Stabs leave holes, so a
            // stabby, dense line also takes the figure's own hits again a
            // beat or half a bar later (by the seed) on the 8ths.
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
                // (The core plays at any Density above 0.)
                let threshold = ((lerp(GROOVE_REVEAL.0, GROOVE_REVEAL.1, d) - (GROOVE_FILL_REACH + GROOVE_STAB_REACH * stab_fill) * d) as i32).min(255);
                // The core plays with the kick; the rest leaves it room.
                let kick_clash = is_kick(s) && mode == KickMode::Avoid && level < 255;
                if level as i32 >= threshold && !kick_clash {
                    onsets |= 1 << s;
                }
            }
        } else {
            for s in 0..STEPS_PER_BAR {
                let kick = is_kick(s);
                if kick && mode == KickMode::Avoid {
                    continue;
                }
                let gap = if ctx.drum_mask[s] == 0 {
                    GAP_BONUS
                } else if kick {
                    0.0
                } else {
                    -ONSET_PENALTY
                };
                let cx = (note_param(ctx, P_COMPLEXITY, s as u8) + persona.sync).clamp(0.0, 1.0);
                let mut importance = lerp(BASS_SIMPLE[s], BASS_SYNC[s], cx) + self.rules.gaps * gap - if s % 2 == 1 { thin } else { 0.0 };
                if kick && mode == KickMode::Double {
                    importance = 1.0;
                }
                if importance > 0.0
                    && reveals(importance, step_density(note_param(ctx, P_DENSITY, s as u8)), complexity * BASS_CHAOS_MAX, rng.unit(s as u8, S_RHYTHM))
                {
                    onsets |= 1 << s;
                }
            }
        }
        let root = Self::home(ctx).clamp(0, 127) as u8;
        for s in (0..STEPS_PER_BAR).filter(|&s| onsets & (1 << s) != 0) {
            let su = s as u8;
            let next = (s + 1..STEPS_PER_BAR).find(|&t| onsets & (1 << t) != 0).unwrap_or(STEPS_PER_BAR);
            let gate = ((next - s) as u16 * TICKS_PER_STEP).max(GATE_MIN_TICKS as u16);
            // Accents mostly on the strong steps (user, 2026-10-21: less chance).
            let accented = rng.chance(su, S_ACCENT, note_param(ctx, P_ACCENT, su) * ACCENT_PROB_MAX * metric_weight(s));
            let tier = if accented { Tier::Accent } else { Tier::Normal };
            let mut ev = note(step_tick(s), root, velocity(tier, rng.unit(su, S_VEL), self.rules.tiers), gate, s as u16 + 1);
            ev.step = su;
            if accented {
                ev.flags |= flags::ACCENT;
            }
            out.push(ev);
        }
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

    /// A note's own draw on stream `stream`: its written step and its id, so
    /// two notes on one step (a recursion's 32nds) draw apart.
    fn draw(ctx: &BarCtx, e: &LiveEvent, stream: u32) -> u64 {
        rng::hash(ctx.seed, ctx.lane, e.step, (e.note_id & 0xFF) as u8 ^ (e.group << 5), stream)
    }

    /// The stab share a note at `step` reads: Density's balance, Stabs
    /// moving it (0.5 = as the density says), none at Stabs 0; Splice's
    /// lengths alike or against a pitched host's (the bar's first note
    /// stays as it is, an anchor).
    fn stab_share(ctx: &BarCtx, step: u8, first: bool) -> f32 {
        let share = lerp(SHORT_SHARE.0, SHORT_SHARE.1, note_param(ctx, P_DENSITY, step)) + STABS_SHIFT * (note_param(ctx, P_STABS, step) - 0.5);
        let share = share.clamp(STABS_RANGE.0, STABS_RANGE.1) * (2.0 * note_param(ctx, P_STABS, step)).min(1.0);
        if first { share } else { crate::splice::short_share(&ctx.host, step as usize % STEPS_PER_BAR, share) }
    }
}

/// The MIDI key of a clip note `semis` over the clip root on scale degree
/// `degree`, its root at `root`, in the bar's scale and moved with its
/// chord by scale steps (the nearer way).
fn clip_key(ctx: &BarCtx, root: i32, semis: i32, degree: i32) -> i32 {
    let scale = ctx.harmony.scale();
    let root_pc = ctx.harmony.key.root as i32;
    let chord = match ctx.harmony.chord_degree as i32 {
        d if d > 3 => d - 7,
        d => d,
    };
    let k = root + clips::snap(semis, degree, scale);
    if chord == 0 {
        return k;
    }
    let rel = k - root_pc;
    let idx = scale.iter().position(|&s| s as i32 == rel.rem_euclid(12)).unwrap_or(0) as i32 + chord;
    root_pc + 12 * (rel.div_euclid(12) + idx.div_euclid(7)) + scale[idx.rem_euclid(7) as usize] as i32
}

/// The key a scale step up or down from `key` in the bar's scale.
fn neighbour(ctx: &BarCtx, key: i32, up: bool) -> i32 {
    let scale = ctx.harmony.scale();
    let root_pc = ctx.harmony.key.root as i32;
    let rel = key - root_pc;
    let Some(i) = scale.iter().position(|&s| s as i32 == rel.rem_euclid(12)) else { return key };
    let j = i as i32 + if up { 1 } else { -1 };
    root_pc + 12 * (rel.div_euclid(12) + j.div_euclid(7)) + scale[j.rem_euclid(7) as usize] as i32
}

// MUSICAL: tune by ear. Lowest MIDI key a clip bar may reach (C1); lower
// bars move up an octave. The velocity of a kick-doubling clip note.
const MOTIF_MIN_KEY: i32 = 24;
const VEL_ACCENT_CLIP: u8 = super::blocks::VEL_ACCENT;

impl LaneGenerator for BassGenerator {
    /// R1: the skeleton (see the module docs).
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents) {
        if param(ctx, P_DENSITY) <= 0.0 {
            return;
        }
        match self.clip(ctx) {
            Some(clip) => self.clip_skeleton(ctx, clip, out),
            None => self.generated_skeleton(ctx, out),
        }
    }

    /// R5: Density's clusters in the rests (a Splice follower towards the
    /// counter end keeps its rests: it answers the host there).
    fn fill(&self, ctx: &BarCtx, out: &mut BarEvents, finest: u16) {
        let counter = if ctx.host.on { ctx.host.relation.max(0.0) } else { 0.0 };
        let hold = CLIP_FILL_HOLD * (1.0 - param(ctx, P_STABS)).clamp(0.0, 1.0) * param(ctx, P_LENGTH).clamp(0.0, 1.0);
        // Below the middle of Length notes will sound shorter: the fill sees
        // only their start as taken.
        let share = (2.0 * param(ctx, P_LENGTH)).min(1.0);
        crate::chain::fill::fill(out, param(ctx, P_DENSITY) * (1.0 - counter), finest, hold, share, ctx.seed, ctx.lane);
    }

    /// R6: stab or held per note (a chain: stabs cluster), a held note's
    /// start or end sometimes a stab or two, then the Length knob.
    fn lengths(&self, ctx: &BarCtx, out: &mut BarEvents, finest: u16) {
        let density = param(ctx, P_DENSITY);
        let gate_frac = lerp(GATE_BY_DENSITY.0, GATE_BY_DENSITY.1, density);
        let gate_max = lerp(GATE_MAX_BY_DENSITY.0, GATE_MAX_BY_DENSITY.1, density).round() as u16 * TICKS_PER_STEP;
        let mut prev_stab: Option<bool> = None;
        let mut first = true;
        let n = out.len();
        for i in 0..n {
            let ev = out.as_slice();
            if ev[i].kind != EventKind::NoteOn {
                continue;
            }
            let e = ev[i];
            // A figure (a held fill run) plays as made.
            if e.has(flags::FIGURE) {
                prev_stab = Some(false);
                continue;
            }
            let next = ev[i + 1..].iter().find(|x| x.kind == EventKind::NoteOn).map_or(TICKS_PER_BAR, |x| x.tick);
            let gap = next.saturating_sub(e.tick).max(1);
            let share = Self::stab_share(ctx, e.step, first);
            first = false;
            // The chain: after a stab another one is likely (at least the
            // share), after a held note less, so the share holds overall.
            let stick = STAB_STICK.max(share);
            let enter = (share * (1.0 - stick) / (1.0 - share).max(1e-3)).clamp(0.0, 1.0);
            let u = rng::unit_f32(Self::draw(ctx, &e, S_LENGTH));
            let (stab, gate) = if e.origin & origin::FIXED != 0 {
                // A clip's note plays as written at the middle of Stabs;
                // above it held notes turn short, below it stabs hold.
                let stabs = note_param(ctx, P_STABS, e.step);
                let written = e.has(flags::STAB);
                let flip = if written { (1.0 - 2.0 * stabs).max(0.0) } else { (2.0 * stabs - 1.0).max(0.0) };
                let gate = e.gate.min(gap);
                match (written, u < flip) {
                    (w, false) => (w, gate),
                    (false, true) => (true, (gate / 2).max(GATE_MIN_TICKS as u16)),
                    (true, true) => (false, gap),
                }
            } else {
                let p = match prev_stab {
                    Some(true) => stick,
                    Some(false) => enter,
                    None => share,
                };
                let stab = u < p;
                let gate = if stab {
                    let span = gap.min(SHORT_LENGTH.2 * TICKS_PER_STEP) as f32;
                    ((span * lerp(SHORT_LENGTH.0, SHORT_LENGTH.1, density)) as u16).max(GATE_MIN_TICKS as u16)
                } else {
                    ((gap.min(gate_max) as f32 * gate_frac.max(0.85)) as u16).max(GATE_MIN_TICKS as u16)
                };
                (stab, gate)
            };
            prev_stab = Some(stab);
            let x = &mut out.as_mut_slice()[i];
            x.gate = gate;
            x.flags = if stab { x.flags | flags::STAB } else { x.flags & !flags::STAB };
        }
        partial_stabs(ctx, out, finest);
        let rng = PhraseRng::new(ctx);
        let n = out.len();
        shape_notes(&mut out.as_mut_slice()[..n], |e| note_length(ctx, e), param(ctx, P_SLIDE), |step| rng.unit(step, S_LEGATO));
    }

    /// R8: the pitches (see the module docs).
    fn melody(&self, ctx: &BarCtx, out: &mut BarEvents) {
        if self.clip(ctx).is_some() {
            clip_melody(ctx, out);
        } else {
            self.walk(ctx, out);
        }
    }

    /// R11: velocities, slides, the filter, the attack, the stream position
    /// and the loops.
    fn expression(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let persona = if self.rules.persona { Persona::new(ctx) } else { Persona::PLAIN };
        let variation = param(ctx, P_VARIATION);
        let window = vel_window(variation);
        let clip = self.clip(ctx).is_some();
        let root_key = ctx.harmony.resolve(0, Self::octave(ctx)) as i32;
        let attack = shape_attack(crate::macros::length_knob(param(ctx, P_LENGTH)));
        let expression = param(ctx, P_EXPRESSION);
        let n = out.len();
        let ev = &mut out.as_mut_slice()[..n];
        // The dynamics inside the Variation's window: the generated line's
        // tiers fitted into it, a clip's written dynamics kept inside it.
        if clip {
            let notes = ev.iter().filter(|e| e.kind == EventKind::NoteOn && e.vel > 0);
            let (sum, count) = notes.fold((0u32, 0u32), |(s, c), e| (s + e.vel as u32, c + 1));
            clamp_vels(ev, sum as f32 / count.max(1) as f32, window);
        } else {
            for e in ev.iter_mut().filter(|e| e.vel > 0) {
                e.vel = fit_vel(e.vel as f32, GEN_VEL_CENTER, GEN_VEL_SPREAD / 2.0, window, 1.0);
            }
        }
        let mut prev: Option<usize> = None;
        for i in 0..ev.len() {
            if ev[i].kind != EventKind::NoteOn {
                continue;
            }
            let e = ev[i];
            let shape = NoteShape::note(e.step, e.gate, e.key as i32, ctx.harmony.key.root);
            let accented = e.has(flags::ACCENT);
            let x = &mut ev[i];
            x.attack = attack;
            x.start = stream_region(ctx, e.step);
            let c = self.cutoff(ctx, e.step, e.key as i32 - root_key, accented);
            // Expression scales each note's filter offset (0 = none, 1 = double).
            x.cutoff = (c as f32 * expression * 2.0).round().clamp(-127.0, 127.0) as i8;
            // A slide from the note before: close enough, another pitch.
            if let Some(p) = prev {
                let pe = ev[p];
                if !e.has(flags::TIE)
                    && e.tick - pe.tick <= SLIDE_MAX_GAP_STEPS as u16 * TICKS_PER_STEP
                    && e.key != pe.key
                    && rng::unit_f32(Self::draw(ctx, &e, S_SLIDE)) < note_param_for(ctx, P_SLIDE, shape) * SLIDE_PROB_MAX * persona.slide
                {
                    ev[i].flags |= flags::TIE;
                    ev[i].glide = slide_glide(note_param_for(ctx, P_SLIDE, shape));
                    // Hold the previous note into this one.
                    ev[p].gate = e.tick - pe.tick + TIE_OVERLAP_TICKS;
                }
            }
            prev = Some(i);
        }
        // Loops last, on the notes as long as they now are.
        loops(ctx, ev, variation * persona.looping, persona.reverses);
    }

    fn param_names(&self) -> &'static [&'static str] {
        &PARAM_NAMES
    }

    fn material_beats(&self, ctx: &BarCtx) -> Option<u32> {
        self.clip(ctx).map(|c| c.beats as u32)
    }

    fn param_defaults(&self) -> [f32; MAX_GEN_PARAMS] {
        self.defaults
    }
}

/// A held note's start or end turned into one or two stabs (a share of the
/// held notes, by the stab share): the stabs on the 16ths, the rest held.
fn partial_stabs(ctx: &BarCtx, out: &mut BarEvents, grid: u16) {
    let n = out.len();
    for i in 0..n {
        let e = out.as_slice()[i];
        if e.kind != EventKind::NoteOn || e.has(flags::STAB) || e.has(flags::TIE) || e.gate < PARTIAL_MIN_GATE.max(3 * grid) {
            continue;
        }
        // A clip's held notes only above the middle of Stabs (they play as
        // written there).
        let share = if e.origin & origin::FIXED != 0 {
            PARTIAL_SHARE * (2.0 * note_param(ctx, P_STABS, e.step) - 1.0).max(0.0)
        } else {
            PARTIAL_SHARE * BassGenerator::stab_share(ctx, e.step, false)
        };
        let h = BassGenerator::draw(ctx, &e, S_PARTIAL);
        if rng::unit_f32(h) >= share {
            continue;
        }
        let stabs = 1 + rng::below(rng::derive(h, 1), PARTIAL_STABS as u32) as u16;
        let at_start = rng::chance(rng::derive(h, 2), 0.5);
        let stab_gate = (grid as f32 * SHORT_LENGTH.1.max(0.5)) as u16;
        // Leave a held part of at least a grid step.
        let stabs = stabs.min((e.gate / grid).saturating_sub(1));
        if stabs == 0 {
            continue;
        }
        let span = stabs * grid;
        let mut held = e;
        if at_start {
            held.tick = e.tick + span;
            held.gate = e.gate - span;
            held.step = (held.tick / TICKS_PER_STEP) as u8;
            held.flags &= !flags::ACCENT;
            held.origin = (e.origin & origin::FIXED) | origin::SPLIT;
            held.note_id = e.note_id.wrapping_add(0x700);
            let first = &mut out.as_mut_slice()[i];
            first.gate = stab_gate;
            first.flags |= flags::STAB;
            for k in 1..stabs {
                let mut s = e;
                s.tick = e.tick + k * grid;
                s.step = (s.tick / TICKS_PER_STEP) as u8;
                s.gate = stab_gate;
                s.flags = (s.flags | flags::STAB) & !flags::ACCENT;
                s.origin = (e.origin & origin::FIXED) | origin::SPLIT;
                s.note_id = e.note_id.wrapping_add(0x710 + k);
                out.push(s);
            }
            out.push(held);
        } else {
            out.as_mut_slice()[i].gate = e.gate - span;
            for k in 0..stabs {
                let mut s = e;
                s.tick = e.tick + e.gate - span + k * grid;
                s.step = (s.tick / TICKS_PER_STEP) as u8;
                s.gate = stab_gate;
                s.flags = (s.flags | flags::STAB) & !(flags::ACCENT | flags::TIE);
                s.origin = (e.origin & origin::FIXED) | origin::SPLIT;
                s.note_id = e.note_id.wrapping_add(0x720 + k);
                out.push(s);
            }
        }
    }
    out.sort();
}

/// The walk's state at a note: the degree walked and the last move.
#[derive(Debug, Clone, Copy, Default)]
struct Walk {
    degree: i32,
    last: i32,
}

impl BassGenerator {
    /// R8 for the generated line: the walk over every note in time order.
    /// A recursion group plays a line of its own: its first note takes the
    /// walk's next step (where the parent would have been), the others walk
    /// on from there with the group's own draws; the line goes on from the
    /// group's first note.
    fn walk(&self, ctx: &BarCtx, out: &mut BarEvents) {
        let persona = if self.rules.persona { Persona::new(ctx) } else { Persona::PLAIN };
        let octave = Self::octave(ctx);
        let spread = param(ctx, P_SPREAD);
        let ceiling = ctx.harmony.resolve_in_key(0, octave) as i32 + BASS_SPAN as i32;
        let root_key = ctx.harmony.resolve(0, octave) as i32;
        let floor = (root_key - BELOW_ROOT_SEMITONES).max(ctx.harmony.resolve_in_key(0, octave) as i32 - BELOW_KEY_FLOOR);
        let rng = PhraseRng::new(ctx);
        let count = out.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn && e.group == 0).count() as u32
            + out.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn && e.group > 0).map(|e| e.group).collect_groups();
        // Odd block: notes `start..start + len` of the bar jump up by `size`.
        // The bar's first note stays home (the root), so a block starts later.
        let block_prob = lerp(JUMP_BLOCK_PROB.0, JUMP_BLOCK_PROB.1, spread);
        let block = (spread >= JUMP_SPREAD_MIN && rng.chance(0, S_JUMP, block_prob) && count > 1).then(|| {
            let largest = (((spread - JUMP_SPREAD_MIN) / (1.0 - JUMP_SPREAD_MIN)) * JUMPS.len() as f32 - 0.001) as usize;
            let largest = largest.min(JUMPS.len() - 1);
            let size = if rng.chance(0, S_JUMP_SIZE, JUMP_LARGEST_SHARE) {
                JUMPS[largest]
            } else {
                JUMPS[rng::below(rng.at(1, S_JUMP_SIZE), largest as u32 + 1) as usize]
            };
            let start = 1 + rng::below(rng.at(0, S_JUMP_START), count - 1) as usize;
            let len = 1 + rng::below(rng.at(0, S_JUMP_LEN), JUMP_BLOCK_MAX) as usize;
            (start, len, size)
        });
        let mut main = Walk::default();
        let mut sub: Option<(u8, Walk)> = None;
        let mut prev: Option<(usize, i32)> = None;
        let mut note_index = 0usize;
        let mut first = self.rules.root >= 0.5;
        let n = out.len();
        for i in 0..n {
            let e = out.as_slice()[i];
            if e.kind != EventKind::NoteOn {
                continue;
            }
            let s = (e.tick / TICKS_PER_STEP) as usize % STEPS_PER_BAR;
            let note_spread = note_param(ctx, P_SPREAD, e.step);
            // Splice: how each candidate moves against a pitched host (D46).
            let fit = |d: i32| crate::splice::motion_weight(&ctx.host, s, prev, ctx.harmony.resolve(d, octave) as i32);
            let in_group = e.group > 0 && sub.is_some_and(|(g, _)| g == e.group);
            let state = if in_group { sub.map(|(_, w)| w).unwrap_or(main) } else { main };
            let h = if in_group { rng::hash(ctx.seed, ctx.lane, e.group, (e.note_id & 0x1F) as u8, S_GROUP) } else { Self::draw(ctx, &e, S_PITCH) };
            let mut degree = if first {
                0
            } else if self.rules.walk {
                walk_step(state.degree, state.last, h, note_spread, persona.tonal, &fit)
            } else {
                self.pick_degree(h, note_spread)
            };
            let moved = Walk { degree, last: if first { 0 } else { degree - state.degree } };
            let mut key = ctx.harmony.resolve(degree, octave) as i32;
            if self.rules.walk {
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
            }
            let moved = Walk { degree, ..moved };
            if in_group {
                sub = sub.map(|(g, _)| (g, moved));
            } else if e.group > 0 {
                // A group's first note: where the parent would have been.
                main = moved;
                sub = Some((e.group, moved));
            } else {
                main = moved;
                sub = None;
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
                }
            }
            if !in_group {
                note_index += 1;
            }
            first = false;
            out.as_mut_slice()[i].key = key.clamp(0, 127) as u8;
            prev = Some((s, key));
        }
    }
}

/// How many recursion groups `groups` (their notes' group ids) count as.
trait CollectGroups {
    fn collect_groups(self) -> u32;
}

impl<I: Iterator<Item = u8>> CollectGroups for I {
    fn collect_groups(self) -> u32 {
        let mut seen = [false; 256];
        let mut n = 0;
        for g in self {
            if !std::mem::replace(&mut seen[g as usize], true) {
                n += 1;
            }
        }
        n
    }
}

/// R8 for a clip: its written notes keep their pitches (Spread lifts weak
/// ones an octave); every other note (fills, the recursion's, the split
/// stabs keep theirs) repeats the clip note before it (the bar's last before
/// its first), at Variation's chance the octave or a scale neighbour.
fn clip_melody(ctx: &BarCtx, out: &mut BarEvents) {
    let variation = param(ctx, P_VARIATION);
    let n = out.len();
    let mut last_fixed = out.as_slice().iter().rev().find(|e| e.kind == EventKind::NoteOn && e.origin & origin::FIXED != 0).map(|e| e.key as i32);
    for i in 0..n {
        let e = out.as_slice()[i];
        if e.kind != EventKind::NoteOn {
            continue;
        }
        if e.origin & origin::FIXED != 0 {
            let mut key = e.key as i32;
            let weight = metric_weight(e.step as usize % STEPS_PER_BAR);
            let h = BassGenerator::draw(ctx, &e, S_CLIP_LIFT);
            if weight < CLIP_LIFT_BELOW && origin::of(e.origin) == origin::SKELETON && rng::unit_f32(h) < note_param(ctx, P_SPREAD, e.step) * CLIP_LIFT_MAX {
                key += 12;
            }
            out.as_mut_slice()[i].key = key.clamp(0, 127) as u8;
            last_fixed = Some(e.key as i32);
            continue;
        }
        let Some(src) = last_fixed else { continue };
        let alt = rng::unit_f32(BassGenerator::draw(ctx, &e, S_CLIP_FILL_ALT));
        let key = if alt < CLIP_FILL_ALT * variation {
            match (alt * 3.0 / (CLIP_FILL_ALT * variation).max(1e-6)) as u8 {
                0 => src + 12,
                1 => neighbour(ctx, src, true),
                _ => neighbour(ctx, src, false),
            }
        } else {
            src
        };
        out.as_mut_slice()[i].key = key.clamp(0, 127) as u8;
    }
    let _ = S_CLIP_STAB;
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
    /// How much tonal stability steers the walk (exponent on [`STABILITY`]).
    tonal: f32,
    /// How much the seed loops its long notes ([`loops`]).
    looping: f32,
    /// The seed reverses those notes instead of looping them.
    reverses: bool,
}

impl Persona {
    /// No character: the rhythm and slides as the parameters say.
    const PLAIN: Self = Self { sync: 0.0, slide: 1.0, tonal: 1.0, looping: 0.65, reverses: false };

    fn new(ctx: &BarCtx) -> Self {
        let u = |k: u8| rng::unit_f32(rng::hash(ctx.seed, ctx.lane, 0, k, S_PERSONA));
        Self {
            sync: lerp(PERSONA_SYNC.0, PERSONA_SYNC.1, u(0)),
            slide: lerp(PERSONA_SLIDE.0, PERSONA_SLIDE.1, u(1)),
            tonal: lerp(PERSONA_TONAL.0, PERSONA_TONAL.1, u(4)),
            looping: lerp(PERSONA_LOOP.0, PERSONA_LOOP.1, u(5)),
            reverses: u(6) < PERSONA_REVERSE_SHARE,
        }
    }
}

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
/// and its answer, about 2.5 notes a bar, D57's half pace; 1: everything).
const GROOVE_REVEAL: (f32, f32) = (280.0, 115.0);
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
fn loops(ctx: &BarCtx, events: &mut [LiveEvent], share: f32, reverse: bool) {
    if share <= 0.0 {
        return;
    }
    let chance = LOOP_SHARE_MAX * share.clamp(0.0, 1.0).powf(1.5);
    for ev in events.iter_mut().filter(|e| e.kind == EventKind::NoteOn && e.gate >= LOOP_MIN_GATE && !e.has(flags::TIE)) {
        let at = *ev;
        let draw = |stream: u32| BassGenerator::draw(ctx, &at, stream);
        if !rng::chance(draw(S_LOOP), chance) {
            continue;
        }
        if reverse {
            ev.flags |= flags::REVERSE;
            continue;
        }
        let gate = ev.gate;
        let fits = |len: u8, at: u8| at as u16 + 2 * len as u16 <= gate;
        let weights: [f32; LOOP_LENGTHS.len()] = std::array::from_fn(|k| if fits(LOOP_LENGTHS[k].0, LOOP_STARTS[0]) { LOOP_LENGTHS[k].1 } else { 0.0 });
        let Some(k) = rng::weighted_pick(draw(S_LOOP_LEN), &weights) else { continue };
        let len = LOOP_LENGTHS[k].0;
        let starts = LOOP_STARTS.iter().filter(|&&a| fits(len, a)).count().max(1);
        ev.loop_len = len;
        ev.loop_at = LOOP_STARTS[(draw(S_LOOP_AT) % starts as u64) as usize];
        if rng::chance(draw(S_LOOP_PONG), PING_PONG_SHARE) {
            ev.flags |= flags::PING_PONG;
        }
    }
}

/// The Length a note reads: its own move (step and height) around the
/// lane's, fading out towards full length so long lines stay held.
fn note_length(ctx: &BarCtx, ev: &LiveEvent) -> f32 {
    let shape = NoteShape { step: ev.step, gate: None, pitch_class: Some((ev.key as i32 - ctx.harmony.key.root as i32).rem_euclid(12) as u8) };
    held_length(param(ctx, P_LENGTH), note_param_for(ctx, P_LENGTH, shape))
}

/// A note's Length from the lane's `base` and its `moved` value: the move
/// fades out over the top quarter, none at full length.
pub fn held_length(base: f32, moved: f32) -> f32 {
    base + (moved - base) * (4.0 * (1.0 - base)).clamp(0.0, 1.0)
}

// (user, 2026-10-21: at Shape 0 and full Density almost every note was
// held: the 16ths kept their whole length.) Share of its length a note up
// to a stab long keeps at Length 0 (all of it at the middle).
const STAB_KEEP: f32 = 0.35;
// MUSICAL: tune by ear (user, 2026-10-24: at Shape 0 the stabs were far
// too short, the top right). How much of a note's shortening a Length of
// 0 still leaves undone: 0 shortens all the way, 1 not at all.
const SHORTEN_FLOOR: f32 = 0.4;

/// How far a Length below the middle shortens, 0 (most) .. 1 (none),
/// without the floor and with it.
fn shortening(length: f32) -> (f32, f32) {
    let k = length.clamp(0.0, 0.5) * 2.0;
    (k, SHORTEN_FLOOR + (1.0 - SHORTEN_FLOOR) * k)
}

/// The share of a stab a Length keeps: [`STAB_KEEP`] at full shortening.
fn stab_keep(k: f32) -> f32 {
    STAB_KEEP + (1.0 - STAB_KEEP) * k
}

/// How much shorter the sampler's pitch envelope reads a stab at `length`
/// than it plays: as long as it played before the floor, so the pitch move
/// stays as it was (user, 2026-10-24: it is right at Shape 0).
pub(crate) fn pitch_env_time(length: f32) -> f32 {
    let (raw, k) = shortening(length);
    stab_keep(raw) / stab_keep(k)
}

/// A gate at a Length below the middle: towards a stab. The part over a
/// stab goes first, a note up to a stab long shortens to [`stab_keep`].
fn shortened(gate: u16, length: f32) -> u16 {
    let k = shortening(length).1;
    let stab = gate.min(STAB_TICKS);
    let short = (stab as f32 * stab_keep(k)).max(GATE_MIN_TICKS.min(stab as f32));
    short as u16 + ((gate - stab) as f32 * k) as u16
}

/// `length` (0.5 = unchanged): below 0.5 notes shorten towards stabs,
/// above it they hold towards the next onset, from [`LEGATO_FROM`] a rising
/// share of held notes ties into it and glides.
fn shape_notes(events: &mut [LiveEvent], length_of: impl Fn(&LiveEvent) -> f32, slide: f32, roll: impl Fn(u8) -> f32) {
    let glide = slide_glide(slide);
    // Slide above its middle ties more held notes (liquid).
    let liquid = LEGATO_SLIDE * (2.0 * slide - 1.0).max(0.0);
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
    use crate::generator::test_util::{assert_well_formed, ctx, notes};
    use crate::harmony::scale_for;
    use crate::TICKS_PER_BAR;

    /// Two-step backbone: kicks on steps 0 and 10, snares on 4 and 12.
    const TWO_STEP: [u8; 16] = [120, 0, 0, 0, 80, 0, 0, 0, 0, 0, 118, 0, 80, 0, 0, 0];
    const EMPTY: [u8; 16] = [0; 16];

    /// The plain generator (no seed character, pool pitches), for the rule tests.
    fn test_rules() -> BassRules {
        BassRules::default()
    }

    fn generator() -> BassGenerator {
        BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, walk: false, scheme: false, ..test_rules() })
    }

    /// The generator's phases of the chain (D57: R1, R5, R6, R8, R11) on one
    /// bar, at the generator's own pace (no speed, recursion or groove).
    fn render(g: &BassGenerator, c: &BarCtx) -> BarEvents {
        let mut out = BarEvents::default();
        g.render_bar(c, &mut out);
        out.sort();
        g.fill(c, &mut out, TICKS_PER_STEP);
        g.lengths(c, &mut out, TICKS_PER_STEP);
        g.melody(c, &mut out);
        g.expression(c, &mut out);
        crate::chain::cleanup(&mut out);
        out
    }

    /// The skeleton alone (R1).
    fn skel(g: &BassGenerator, seed: u32, p: &[f32; MAX_GEN_PARAMS], mask: &[u8; 16]) -> Vec<LiveEvent> {
        let mut c = ctx(seed, 0, p, mask);
        c.lane = 1;
        let mut out = BarEvents::default();
        g.render_bar(&c, &mut out);
        out.as_slice().to_vec()
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
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules::default());
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
        assert!(total > 200);
        assert!(share > 0.55, "root share {share}");
        // The first note of every bar is the root.
        for seed in 0..50 {
            if let Some(first) = bar(&g, seed, 0, &PARAM_DEFAULTS, &TWO_STEP).first() {
                assert_eq!(first.key % 12, 9);
            }
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
            .flat_map(|seed| bar(&g, seed, 0, &params(|p| {
                p[P_SPREAD] = 1.0;
                p[P_DENSITY] = 0.8;
            }), &TWO_STEP))
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
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { root: 0.0, walk: false, scheme: false, ..test_rules() });
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
            let p = params(|p| {
                p[P_SLIDE] = 1.0;
                p[P_DENSITY] = 0.7;
            });
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
                    let _ = b;
                    let s = steps(&skel(&g, seed, &avoid, &TWO_STEP));
                    assert!(kicks.iter().all(|k| !s.contains(k)), "co-onset in avoid mode (seed {seed})");
                    let double = params(|p| {
                        p[P_DENSITY] = density;
                        p[P_KICK_MODE] = 1.0;
                    });
                    let s = steps(&skel(&g, seed, &double, &TWO_STEP));
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
                .flat_map(|seed| steps(&skel(g, seed, &params(|p| p[P_COMPLEXITY] = 1.0), &busy)))
                .filter(|s| s % 2 == 1)
                .count()
        };
        let follow = generator();
        let ignore = BassGenerator::new(PARAM_DEFAULTS, BassRules { gaps: 0.0, scheme: false, ..test_rules() });
        assert!(count_on_odd(&follow) < count_on_odd(&ignore));
    }

    #[test]
    fn density_is_monotonic() {
        let g = generator();
        for seed in 0..30 {
            let mut prev: Vec<usize> = Vec::new();
            for d in 0..=10 {
                let now = steps(&skel(&g, seed, &params(|p| p[P_DENSITY] = d as f32 / 10.0), &TWO_STEP));
                assert!(prev.iter().all(|s| now.contains(s)));
                prev = now;
            }
            assert!(prev.len() >= 8);
        }
        assert!(bar(&g, 1, 0, &params(|p| p[P_DENSITY] = 0.0), &TWO_STEP).is_empty());
        // The phases only add to it: Density's fill above its start.
        let full = bar(&g, 1, 0, &params(|p| p[P_DENSITY] = 1.0), &TWO_STEP);
        assert!(full.len() >= skel(&g, 1, &params(|p| p[P_DENSITY] = 1.0), &TWO_STEP).len());
    }

    #[test]
    fn a_letter_plays_alike_in_every_bar() {
        // D57: no phrase rule; the bar of the phrase does not matter.
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules::default());
        for seed in 0..60 {
            for density in [0.2, 0.5, 1.0] {
                let p = params(|p| p[P_DENSITY] = density);
                let b0 = bar(&g, seed, 0, &p, &TWO_STEP);
                for b in 1..4 {
                    assert_eq!(b0, bar(&g, seed, b, &p, &TWO_STEP), "seed {seed} density {density} bar {b}");
                }
            }
        }
    }

    #[test]
    fn spread_sizes_the_walk() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..test_rules() });
        let intervals = |spread: f32| -> Vec<i32> {
            let p = params(|p| {
                p[P_SPREAD] = spread;
                p[P_DENSITY] = 0.7;
            });
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
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..test_rules() });
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
    fn grooves_follow_the_vocabulary() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { persona: false, ..test_rules() });
        let count = |density: f32| -> f32 {
            let p = params(|p| p[P_DENSITY] = density);
            (0..200).map(|seed| skel(&g, seed, &p, &EMPTY).len()).sum::<usize>() as f32 / 200.0
        };
        let (low, mid, high) = (count(0.02), count(0.5), count(1.0));
        // (D57: half the old pace at the middle, on the 16ths.)
        assert!(low >= 0.9 && low <= 1.6 && (2.0..3.2).contains(&mid) && high > mid && high <= 10.0, "{low} {mid} {high}");
        // Dense and stabby: the gaps fill sooner (at full density everything
        // plays either way).
        let stabby = |stabs: f32| -> f32 {
            let p = params(|p| {
                p[P_DENSITY] = 0.75;
                p[P_STABS] = stabs;
            });
            (0..200).map(|seed| skel(&g, seed, &p, &EMPTY).len()).sum::<usize>() as f32 / 200.0
        };
        assert!(stabby(1.0) > stabby(0.5) + 1.0, "{} vs {}", stabby(1.0), stabby(0.5));
        // Every onset is a step of the figure vocabulary; most on the 8th grid.
        let p = params(|p| p[P_DENSITY] = 0.7);
        let ev: Vec<LiveEvent> = (0..200).flat_map(|seed| skel(&g, seed, &p, &EMPTY)).collect();
        let on_8ths = ev.iter().filter(|e| (e.tick / TICKS_PER_STEP) % 2 == 0).count();
        assert!(on_8ths * 10 > ev.len() * 7, "{on_8ths} of {}", ev.len());
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
    fn stabs_cluster_and_lone_stabs_are_rarer() {
        // D57: a stab after a stab is likely, after a held note less so.
        let g = generator();
        let p = params(|p| {
            p[P_DENSITY] = 0.8;
            p[P_STABS] = 0.5;
        });
        let (mut after_stab, mut stab_after_stab, mut after_held, mut stab_after_held) = (0, 0, 0, 0);
        for seed in 0..300 {
            let ev = bar(&g, seed, 0, &p, &EMPTY);
            for w in ev.windows(2) {
                let (a, b) = (w[0].has(flags::STAB), w[1].has(flags::STAB));
                if a {
                    after_stab += 1;
                    stab_after_stab += b as u32;
                } else {
                    after_held += 1;
                    stab_after_held += b as u32;
                }
            }
        }
        let (ss, sh) = (stab_after_stab as f32 / after_stab as f32, stab_after_held as f32 / after_held as f32);
        assert!(ss > sh + 0.2, "a stab follows a stab {ss}, a held note {sh}");
    }

    #[test]
    fn some_held_notes_start_or_end_with_stabs() {
        let g = generator();
        let p = params(|p| {
            p[P_DENSITY] = 0.3;
            p[P_STABS] = 0.8;
        });
        let split: usize = (0..300).map(|seed| bar(&g, seed, 0, &p, &EMPTY).iter().filter(|e| origin::of(e.origin) == origin::SPLIT).count()).sum();
        assert!(split > 20, "{split}");
        let none: usize = (0..100).map(|seed| bar(&g, seed, 0, &params(|p| p[P_STABS] = 0.0), &EMPTY).iter().filter(|e| origin::of(e.origin) == origin::SPLIT).count()).sum();
        assert_eq!(none, 0, "no stabs at Stabs 0");
    }

    #[test]
    fn seeds_slide_their_own_way() {
        let g = BassGenerator::new(PARAM_DEFAULTS, BassRules { scheme: false, ..test_rules() });
        let p = params(|p| p[P_DENSITY] = 0.7);
        let slides: Vec<usize> = (0..300).map(|seed| bar(&g, seed, 0, &p, &EMPTY).iter().filter(|e| e.flags & flags::TIE != 0).count()).collect();
        // Some seeds slide a lot, many barely.
        assert!(slides.iter().max().unwrap() >= &3 && slides.iter().filter(|&&s| s == 0).count() > 50);
    }

    #[test]
    fn shortening_has_a_floor_and_the_pitch_envelope_reads_the_old_length() {
        // The middle plays as written, its pitch envelope as played.
        assert_eq!(shortened(96, 0.5), 96);
        assert_eq!(pitch_env_time(0.5), 1.0);
        // Length 0: a dotted 16th stab keeps 61 % (was 35 %), its part over
        // a stab 40 % (was none).
        assert_eq!(shortened(36, 0.0), 21);
        assert_eq!(shortened(96, 0.0), 45);
        // The pitch envelope reads the stab as long as before the floor.
        assert!((36.0 * stab_keep(SHORTEN_FLOOR) * pitch_env_time(0.0) - 36.0 * STAB_KEEP).abs() < 1e-3);
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
    fn golden_bars() {
        // Pinned output: a change here changes how saved seeds sound.
        let g = generator();
        let got: Vec<String> = [1u32, 2, 3].iter().map(|&s| notes(&bar(&g, s, 0, &PARAM_DEFAULTS, &TWO_STEP))).collect();
        assert_eq!(got, GOLDEN, "\n{got:#?}");
    }

    const GOLDEN: [&str; 3] = [
        // 2026-10-23: the render chain (D57).
        "8:33",
        "8:33 14:40",
        "6:33",
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
    fn density_thins_and_fills_the_clip() {
        let g = generator().with_clips(one_clip(&crate::clips::tables::A[3]));
        let key = crate::generator::test_util::a_minor();
        let count = |d: f32| (0..8).map(|b| clip_events(&g, &clip_params(|p| p[P_DENSITY] = d), b, key).len()).sum::<usize>();
        let (low, mid, high) = (count(0.1), count(0.5), count(1.0));
        assert!(low < mid && mid < high, "thinned, as written, filled: {low} {mid} {high}");
        // Thinned notes keep the beats.
        for b in 0..8 {
            for e in clip_events(&g, &clip_params(|p| p[P_DENSITY] = 0.0001), b, key).iter().skip(1) {
                assert_eq!(e.tick % (4 * TICKS_PER_STEP), 0, "on a beat: {}", e.tick);
            }
        }
        // Fills repeat the written pitches (at Variation 0).
        for b in 0..8 {
            let written: std::collections::BTreeSet<u8> = clip_events(&g, &clip_params(|_| {}), b, key).iter().map(|e| e.key).collect();
            let split: std::collections::BTreeSet<u8> = clip_events(&g, &clip_params(|p| p[P_DENSITY] = 1.0), b, key).iter().map(|e| e.key).collect();
            assert_eq!(written, split, "bar {b}");
        }
    }

    #[test]
    fn stabs_reshape_the_clip() {
        let g = generator().with_clips(one_clip(&crate::clips::tables::B[1]));
        let key = crate::generator::test_util::a_minor();
        let gates = |p: [f32; MAX_GEN_PARAMS]| (0..8).flat_map(|b| clip_events(&g, &p, b, key)).map(|e| e.gate as u32).sum::<u32>();
        let mid = gates(clip_params(|_| {}));
        assert!(gates(clip_params(|p| p[P_STABS] = 1.0)) < mid, "stabs shorten held notes");
        assert!(gates(clip_params(|p| p[P_STABS] = 0.0)) > mid, "and hold the stabs");
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
    fn full_density_fills_the_clip() {
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
        // Fills (in the rests of a half-empty clip).
        let g = generator().with_clips(one_clip(&crate::clips::tables::C[2]));
        let fills = |variation: f32| -> Vec<LiveEvent> {
            (0..8).flat_map(|b| clip_events(&g, &clip_params(|p| {
                p[P_DENSITY] = 0.8;
                p[P_VARIATION] = variation;
            }), b, key)).filter(|e| origin::of(e.origin) == origin::FILL).collect()
        };
        // (In clusters, D56: a run may be one held note, so count the time.)
        let filled: u32 = fills(0.0).iter().map(|e| e.gate as u32).sum();
        assert!(filled >= 8 * 32, "the rests fill: {filled} ticks");
        // Low Variation repeats the note it fills from; high varies some.
        // (Over seeds: a bar has few fill runs since D56, one draw each.)
        let keys = |variation: f32| -> usize {
            (0..8u32)
                .map(|seed| {
                    (0..8).flat_map(|b| clip_events_seeded(&g, &clip_params(|p| { p[P_DENSITY] = 0.8; p[P_VARIATION] = variation; }), seed, b))
                        .filter(|e| origin::of(e.origin) == origin::FILL)
                        .map(|e| e.key)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                })
                .sum()
        };
        assert!(keys(1.0) > keys(0.0), "Variation varies the fills: {} vs {}", keys(0.0), keys(1.0));
        assert_eq!(fills(1.0).iter().map(|e| (e.tick, e.key)).collect::<Vec<_>>(), fills(1.0).iter().map(|e| (e.tick, e.key)).collect::<Vec<_>>(), "deterministic");
    }
}
