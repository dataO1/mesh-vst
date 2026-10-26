//! Generated forms (design D48, D61): four phrases, two motives of cycles
//! over 64 bars, and a recipe for the second 64.
//!
//! A, B, C and D are phrase slots. A phrase is its letter's notes (the
//! letter seed, from the pattern seed) looped over a seeded length in beats,
//! a power of two, from a seeded offset into them. A and B repeat as a
//! cycle (A+B or A+B+A) whose split Contrast sets; a motive repeats its
//! cycle over 16, 32 or 64 bars, and the second motive (Y) plays the
//! letters' second patterns. A plan of one character (a roller, question
//! and answer, a breakdown, ...) picks the cycle shapes and how Y differs;
//! C ends a motive, D the pass. The 16-bar grid holds by construction:
//! cycles divide 16 bars and fills overwrite, so no section starts anywhere
//! but bars 1, 17, 33 and 49. Operators live in the plan only: its fixed
//! marks and its `?` slots, which the seed fills. Variation sets how much
//! of the plan's room for variants and extra fills is used, Shape tilts the
//! phrase and cycle lengths.
//!
//! Building is allocation-free into a [`FormBuilder`] and a form from
//! [`Form::generated`].

use crate::form::{variant_seed, Form, Letter, SeedClass, Segment, BEATS_PER_BAR, FORM_BARS, MAX_PASS_SEGMENTS};
use crate::ops::{self, Op, OpChain, Place};
use crate::rng;

/// Beats of a pass, of a section and of a cycle (a 4-bar phrase).
pub const PASS_BEATS: u16 = FORM_BARS * BEATS_PER_BAR;
pub const SECTION_BEATS: u16 = 16 * BEATS_PER_BAR;
pub const WINDOW_BEATS: u16 = 4 * BEATS_PER_BAR;
const SECTIONS: usize = (PASS_BEATS / SECTION_BEATS) as usize;
const WINDOWS: usize = (SECTION_BEATS / WINDOW_BEATS) as usize;

/// Steps Variation and Shape are read in for the form, so a small knob move
/// does not build another one.
pub const BANDS: u8 = 5;

/// The band of knob value `v` (0..1).
pub fn band(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * (BANDS - 1) as f32).round() as u8
}

fn share(b: u8) -> f32 {
    b.min(BANDS - 1) as f32 / (BANDS - 1) as f32
}

fn lerp((a, b): (f32, f32), t: f32) -> f32 {
    a + (b - a) * t
}

/// What a generated form is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormSeeds {
    /// Each letter's phrase seed (A..D): the phrase seed's, or a pinned one.
    pub phrases: [u32; 4],
    /// The fill plan and its slots.
    pub fill: u32,
    /// The second pass's recipe.
    pub second: u32,
}

impl FormSeeds {
    pub fn new(phrase: u32, fill: u32, second: u32) -> Self {
        Self { phrases: Letter::ALL.map(|l| phrase_seed(phrase, l)), fill, second }
    }
}

/// The seed letter `letter`'s phrase draws from for phrase seed `phrase`.
pub fn phrase_seed(phrase: u32, letter: Letter) -> u32 {
    rng::hash(phrase, 0, letter.index(), 0, S_PHRASE) as u32
}

/// A generated form's inputs: the seeds, the knob bands ([`band`]) and the
/// lane (drum phrases keep the backbeat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormSpec {
    pub seeds: FormSeeds,
    pub variation: u8,
    pub shape: u8,
    /// The Contrast band: how often B's phrase is a step off A's (D49).
    pub contrast: u8,
    pub drums: bool,
}

// MUSICAL: tune by ear (user, 2026-10-21: phrases are powers of two in
// beats, at most 4 bars; A and B mostly 1 or 2 bars). Lengths and weights
// of the main phrases (A, B) and of the fills (C, D).
const MAIN_LENGTHS: [(u8, f32); 4] = [(2, 0.15), (4, 0.35), (8, 0.35), (16, 0.15)];
const FILL_LENGTHS: [(u8, f32); 4] = [(1, 0.15), (2, 0.35), (4, 0.35), (8, 0.15)];
// MUSICAL: tune by ear (user, 2026-10-21: low Shape shorter phrases, cells;
// high Shape longer ones; a bias, not a mapping). How far Shape at either
// end tilts a length's weight, per octave of length off the middle.
const SHAPE_TILT: f32 = 1.2;

/// A letter's phrase: its length and its offset into the letter's notes,
/// in beats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Phrase {
    pub beats: u8,
    pub offset: u8,
}

/// The phrase of `letter` from its phrase seed `seed`, in section `section`
/// (0 when the form keeps one length per letter), at Shape band `shape`.
pub fn phrase(seed: u32, letter: Letter, section: u8, shape: u8, drums: bool) -> Phrase {
    let table = if letter.class() == SeedClass::Main { &MAIN_LENGTHS } else { &FILL_LENGTHS };
    let mid = (table[1].0 as f32 * table[2].0 as f32).sqrt();
    let tilt = SHAPE_TILT * (2.0 * share(shape) - 1.0);
    let weights = table.map(|(len, w)| w * 2f32.powf(tilt * (len as f32 / mid).log2()));
    let beats = table[rng::weighted_pick(rng::hash(seed, drums as u8, section, 0, S_LENGTH), &weights).unwrap_or(1)].0;
    Phrase { beats, offset: phrase_offset(seed, drums) % beats }
}

/// A letter's offset before its length: one per letter, whatever the
/// section's length, in even beats (the kick's beats stay the kick's; the
/// bass then starts on a note there, see `pattern::onset_shift`); the drums
/// have none (user, 2026-10-21).
fn phrase_offset(seed: u32, drums: bool) -> u8 {
    if drums {
        return 0;
    }
    rng::below(rng::hash(seed, 0, 0, 0, S_OFFSET), 16) as u8 & !1
}

// MUSICAL: first draft, tune by ear (user, 2026-10-21: Contrast moves B's
// phrase length too, one power-of-two step). Chance B's phrase is a step
// off A's at Contrast 1.
const B_LENGTH_STEP: f32 = 0.7;

/// B's phrase from A's `a` (D49): A's length at Contrast band 0, one step
/// longer or shorter with a chance rising with Contrast; its own offset.
pub fn answer_phrase(a: Phrase, b_seed: u32, section: u8, contrast: u8, drums: bool) -> Phrase {
    let (lo, hi) = (MAIN_LENGTHS[0].0, MAIN_LENGTHS[MAIN_LENGTHS.len() - 1].0);
    let mut beats = a.beats;
    if rng::chance(rng::hash(b_seed, drums as u8, section, 0, S_B_STEP), share(contrast) * B_LENGTH_STEP) {
        let up = rng::chance(rng::hash(b_seed, drums as u8, section, 1, S_B_STEP), 0.5);
        beats = if (up && a.beats < hi) || a.beats / 2 < lo { (a.beats * 2).min(hi) } else { a.beats / 2 };
    }
    Phrase { beats, offset: phrase_offset(b_seed, drums) % beats }
}

/// The phrase letter `letter` plays as pattern `variant` (D60) in the form
/// of `spec`: one for the whole form; B answers A of the same pattern.
pub fn letter_phrase(spec: &FormSpec, letter: Letter, variant: u8) -> Phrase {
    let lane = spec.drums as u8;
    let variant = if letter.class() == SeedClass::Main { variant } else { 0 };
    let seed = |l: Letter| variant_seed(spec.seeds.phrases[l.index() as usize], lane, variant);
    if letter == Letter::B {
        let a = phrase(seed(Letter::A), Letter::A, 0, spec.shape, spec.drums);
        answer_phrase(a, seed(Letter::B), 0, spec.contrast, spec.drums)
    } else {
        phrase(seed(letter), letter, 0, spec.shape, spec.drums)
    }
}

/// A plan (D61): a character. It picks the cycle's shape and how the
/// second motive (Y) differs from the first (X); the seed fills the rest.
struct Plan {
    name: &'static str,
    /// Cycle shapes the seed picks from, per motive: its letters in order.
    shapes: &'static [&'static str],
    /// The marks of A and B in X and in Y (`?` a slot the seed fills).
    x: [&'static str; 2],
    y: [&'static str; 2],
    /// Chance Y plays X's split the other way round (else its own), and
    /// chance it plays the lane's other sound (`*`) at Contrast 1.
    flip: f32,
    sound: f32,
    /// Chance 16-bar motives alternate (X Y X Y), else they pair (X X Y Y).
    alternate: f32,
    /// The pickup the form may open with.
    head: &'static str,
}

// MUSICAL: tune by ear (user, 2026-10-26: a cycle of A and B repeats, A
// asks and B answers; the second motive is another one: the split, the
// other patterns, operators, the sound; the plans are characters). X
// keeps to its slots, the character shows in Y.
// Marks: the operators, `'` and `*`; `?` slots the seed fills.
const PLANS: [Plan; 10] = [
    Plan { name: "steady roller", shapes: &["AB", "ABA"], x: ["", "?"], y: ["?", "?a"], flip: 0.5, sound: 0.3, alternate: 0.5, head: "" },
    Plan { name: "question and answer", shapes: &["AB"], x: ["q", "a"], y: ["q?", "a?"], flip: 0.7, sound: 0.2, alternate: 0.7, head: "C" },
    Plan { name: "slow build", shapes: &["AB", "ABA"], x: ["", ""], y: ["?", "p?"], flip: 0.3, sound: 0.4, alternate: 0.1, head: "" },
    Plan { name: "two-step cells", shapes: &["AB", "ABA"], x: ["", "?"], y: ["j", "z?"], flip: 0.5, sound: 0.3, alternate: 0.5, head: "C" },
    Plan { name: "breakdown", shapes: &["AB"], x: ["", "?"], y: ["h", "h?"], flip: 0.4, sound: 0.5, alternate: 0.3, head: "" },
    Plan { name: "B-led", shapes: &["BA", "BAB"], x: ["?", ""], y: ["?a", "?"], flip: 0.5, sound: 0.3, alternate: 0.5, head: "" },
    Plan { name: "cells and fragments", shapes: &["AB", "ABA"], x: ["", "?"], y: ["j?", "F"], flip: 0.5, sound: 0.3, alternate: 0.5, head: "C" },
    Plan { name: "space", shapes: &["AB"], x: ["", "?"], y: ["g", "g?"], flip: 0.4, sound: 0.3, alternate: 0.4, head: "" },
    Plan { name: "switch-up", shapes: &["AB"], x: ["", "?"], y: ["r", "i"], flip: 0.6, sound: 0.4, alternate: 0.6, head: "" },
    Plan { name: "bounce", shapes: &["ABA", "AB"], x: ["", "?"], y: ["u?", "z"], flip: 0.5, sound: 0.3, alternate: 0.5, head: "C" },
];

/// Plans there are.
pub const PLAN_COUNT: usize = PLANS.len();

// MUSICAL: tune by ear (user, 2026-10-26: motives of 16, 32 or 64 bars,
// the seed picks; 16 alternate or pair, 32 X Y, 64 X with Y in the second
// pass). Motive lengths in bars and their weights.
const MOTIVE_BARS: [(u16, f32); 3] = [(16, 0.45), (32, 0.4), (64, 0.15)];
// MUSICAL: tune by ear (user, 2026-10-26: 2 beats A 6 beats B, 12 A 4 B;
// any split as long as the cycle fills 16 bars). Cycle lengths in beats
// (they divide a section) and their weights; Shape tilts them like the
// phrases.
const CYCLE_LENGTHS: [(u16, f32); 4] = [(8, 0.3), (16, 0.4), (32, 0.22), (64, 0.08)];
// MUSICAL: tune by ear (user, 2026-10-26: the split is the seed's, not
// Contrast's). A's shares of a cycle and their weights.
const SPLITS: [(f32, f32); 5] = [(0.25, 1.0), (0.375, 0.8), (0.5, 0.6), (0.625, 0.8), (0.75, 1.0)];
// MUSICAL: tune by ear (user, 2026-10-26: the drums much longer on A, B
// sparingly or not at all most of the time). The drums' cycle lengths and
// weights, the chance a drum cycle has a B at all, and B's share then.
const DRUM_CYCLES: [(u16, f32); 2] = [(32, 0.4), (64, 0.6)];
const DRUM_B: f32 = 0.3;
const DRUM_B_SHARE: f32 = 0.125;

// MUSICAL: tune by ear (user, 2026-10-21: Variation scales the form's
// variation too, at 0 a little, at 1 the full set). Share of `?` slots the
// seed fills, chance of an extra fill at a 4-bar seam inside a section, at
// Variation 0 and 1.
const SLOT_FILL: (f32, f32) = (0.25, 0.85);
const EXTRA_FILL: (f32, f32) = (0.04, 0.3);
/// Chance the form opens with its plan's pickup, and its longest length.
const HEAD_CHANCE: f32 = 0.6;
const HEAD_BEATS: u8 = 4;
/// Longest extra fill (beats).
const EXTRA_BEATS: u8 = 4;
/// Chance an extra fill is D (else C).
const EXTRA_D: f32 = 0.3;

/// Where a section ends: inside a motive, at a motive's end, at the pass's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seam {
    Inner,
    Motive,
    Pass,
}

// MUSICAL: tune by ear (user, 2026-10-26: fills at the seams; C a short
// one at each motive's end, D a longer one at the pass's, each its own
// pattern; inside a long motive the 16-bar seams rarely fill, briefly).
/// Longest seam fill per kind of seam (inner, motive, pass; beats).
const SEAM_BEATS: [u8; 3] = [2, 4, 8];
/// Chance an inner seam plays its fill, at Variation 0 and 1.
const INNER_SEAM: (f32, f32) = (0.15, 0.4);
// MUSICAL: tune by ear (user, 2026-10-21: the seam fill kinds are seeded
// too; C the mild fill, D the aggressive one, the bigger change at 64).
/// Chance a seam draws its kind from its pool (else the plain letter), at
/// Variation 0 and 1, and how much more often in the second pass.
const SEAM_DRAW: (f32, f32) = (0.35, 0.8);
const SECOND_SEAM: f32 = 0.15;
/// The kinds of the C seams (motive ends, inner) and of the D seam.
const C_KINDS: &[&str] = &["C", "C*", "Cx", "CF", "Cj", "Cz", "Cg", "Ch"];
const D_KINDS: &[&str] = &["D", "D*", "Dx", "D*x", "Dz", "D*F", "Dj"];

// MUSICAL: tune by ear (user, 2026-10-26: not enough operators and variety
// in the line at the seams). The operators the part before a seam may
// play (the bass, the drums); chance a motive's or the pass's seam turns
// the line around at Variation 0 and 1 (an inner seam half as often), the
// second pass's more; chance the pass's end adds a push.
const TURNS: [Op; 8] = [Op::DoubleTime, Op::Passing, Op::Fragment, Op::Sequence, Op::OctaveUp, Op::Retrograde, Op::Hemiola, Op::CallResponse];
const DRUM_TURNS: [Op; 4] = [Op::DoubleTime, Op::Fragment, Op::Hemiola, Op::Displacement];
const TURN_CHANCE: (f32, f32) = (0.5, 0.95);
const SECOND_TURN: f32 = 0.1;
const PASS_PUSH: f32 = 0.5;
/// Beats before a seam the turnaround plays.
const TURN_BEATS: u16 = BEATS_PER_BAR;

/// A plan token: a letter, its marks, which of its patterns plays (D60);
/// `?` in `ops` is a slot.
#[derive(Debug, Clone, Copy)]
struct Tok {
    letter: Letter,
    prime: bool,
    swap: bool,
    ops: OpChain,
    variant: u8,
}

impl Tok {
    const PLAIN: Tok = Tok { letter: Letter::A, prime: false, swap: false, ops: OpChain::EMPTY, variant: 0 };
}

fn tok(s: &str) -> Option<Tok> {
    let mut chars = s.chars();
    let letter = match chars.next()? {
        'A' => Letter::A,
        'B' => Letter::B,
        'C' => Letter::C,
        'D' => Letter::D,
        _ => return None,
    };
    let mut t = Tok { letter, ..Tok::PLAIN };
    apply_marks(&mut t, chars.as_str()).then_some(t)
}

/// Put marks `s` (`'`, `*`, operators) on `t`; false on one it does not know.
fn apply_marks(t: &mut Tok, s: &str) -> bool {
    for c in s.chars() {
        match c {
            '\'' => t.prime = true,
            '*' => t.swap = true,
            c => match Op::from_char(c) {
                Some(op) => {
                    t.ops.push(op);
                }
                None => return false,
            },
        }
    }
    true
}

/// How a pass is built: the first as the plan says, the second through its
/// recipe ([`Mods`] fields the recipe sets).
#[derive(Debug, Clone, Copy, Default)]
struct Mods {
    /// Seed of the slots and the extra fills.
    salt: u32,
    /// Seed of which operator each letter plays per kind of place: the fill
    /// seed, the same for both passes and every attempt (snapshotted).
    snap: u32,
    /// Share of slots a melodic operator fills (Variation), chance of an
    /// extra fill.
    slot: f32,
    extra: f32,
    /// Chance a seam draws its kind from its pool ([`C_KINDS`], [`D_KINDS`]),
    /// and an inner seam plays at all.
    seam: f32,
    inner: f32,
    /// Chance the line turns around before a motive's or the pass's seam
    /// ([`TURNS`]), and which pass this is.
    turn: f32,
    pass: u8,
    /// The second pass's recipe ([`Recipe`]): from section `recut_from` on
    /// A and B play these re-cuts on top; from `swap_from` A and B trade
    /// places; from `answer_from` B plays `answer` on top; a filled slot
    /// becomes a push (`x` `y` `p`) at chance `escalate`; section
    /// `hook_back` returns to the opening's unit halfway; the last 4 bars
    /// double up; the opening section thins out.
    recut: [Option<Op>; 2],
    recut_from: usize,
    swap_from: usize,
    answer: Option<Op>,
    answer_from: usize,
    escalate: f32,
    hook_back: Option<usize>,
    double_end: bool,
    thin_first: bool,
}

/// How the second pass transforms the first (music theory §5.2: the
/// second drop is a switch-up, the same drums with a re-cut or varied riff).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recipe {
    /// The riff re-cut (reversed, inverted, dotted, rotated) from a section on.
    SwitchUp,
    /// More variants, more fills, pushes before the seams.
    Escalate,
    /// A and B trade places.
    OrderSwap,
    /// The answers change.
    NewAnswers,
    /// A section (the breakdown) cut short: the hook returns halfway.
    HookBack,
    /// The last 4 bars in double time.
    DoubleEnd,
    /// The second pass opens thinned out, then drops.
    ThinDrop,
}

// MUSICAL: tune by ear (user, 2026-10-21: the second 64 bars transform the
// first, Variation sets how far). The recipes and their weights; how far a
// recipe reaches (its extent) at Variation 0 and 1; what every second pass
// adds to the slots and fills.
const RECIPES: [(Recipe, f32); 7] = [
    (Recipe::SwitchUp, 1.5),
    (Recipe::Escalate, 1.5),
    (Recipe::OrderSwap, 1.0),
    (Recipe::NewAnswers, 1.0),
    (Recipe::HookBack, 0.8),
    (Recipe::DoubleEnd, 1.0),
    (Recipe::ThinDrop, 0.7),
];
const SECOND_EXTENT: (f32, f32) = (0.35, 1.0);
const SECOND_SLOT: f32 = 0.25;
const SECOND_EXTRA: f32 = 0.15;
/// Re-cuts of a switch-up, new answers, pushes.
const RECUTS: [Op; 5] = [Op::Retrograde, Op::Inversion, Op::Reverse, Op::RetroInversion, Op::Displacement];
const ANSWERS: [Op; 5] = [Op::Sequence, Op::Inversion, Op::Counter, Op::Passing, Op::CallResponse];
const PUSHES: [Op; 3] = [Op::DoubleTime, Op::Passing, Op::Fragment];

fn pick<T: Copy>(h: u64, from: &[T]) -> T {
    from[rng::below(h, from.len() as u32) as usize]
}

impl Mods {
    /// No recipe.
    fn plain(salt: u32, slot: f32, extra: f32, seam: f32, inner: f32) -> Self {
        Self { salt, slot, extra, seam, inner, recut_from: SECTIONS, swap_from: SECTIONS, answer_from: SECTIONS, ..Self::default() }
    }

    /// Apply `recipe` at extent `e` (0..1), seeded by `h`.
    fn apply(&mut self, recipe: Recipe, e: f32, h: u64) {
        // The sections it reaches back from the end: the last two at a
        // little, the last three in full.
        let from = SECTIONS - 1 - (e * (SECTIONS - 2) as f32).round() as usize;
        match recipe {
            Recipe::SwitchUp => {
                self.recut[0] = Some(pick(h, &RECUTS));
                if e > 0.6 {
                    self.recut[1] = Some(pick(rng::derive(h, 1), &RECUTS));
                }
                self.recut_from = from;
            }
            Recipe::Escalate => {
                self.escalate = 0.3 + 0.6 * e;
                self.slot += 0.15 * e;
                self.extra += 0.2 * e;
            }
            Recipe::OrderSwap => self.swap_from = from,
            Recipe::NewAnswers => {
                self.answer = Some(pick(h, &ANSWERS));
                self.answer_from = from;
            }
            Recipe::HookBack => self.hook_back = Some(1 + rng::below(h, 2) as usize),
            Recipe::DoubleEnd => {
                self.double_end = true;
                self.escalate = self.escalate.max(0.3 * e);
            }
            Recipe::ThinDrop => {
                self.thin_first = true;
                self.extra += 0.1 * e;
            }
        }
    }

    /// Token `t` of section `s` as this pass plays it.
    fn token(&self, mut t: Tok, s: usize) -> Tok {
        if t.letter.class() != SeedClass::Main {
            return t;
        }
        if s >= self.swap_from {
            t.letter = t.letter.partner();
        }
        let ab = (t.letter == Letter::B) as usize;
        if let Some(op) = self.recut[ab].filter(|_| s >= self.recut_from) {
            t.ops.push(op);
        }
        if let Some(op) = self.answer.filter(|_| s >= self.answer_from && t.letter == Letter::B) {
            t.ops.push(op);
        }
        if self.thin_first && s == 0 {
            t.ops.push(Op::Truncation);
        }
        t
    }
}

/// Builds generated forms without allocating (after [`FormBuilder::new`]).
pub struct FormBuilder {
    first: Vec<Segment>,
    second: Vec<Segment>,
    /// The cycle length of each section of the two passes last built.
    cycles: [[u16; SECTIONS]; 2],
    /// The motive length in bars of the two passes last built.
    motives: [u16; 2],
    plan: usize,
    recipes: [Option<Recipe>; 2],
    attempts: u32,
    broken: u32,
    repetition: f32,
    /// The Variation share of the last spec (the repetition rule's ceiling).
    variation: f32,
}

impl Default for FormBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FormBuilder {
    pub fn new() -> Self {
        // Overwrites split a segment at most in two, inserting one more.
        let cap = MAX_PASS_SEGMENTS + 8;
        Self {
            first: Vec::with_capacity(cap),
            second: Vec::with_capacity(cap),
            cycles: [[WINDOW_BEATS; SECTIONS]; 2],
            motives: [16; 2],
            plan: 0,
            recipes: [None; 2],
            attempts: 0,
            broken: 0,
            repetition: 0.0,
            variation: 0.5,
        }
    }

    /// The second pass's recipes of the last form, by name.
    pub fn recipe_names(&self) -> [Option<&'static str>; 2] {
        self.recipes.map(|r| {
            r.map(|r| match r {
                Recipe::SwitchUp => "switch-up",
                Recipe::Escalate => "escalate",
                Recipe::OrderSwap => "order swap",
                Recipe::NewAnswers => "new answers",
                Recipe::HookBack => "hook back",
                Recipe::DoubleEnd => "double end",
                Recipe::ThinDrop => "thin drop",
            })
        })
    }

    /// The motive length in bars of each pass of the last form (D61).
    pub fn motive_bars(&self) -> [u16; 2] {
        self.motives
    }

    /// The cycle length in beats of each 16-bar section of the last form.
    pub fn cycle_beats(&self) -> [[u16; SECTIONS]; 2] {
        self.cycles
    }

    /// The character of the plan the last form was built on.
    pub fn plan_name(&self) -> &'static str {
        PLANS[self.plan].name
    }

    /// Build the form of `spec` into `out` (a form from [`Form::generated`]).
    pub fn build(&mut self, spec: &FormSpec, out: &mut Form) {
        // The rules checked (music theory §5, §6.3); a form that breaks one
        // builds again from the same seeds, its slots, fills and recipe
        // drawn anew, and the attempt that breaks the fewest plays.
        let mut best = (u32::MAX, 0u32);
        for attempt in 0..MAX_ATTEMPTS {
            self.attempt(spec, attempt);
            let broken = self.check();
            if broken.count_ones() < best.0 {
                best = (broken.count_ones(), attempt);
            }
            if broken == 0 {
                break;
            }
        }
        if self.attempts != best.1 {
            self.attempt(spec, best.1);
            self.check();
        }
        out.set_passes(&self.first, &self.second);
        out.set_cycles(self.cycles);
    }

    /// The rules the last form broke ([`Rule`] bits) and the attempt it is.
    pub fn broken(&self) -> (u32, u32) {
        (self.broken, self.attempts)
    }

    fn attempt(&mut self, spec: &FormSpec, attempt: u32) {
        self.attempts = attempt;
        self.variation = share(spec.variation);
        let lane = spec.drums as u8;
        let fill = spec.seeds.fill;
        let salt = |seed: u32| if attempt == 0 { seed } else { rng::hash(seed, lane, attempt as u8, 0, S_ATTEMPT) as u32 };
        self.plan = rng::below(rng::hash(fill, lane, 0, 0, S_PLAN), PLANS.len() as u32) as usize;
        let plan = &PLANS[self.plan];
        // One phrase per letter and pattern for the whole form.
        let lengths = [0u8, 1].map(|v| Letter::ALL.map(|l| letter_phrase(spec, l, v)));
        let v = share(spec.variation);
        let (slot, extra) = (lerp(SLOT_FILL, v), lerp(EXTRA_FILL, v));
        let seam = lerp(SEAM_DRAW, v);
        let inner = lerp(INNER_SEAM, v);
        // The motives come from the seeds alone (snapshotted: a rule broken
        // draws the slots and fills again, never the structure).
        let first_shape = Layout::draw(spec, plan, fill, lane, None);
        let mut first = Mods::plain(salt(fill), slot, extra, seam, inner);
        first.snap = fill;
        first.turn = lerp(TURN_CHANCE, v);
        let head = rng::chance(rng::hash(fill, lane, 0, 0, S_HEAD), HEAD_CHANCE).then_some(plan.head).and_then(tok);
        self.cycles[0] = build_pass(&mut self.first, &first_shape, &lengths, head, &first, lane);
        self.motives[0] = first_shape.bars;
        // The second pass: its own motives (a 64-bar first one answered by
        // its Y; drawn again with a broken rule), one recipe, a second one
        // further up Variation.
        let sec = salt(spec.seeds.second);
        let second_shape = Layout::draw(spec, plan, sec, lane, Some(&first_shape));
        let mut second = Mods::plain(sec, slot + SECOND_SLOT, extra + SECOND_EXTRA, (seam + SECOND_SEAM).min(1.0), (inner + SECOND_SEAM).min(1.0));
        second.snap = fill;
        second.turn = (lerp(TURN_CHANCE, v) + SECOND_TURN).min(1.0);
        second.pass = 1;
        let e = lerp(SECOND_EXTENT, v);
        let weights = RECIPES.map(|r| r.1);
        let a = rng::weighted_pick(rng::hash(sec, lane, 0, 0, S_RECIPE), &weights).unwrap_or(0);
        second.apply(RECIPES[a].0, e, rng::hash(sec, lane, 1, 0, S_RECIPE));
        self.recipes = [Some(RECIPES[a].0), None];
        if rng::chance(rng::hash(sec, lane, 2, 0, S_RECIPE), e - 0.5) {
            let b = (a + 1 + rng::below(rng::hash(sec, lane, 3, 0, S_RECIPE), RECIPES.len() as u32 - 1) as usize) % RECIPES.len();
            second.apply(RECIPES[b].0, e, rng::hash(sec, lane, 4, 0, S_RECIPE));
            self.recipes[1] = Some(RECIPES[b].0);
        }
        self.cycles[1] = build_pass(&mut self.second, &second_shape, &lengths, None, &second, lane);
        self.motives[1] = second_shape.bars;
    }

    /// Check the passes last built; returns the [`Rule`] bits broken.
    fn check(&mut self) -> u32 {
        let (a, b) = (beats(&self.first), beats(&self.second));
        let mut broken = 0;
        let grid = |pass: &[Segment]| {
            let mut at = 0u16;
            for s in pass {
                if s.beats == 0 || at / SECTION_BEATS != (at + s.beats - 1) / SECTION_BEATS {
                    return false;
                }
                at += s.beats;
            }
            at == PASS_BEATS
        };
        if !grid(&self.first) || !grid(&self.second) {
            broken |= Rule::GRID;
        }
        if changes(&a, &self.cycles[0]) > 1 || changes(&b, &self.cycles[1]) > 1 {
            broken |= Rule::CHANGES;
        }
        let rep = repetition(&a);


        self.repetition = rep;
        if !(REPETITION.0..=lerp(REPETITION_CEILING, self.variation)).contains(&rep) {
            broken |= Rule::REPETITION;
        }
        if busy(&b) <= busy(&a) {
            broken |= Rule::BUSIER;
        }
        self.broken = broken;
        broken
    }

    /// The first pass's repetition ([`repetition`]) of the last form.
    pub fn repetition(&self) -> f32 {
        self.repetition
    }
}

/// The rules a generated form keeps, as bits.
pub struct Rule;

impl Rule {
    /// Every pass 64 bars, no segment across a 16-bar seam.
    pub const GRID: u32 = 1;
    /// Inside a section a cycle changes at most once against the one before.
    pub const CHANGES: u32 = 4;
    /// The 4-bar repetition sits in [`REPETITION`] (music theory §6.3).
    pub const REPETITION: u32 = 8;
    /// The second pass is busier than the first.
    pub const BUSIER: u32 = 16;
}

// MUSICAL: tune by ear (music theory §6.3: the reference sets repeat their
// 4-bar blocks 0.33 .. 0.63 in pitch; the structure repeats more than the
// pitches, which follow the chords). The range of the structural
// repetition ([`repetition`]) a first pass keeps: it catches the forms that
// never change and the ones that never repeat (D51: dense rhythm slots in
// the plans of 16-bar cycles, the cells, sit lower; 0.05 still catches the
// ones that never repeat). Attempts before the fewest broken rules win.
const REPETITION: (f32, f32) = (0.05, 0.95);
/// The ceiling at Variation 0 and 1: at Variation 0 a form may repeat
/// almost unchanged (D51: dense rhythm slots copied cycle to cycle).
const REPETITION_CEILING: (f32, f32) = (0.99, REPETITION.1);
const MAX_ATTEMPTS: u32 = 12;

/// One beat of a pass as the checks compare it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Beat {
    letter: u8,
    ops: OpChain,
    marks: u8,
    phrase: u8,
    /// Where in its phrase's notes the beat plays.
    at: u8,
    filler: bool,
}

fn beats(pass: &[Segment]) -> [Beat; PASS_BEATS as usize] {
    let mut out = [Beat::default(); PASS_BEATS as usize];
    let mut i = 0usize;
    for s in pass {
        for k in 0..s.beats {
            if i < out.len() {
                let p = s.phrase.max(1) as u16;
                out[i] = Beat {
                    letter: s.letter.index(),
                    ops: s.ops,
                    marks: s.prime as u8 | (s.swap as u8) << 1,
                    phrase: s.phrase,
                    at: ((s.offset as u16 + s.skip as u16 + k) % p) as u8,
                    filler: s.letter.class() == SeedClass::Filler,
                };
                i += 1;
            }
        }
    }
    out
}

/// The most changes a cycle makes against the one before it inside a
/// section (outside the fills and the seam cycle): runs of main beats on the
/// same letter whose marks differ.
fn changes(pass: &[Beat; PASS_BEATS as usize], cycles: &[u16; SECTIONS]) -> u32 {
    let mut most = 0;
    for (s, &cycle) in cycles.iter().enumerate() {
        let base = s * SECTION_BEATS as usize;
        for w in 1..WINDOWS - 1 {
            let from = base + w * WINDOW_BEATS as usize;
            if from < base + cycle as usize {
                continue;
            }
            let (mut runs, mut inside) = (0, false);
            for i in from..from + WINDOW_BEATS as usize {
                let (x, y) = (pass[i], pass[i - cycle as usize]);
                let differs = !x.filler && !y.filler && x.letter == y.letter && (x.ops, x.marks) != (y.ops, y.marks);
                if differs && !inside {
                    runs += 1;
                }
                inside = differs;
            }
            most = most.max(runs);


        }
    }
    most
}

/// Share of beats that play as the beat one hook period before them
/// (letter, marks, phrase and the place in it), inside each section, at
/// the section's best lag of 4 or 8 bars (music theory §6.3: the best
/// repetition lag is the hook period); the mean over the sections.
fn repetition(pass: &[Beat; PASS_BEATS as usize]) -> f32 {
    let mut sum = 0.0;
    for s in 0..SECTIONS {
        let base = s * SECTION_BEATS as usize;
        let at_lag = |lag: usize| {
            let same = (base + lag..base + SECTION_BEATS as usize).filter(|&i| pass[i] == pass[i - lag]).count();
            same as f32 / (SECTION_BEATS as usize - lag) as f32
        };
        sum += at_lag(WINDOW_BEATS as usize).max(at_lag(2 * WINDOW_BEATS as usize));
    }
    sum / SECTIONS as f32
}

/// The operators the beats play, a fill's beat as one.
fn busy(pass: &[Beat; PASS_BEATS as usize]) -> usize {
    pass.iter().map(|b| b.filler as usize + b.ops.len()).sum()
}


/// A segment of token `t` from `lengths` (per pattern), `beats` long, entering its phrase
/// `skip` beats in.
fn segment(t: &Tok, lengths: &[[Phrase; 4]; 2], beats: u16, skip: u16) -> Segment {
    let p = lengths[t.variant.min(1) as usize][t.letter.index() as usize];
    Segment {
        prime: t.prime,
        swap: t.swap,
        ops: t.ops,
        phrase: p.beats,
        offset: p.offset,
        skip: (skip % p.beats.max(1) as u16) as u8,
        variant: t.variant,
        ..Segment::new(t.letter, beats, 0)
    }
}

/// Lay motive `mo`'s cycle over pass beats `from..to`, again and again from
/// `from`, the last cycle clipped, its tokens as pass `m` plays them in
/// section `s`.
fn lay(out: &mut Vec<Segment>, mo: &Motive, lengths: &[[Phrase; 4]; 2], m: &Mods, s: usize, from: u16, to: u16) {
    let mut cs = from;
    while cs < to {
        let ce = (cs + mo.cycle).min(to);
        let mut at = cs;
        for &(t, beats, skip) in &mo.parts[..mo.n] {
            if at >= ce {
                break;
            }
            let beats = beats.min(ce - at);
            if out.len() < out.capacity() {
                out.push(segment(&m.token(t, s), lengths, beats, skip));
            }
            at += beats;
        }
        cs = ce;
    }
}

/// Put `fill` over pass beats `at..at + fill.beats`: the segments under it
/// shorten, split or go; one cut at its end enters its phrase later.
fn overwrite(out: &mut Vec<Segment>, at: u16, fill: Segment) {
    let end = at + fill.beats;
    let mut i = 0usize;
    let mut start = 0u16;
    let mut insert = None;
    while i < out.len() {
        let s = out[i];
        let (a, b) = (start, start + s.beats);
        start = b;
        if b <= at || a >= end {
            if a >= end && insert.is_none() {
                insert = Some(i);
            }
            i += 1;
            continue;
        }
        let cut = |seg: &mut Segment, by: u16| {
            seg.beats -= by;
            if seg.phrase > 0 {
                seg.skip = ((seg.skip as u16 + by) % seg.phrase as u16) as u8;
            }
        };
        if a < at && b > end {
            // Under the middle: split in two.
            let mut right = s;
            cut(&mut right, end - a);
            out[i].beats = at - a;
            if out.len() + 1 < out.capacity() {
                out.insert(i + 1, right);
            }
            insert = Some(i + 1);
            break;
        } else if a < at {
            out[i].beats = at - a;
            i += 1;
        } else if b > end {
            cut(&mut out[i], end - a);
            insert.get_or_insert(i);
            i += 1;
        } else {
            out.remove(i);
            insert.get_or_insert(i);
        }
    }
    let at_index = insert.unwrap_or(out.len()).min(out.len());
    if out.len() < out.capacity() {
        out.insert(at_index, fill);
    }
}

/// Build one pass of `plan` into `out`; returns each section's cycle.
/// Build one pass of `shape` into `out`; returns each section's cycle.
fn build_pass(out: &mut Vec<Segment>, shape: &Layout, lengths: &[[Phrase; 4]; 2], head: Option<Tok>, m: &Mods, lane: u8) -> [u16; SECTIONS] {
    out.clear();
    let mut cycles = [WINDOW_BEATS; SECTIONS];
    for s in 0..SECTIONS {
        let mo = &shape.motives[shape.order[s]];
        let base = s as u16 * SECTION_BEATS;
        cycles[s] = mo.cycle;
        if m.hook_back == Some(s) {
            // Cut short: the opening's motive comes back halfway.
            let half = base + SECTION_BEATS / 2;
            let hook = &shape.motives[shape.order[0]];
            lay(out, mo, lengths, m, s, base, half);
            lay(out, hook, lengths, m, s, half, base + SECTION_BEATS);
            cycles[s] = mo.cycle.max(hook.cycle).min(SECTION_BEATS / 2);
        } else {
            lay(out, mo, lengths, m, s, base, base + SECTION_BEATS);
        }
        // Extra fills at the 4-bar seams inside the section.
        for w in 1..WINDOWS as u16 {
            let at = base + w * WINDOW_BEATS;
            let h = rng::hash(m.salt, lane, s as u8, w as u8, S_EXTRA);
            if !rng::chance(h, m.extra) {
                continue;
            }
            let letter = if rng::chance(rng::derive(h, 1), EXTRA_D) { Letter::D } else { Letter::C };
            let t = Tok { letter, ..Tok::PLAIN };
            let beats = lengths[0][letter.index() as usize].beats.min(EXTRA_BEATS) as u16;
            overwrite(out, at - beats, segment(&t, lengths, beats, 0));
        }
        // The seam: C at a motive's end, D at the pass's, now and then a
        // short C inside a motive.
        let kind = shape.seam(s);
        if let Some(t) = seam_tok(kind, s, m, lane) {
            let most = SEAM_BEATS[kind as usize] as u16;
            let beats = (lengths[0][t.letter.index() as usize].beats as u16).clamp(1, most);
            overwrite(out, base + SECTION_BEATS - beats, segment(&t, lengths, beats, 0));
        }
    }
    // The pickup the form opens with.
    if let Some(t) = head {
        let beats = lengths[0][t.letter.index() as usize].beats.min(HEAD_BEATS) as u16;
        overwrite(out, 0, segment(&t, lengths, beats, 0));
    }
    // The last 4 bars doubled up.
    if m.double_end {
        let mut at = 0u16;
        for seg in out.iter_mut() {
            if at >= PASS_BEATS - WINDOW_BEATS && seg.letter.class() == SeedClass::Main {
                seg.ops.push(Op::DoubleTime);
            }
            at += seg.beats;
        }
    }
    resolve(out, &cycles, m, lane);
    turnarounds(out, shape, m, lane);
    merge(out);
    cycles
}

/// Turn the line around before the seams (after the slots, so the one-change
/// rule leaves it): the last main part before a motive's or the pass's seam
/// plays an operator at chance `m.turn`, an inner seam's at half of it; the
/// pass's end now and then a push on top. Which is the fill seed's per seam
/// and pass (snapshotted).
fn turnarounds(out: &mut Vec<Segment>, shape: &Layout, m: &Mods, lane: u8) {
    let pool: &[Op] = if lane == 1 { &DRUM_TURNS } else { &TURNS };
    for s in 0..SECTIONS {
        let kind = shape.seam(s);
        let h = rng::hash(m.snap, lane, s as u8, m.pass, S_TURN);
        let chance = if kind == Seam::Inner { m.turn / 2.0 } else { m.turn };
        if !rng::chance(h, chance) {
            continue;
        }
        let end = (s as u16 + 1) * SECTION_BEATS;
        let mut at = 0u16;
        let mut last = None;
        for (i, seg) in out.iter().enumerate() {
            if at >= end {
                break;
            }
            if seg.letter.class() == SeedClass::Main && at >= end - SECTION_BEATS {
                last = Some((i, at));
            }
            at += seg.beats;
        }
        let Some((mut i, _)) = last else { continue };
        // Only its last bar turns: a longer part splits there.
        let seg = out[i];
        if seg.beats > TURN_BEATS && out.len() < out.capacity() {
            let mut tail = seg;
            tail.beats = TURN_BEATS;
            if seg.phrase > 0 {
                tail.skip = ((seg.skip as u16 + seg.beats - TURN_BEATS) % seg.phrase as u16) as u8;
            }
            out[i].beats -= TURN_BEATS;
            out.insert(i + 1, tail);
            i += 1;
        }
        let op = pick(rng::derive(h, 1), pool);
        if !out[i].ops.contains(op) {
            out[i].ops.push(op);
        }
        if kind == Seam::Pass && rng::chance(rng::derive(h, 2), PASS_PUSH) {
            let push = pick(rng::derive(h, 3), &PUSHES);
            if !out[i].ops.contains(push) {
                out[i].ops.push(push);
            }
        }
    }
}

/// The fill at the end of section `s`, a seam of `kind`: the plain C or D,
/// or at chance `m.seam` a kind from its pool; an inner one only at chance
/// `m.inner`.
fn seam_tok(kind: Seam, s: usize, m: &Mods, lane: u8) -> Option<Tok> {
    let h = rng::hash(m.salt, lane, s as u8, 0, S_SEAM);
    if kind == Seam::Inner && !rng::chance(rng::derive(h, 2), m.inner) {
        return None;
    }
    let (plain, pool) = if kind == Seam::Pass { ("D", D_KINDS) } else { ("C", C_KINDS) };
    if !rng::chance(h, m.seam) {
        return tok(plain);
    }
    tok(pick(rng::derive(h, 1), pool))
}

/// A motive (D61): its cycle, the parts that fill it (token, beats, beats
/// into its phrase it enters).
#[derive(Debug, Clone, Copy)]
struct Motive {
    cycle: u16,
    parts: [(Tok, u16, u16); 3],
    n: usize,
}

/// A pass's motives: X and Y, their length and which plays in each section.
#[derive(Debug, Clone, Copy)]
struct Layout {
    motives: [Motive; 2],
    order: [usize; SECTIONS],
    bars: u16,
}

impl Layout {
    /// The motives of `plan` from `seed`. The second pass after `first`: a
    /// 64-bar X is answered by Y all through, else it draws its own.
    fn draw(spec: &FormSpec, plan: &Plan, seed: u32, lane: u8, first: Option<&Layout>) -> Self {
        let h = |k: u8, stream: u32| rng::hash(seed, lane, k, 0, stream);
        let bars = MOTIVE_BARS[rng::weighted_pick(h(0, S_MOTIVE), &MOTIVE_BARS.map(|m| m.1)).unwrap_or(0)].0;
        let alternate = rng::chance(h(1, S_MOTIVE), plan.alternate);
        let (bars, order) = match (first.map(|f| f.bars), bars) {
            (Some(64), _) => (64, [1; SECTIONS]),
            (_, 16) if alternate => (16, [0, 1, 0, 1]),
            (_, 64) => (64, [0; SECTIONS]),
            (_, b) => (b, [0, 0, 1, 1]),
        };
        let (cycle, a, y_a) = if spec.drums {
            // Mostly A alone; now and then a short B at the cycle's end.
            let cycle = DRUM_CYCLES[rng::weighted_pick(h(0, S_CYCLE), &DRUM_CYCLES.map(|c| c.1)).unwrap_or(1)].0;
            let b = ((cycle as f32 * DRUM_B_SHARE) as u16 / 2 * 2).max(2);
            let a_of = |k: u8| if rng::chance(h(k, S_SPLIT), DRUM_B) { cycle - b } else { cycle };
            (cycle, a_of(0), a_of(1))
        } else {
            let tilt = SHAPE_TILT * (2.0 * share(spec.shape) - 1.0);
            let weights = CYCLE_LENGTHS.map(|(len, w)| w * 2f32.powf(tilt * (len as f32 / 16.0).log2()));
            let cycle = CYCLE_LENGTHS[rng::weighted_pick(h(0, S_CYCLE), &weights).unwrap_or(1)].0;
            let a = split(cycle, h(0, S_SPLIT));
            let y_a = if rng::chance(h(1, S_SPLIT), plan.flip) { cycle - a } else { split(cycle, h(2, S_SPLIT)) };
            (cycle, a, y_a)
        };
        let sound = rng::chance(h(0, S_SOUND), plan.sound * share(spec.contrast));
        let motives = [
            motive(plan, 0, cycle, a, h(0, S_SHAPE), false, spec.drums),
            motive(plan, 1, cycle, y_a, h(1, S_SHAPE), sound, spec.drums),
        ];
        Self { motives, order, bars }
    }

    /// The seam section `s` ends at.
    fn seam(&self, s: usize) -> Seam {
        let end = (s as u16 + 1) * SECTION_BEATS / BEATS_PER_BAR;
        if s + 1 == SECTIONS {
            Seam::Pass
        } else if end % self.bars == 0 {
            Seam::Motive
        } else {
            Seam::Inner
        }
    }
}

/// A's beats of a `cycle` (D61): a share the seed picks ([`SPLITS`]).
fn split(cycle: u16, h: u64) -> u16 {
    let share_a = SPLITS[rng::weighted_pick(h, &SPLITS.map(|s| s.1)).unwrap_or(0)].0;
    ((cycle as f32 * share_a).round() as u16).clamp(1, cycle - 1)
}

/// Motive `which` (0 X, 1 Y) of `plan`: a cycle of `cycle` beats, A's share
/// `a`, its shape from `h`; Y plays the letters' second patterns, with the
/// lane's other sound when `sound`.
fn motive(plan: &Plan, which: usize, cycle: u16, a: u16, h: u64, sound: bool, drums: bool) -> Motive {
    // The drums: A alone, or A and a short B ([`DRUM_B`]).
    let shape: &[u8] = match (drums, a >= cycle) {
        (true, true) => b"A",
        (true, false) => b"AB",
        _ => pick(h, plan.shapes).as_bytes(),
    };
    let marks = if which == 0 { plan.x } else { plan.y };
    let beats_of = |l: Letter| if l == Letter::A { a } else { cycle - a };
    let count = |l: Letter| shape.iter().filter(|&&c| letter_of(c) == Some(l)).count() as u16;
    let mut mo = Motive { cycle, parts: [(Tok::PLAIN, 0, 0); 3], n: 0 };
    // A letter twice in the cycle splits its beats and goes on in its
    // phrase the second time.
    let mut played = [0u16; 2];
    for &c in shape.iter().take(3) {
        let Some(l) = letter_of(c) else { continue };
        let k = (l == Letter::B) as usize;
        let total = beats_of(l);
        let step = if drums { 2 } else { 1 };
        let beats = if count(l) > 1 && played[k] == 0 { ((total + step) / 2 / step * step).min(total) } else { total - played[k] };
        if beats == 0 {
            continue;
        }
        let mut t = Tok { letter: l, variant: which as u8, swap: sound, ..Tok::PLAIN };
        apply_marks(&mut t, marks[k]);
        mo.parts[mo.n] = (t, beats, played[k]);
        mo.n += 1;
        played[k] += beats;
    }
    mo
}

fn letter_of(c: u8) -> Option<Letter> {
    match c {
        b'A' => Some(Letter::A),
        b'B' => Some(Letter::B),
        _ => None,
    }
}

/// Join repeats that continue each other (the same phrase and marks, the
/// next entering where the last left off) inside a section: the form reads
/// shorter and plays the same.
fn merge(out: &mut Vec<Segment>) {
    let (mut kept, mut at, mut last_start) = (0usize, 0u16, 0u16);
    for i in 0..out.len() {
        let s = out[i];
        if kept > 0 {
            let p = &mut out[kept - 1];
            let same = (p.letter, p.ops, p.prime, p.swap, p.phrase, p.offset, p.play) == (s.letter, s.ops, s.prime, s.swap, s.phrase, s.offset, s.play);
            let continues = p.phrase > 0 && (p.skip as u16 + p.beats) % p.phrase as u16 == s.skip as u16;
            if same && continues && last_start / SECTION_BEATS == (at + s.beats - 1) / SECTION_BEATS {
                p.beats += s.beats;
                at += s.beats;
                continue;
            }
        }
        out[kept] = s;
        kept += 1;
        last_start = at;
        at += s.beats;
    }
    out.truncate(kept);
}

/// Fill the slots and keep the rules (music theory §5): a slot plays at
/// Variation's chance an operator by its place (or none); inside a section
/// each cycle repeats the one before but for at most one change. (D57: the
/// rhythm is the chain's; the form places no rhythm operators.)
fn resolve(out: &mut [Segment], cycles: &[u16; SECTIONS], m: &Mods, lane: u8) {
    let mut starts = [0u16; MAX_PASS_SEGMENTS + 8];
    let mut at = 0u16;
    for (i, s) in out.iter().enumerate() {
        starts[i] = at;
        at += s.beats;
    }
    let n = out.len();
    for i in 0..n {
        let start = starts[i];
        let end = start + out[i].beats;
        let next = out.get(i + 1).copied();
        let place = Place {
            period_end: end % WINDOW_BEATS == 0,
            repeat: i > 0 && out[i - 1].letter == out[i].letter,
            before_change: end % SECTION_BEATS == 0 || next.is_some_and(|x| x.letter.class() != out[i].letter.class()),
        };
        let h = rng::hash(m.salt, lane, (start / BEATS_PER_BAR) as u8, (start % 256) as u8, S_SLOT);
        // Which operator is fixed by the seed (user, 2026-10-21: snapshotted,
        // not drawn again per slot): per letter and kind of place. The knobs
        // only decide how many of the slots play it.
        let kind = place.before_change as u8 | (place.period_end as u8) << 1 | (place.repeat as u8) << 2;
        let snap = rng::hash(m.snap, lane, 1 + out[i].letter.index(), kind, S_SNAP);
        let mut chain = OpChain::default();
        for (k, op) in out[i].ops.iter().enumerate() {
            if op != Op::Seeded {
                chain.push(op);
                continue;
            }
            if rng::chance(rng::derive(h, k as u32), m.slot) {
                let push = rng::chance(rng::derive(snap, 2 + 4 * k as u32), m.escalate);
                chain.push(if push { pick(rng::derive(snap, 3 + 4 * k as u32), &PUSHES) } else { ops::seeded_melodic(rng::derive(snap, 1 + 4 * k as u32), place) });
            }
        }
        out[i].ops = chain;
    }
    // One change per cycle against the one before it, in the section.
    let mut changed = [false; SECTIONS * WINDOWS];
    for i in 0..n {
        let (start, seg) = (starts[i], out[i]);
        let s = (start / SECTION_BEATS) as usize;
        let w = ((start % SECTION_BEATS) / WINDOW_BEATS) as usize;
        let cycle = cycles[s.min(SECTIONS - 1)];
        if seg.letter.class() == SeedClass::Filler || w + 1 == WINDOWS || start % SECTION_BEATS < cycle {
            continue;
        }
        // The segment of its letter one cycle before (under its beats, when a
        // fill or the pickup moved the starts).
        let back = start - cycle;
        let Some(j) = (0..i).find(|&j| out[j].letter == seg.letter && starts[j] < back + seg.beats && back < starts[j] + out[j].beats) else { continue };
        let prev = out[j];
        if (prev.ops, prev.prime, prev.swap) == (seg.ops, seg.prime, seg.swap) {
            continue;
        }
        let k = s * WINDOWS + w;
        if changed[k] {
            (out[i].ops, out[i].prime, out[i].swap) = (prev.ops, prev.prime, prev.swap);
        } else {
            changed[k] = true;
        }
    }
}

// MUSICAL: tune by ear (user, 2026-10-21: a Splice follower knows the
// host's form and reacts by the slider: stacked near the same structure
// with some variation, the fills together; in the gaps a contrary one, its
// fills where the host plays, its changes where the host holds). Chance a
// 4-bar window takes the host's at full layer; chances of the counter
// moves at full counter.
const LAYER_COPY: f32 = 0.85;
const COUNTER_MOVE: f32 = 0.8;
// (user, 2026-10-21: Splice sounded like volume automation, it should change
// the pattern.) Chance a 4-bar window plays the other main letter where the
// host plays one (B against A, A against B), at full counter, and towards
// the middle (half of it at relation 0.5).
const COUNTER_SWAP: f32 = 0.7;
/// Beats at a window's end a fill counts in, and a counter fill's length.
const FILL_ZONE: u16 = 4;
const COUNTER_FILL_BEATS: u16 = 2;

impl FormBuilder {
    /// Build a Splice follower's form into `out` from the host's form and
    /// its own (D48), at `relation` (−1 layer .. 1 counter), seeded by
    /// `seed`. Each 4-bar window is the host's or the follower's own, moved:
    /// towards the layer end it takes the host's more often; towards the
    /// counter end its fills leave the host's and go where the host plays,
    /// its variants leave the host's and go where the host holds. Drum
    /// followers keep even offsets (the backbeat). RT-safe.
    pub fn follow(&mut self, host: &Form, own: &Form, relation: f32, seed: u32, drums: bool, out: &mut Form) {
        let full = |segs: &[Segment]| segs.iter().map(|s| s.beats as u32).sum::<u32>() == PASS_BEATS as u32;
        let (own_first, own_second) = if full(own.pattern()) && full(own.second_pass()) { (own.pattern(), own.second_pass()) } else { (host.pattern(), host.second_pass()) };
        derive(&mut self.first, host.pattern(), own_first, relation, seed, drums, 0);
        derive(&mut self.second, host.second_pass(), own_second, relation, seed, drums, 1);
        out.set_passes(&self.first, &self.second);
    }
}

/// Push the parts of `pass` in beats `from..to` onto `out`.
fn slice(pass: &[Segment], from: u16, to: u16, out: &mut Vec<Segment>) {
    let mut at = 0u16;
    for s in pass {
        let (a, b) = (at, at + s.beats);
        at = b;
        if b <= from || a >= to {
            continue;
        }
        let (lo, hi) = (a.max(from), b.min(to));
        let mut seg = Segment { beats: hi - lo, ..*s };
        if seg.phrase > 0 {
            seg.skip = ((seg.skip as u16 + lo - a) % seg.phrase as u16) as u8;
        }
        if out.len() < out.capacity() {
            out.push(seg);
        }
    }
}

/// The window's (beats `from..from + WINDOW_BEATS` of `pass`) fill at its
/// end, and whether it plays a variant (an operator but `q` and `a`).
fn window_shape(pass: &[Segment], from: u16) -> (bool, bool) {
    let (mut fill, mut varied, mut at) = (false, false, 0u16);
    let to = from + WINDOW_BEATS;
    for s in pass {
        let (a, b) = (at, at + s.beats);
        at = b;
        if b <= from || a >= to {
            continue;
        }
        fill |= s.letter.class() == SeedClass::Filler && b > to - FILL_ZONE;
        varied |= s.letter.class() == SeedClass::Main && s.ops.iter().any(|o| !matches!(o, Op::Question | Op::Answer));
    }
    (fill, varied)
}

fn derive(out: &mut Vec<Segment>, host: &[Segment], own: &[Segment], relation: f32, seed: u32, drums: bool, pass: u8) {
    out.clear();
    let windows = PASS_BEATS / WINDOW_BEATS;
    for w in 0..windows {
        let from = w * WINDOW_BEATS;
        let to = from + WINDOW_BEATS;
        let h = rng::hash(seed, drums as u8, w as u8, pass, S_FOLLOW);
        let start = out.len();
        if relation < 0.0 && rng::chance(h, -relation * LAYER_COPY) {
            slice(host, from, to, out);
            continue;
        }
        slice(own, from, to, out);
        if relation <= 0.0 {
            continue;
        }
        let p = relation * COUNTER_MOVE;
        if rng::chance(rng::derive(h, 6), relation * COUNTER_SWAP) {
            // Answer the host: its A meets this lane's B and the other way
            // round, each with its own phrase as the lane's form has it.
            let phrase_of = |l: Letter| own.iter().find(|s| s.letter == l).map(|s| (s.phrase, s.offset));
            for s in out[start..].iter_mut() {
                let other = match s.letter {
                    Letter::A => Letter::B,
                    Letter::B => Letter::A,
                    _ => continue,
                };
                if let Some((phrase, offset)) = phrase_of(other) {
                    s.letter = other;
                    if s.phrase > 0 && phrase > 0 {
                        s.skip %= phrase;
                        (s.phrase, s.offset) = (phrase, offset);
                    }
                }
            }
        }
        let ((host_fill, host_varied), (own_fill, own_varied)) = (window_shape(host, from), window_shape(own, from));
        let window = &mut out[start..];
        if host_fill && own_fill && rng::chance(rng::derive(h, 1), p) {
            // The host fills: hold the line through it.
            let main = window.iter().rev().find(|s| s.letter.class() == SeedClass::Main).copied();
            if let Some(m) = main {
                for s in window.iter_mut().filter(|s| s.letter.class() == SeedClass::Filler) {
                    *s = Segment { beats: s.beats, ..m };
                }
            }
        } else if !host_fill && !own_fill && rng::chance(rng::derive(h, 2), p) {
            // The host plays on: fill here instead.
            let c = own.iter().find(|s| s.letter == Letter::C).map_or(COUNTER_FILL_BEATS as u8, |s| s.phrase);
            let fill = Segment { phrase: c.max(1), ..Segment::new(Letter::C, COUNTER_FILL_BEATS, 0) };
            overwrite(out, to - COUNTER_FILL_BEATS, fill);
        }
        let window = &mut out[start..];
        if host_varied && own_varied && rng::chance(rng::derive(h, 3), p) {
            // The host changes: hold.
            for s in window.iter_mut() {
                let mut kept = OpChain::default();
                for o in s.ops.iter().filter(|o| matches!(o, Op::Question | Op::Answer)) {
                    kept.push(o);
                }
                s.ops = kept;
            }
        } else if !host_varied && !own_varied && rng::chance(rng::derive(h, 4), p) {
            // The host holds: change here.
            if let Some(s) = window.iter_mut().rev().find(|s| s.letter.class() == SeedClass::Main) {
                s.ops.push(ops::seeded(rng::derive(h, 5), Place { period_end: true, ..Place::default() }));
            }
        }
    }
    if drums {
        for s in out.iter_mut() {
            s.offset = 0;
            s.skip &= !1;
        }
    }
    merge(out);
}

// RNG streams (D48).

const S_PHRASE: u32 = 0xF80;
const S_LENGTH: u32 = 0xF81;
const S_OFFSET: u32 = 0xF82;
const S_PLAN: u32 = 0xF83;
const S_EXTRA: u32 = 0xF86;
const S_HEAD: u32 = 0xF87;
const S_SLOT: u32 = 0xF88;
const S_RECIPE: u32 = 0xF89;
const S_ATTEMPT: u32 = 0xF8A;
const S_FOLLOW: u32 = 0xF8B;
const S_B_STEP: u32 = 0xF8C;
const S_SEAM: u32 = 0xF8D;
const S_SNAP: u32 = 0xF90;
const S_MOTIVE: u32 = 0xF91;
const S_CYCLE: u32 = 0xF92;
const S_SPLIT: u32 = 0xF93;
const S_SOUND: u32 = 0xF94;
const S_SHAPE: u32 = 0xF95;
const S_TURN: u32 = 0xF96;




#[cfg(test)]
mod tests {
    use super::*;

    fn built(spec: &FormSpec) -> Form {
        built_with(spec).0
    }

    fn built_with(spec: &FormSpec) -> (Form, [[u16; SECTIONS]; 2]) {
        let mut f = Form::generated();
        let mut b = FormBuilder::new();
        b.build(spec, &mut f);
        (f, b.cycles)
    }

    fn spec(seed: u32, variation: u8, shape: u8) -> FormSpec {
        FormSpec { seeds: FormSeeds::new(seed, seed.wrapping_mul(7).wrapping_add(1), seed ^ 0x55), variation, shape, contrast: 2, drums: false }
    }

    #[test]
    fn phrases_are_powers_of_two_in_beats_with_an_offset_inside() {
        for seed in 0..400 {
            for l in Letter::ALL {
                for drums in [false, true] {
                    let p = phrase(seed, l, 0, 2, drums);
                    let max = if l.class() == SeedClass::Main { 16 } else { 8 };
                    assert!(p.beats.is_power_of_two() && p.beats <= max && p.beats >= 1, "{p:?}");
                    assert!(p.offset < p.beats);
                    assert_eq!(p.offset % 2, 0, "even beats keep the kick's beats");
                    if drums {
                        assert_eq!(p.offset, 0, "the drums have no offset");
                    }
                }
            }
        }
    }

    #[test]
    fn b_answers_a_with_its_length_by_contrast() {
        let mut off = [0u32; BANDS as usize];
        for seed in 0..500u32 {
            for shape in 0..BANDS {
                let a = phrase(seed, Letter::A, 0, shape, false);
                for c in 0..BANDS {
                    let b = answer_phrase(a, seed ^ 0x9E37, 0, c, false);
                    assert!(b.beats.is_power_of_two() && (2..=16).contains(&b.beats) && b.offset < b.beats);
                    assert!(b.beats == a.beats || b.beats == a.beats * 2 || b.beats * 2 == a.beats, "one step at most");
                    off[c as usize] += (b.beats != a.beats) as u32;
                }
            }
        }
        assert_eq!(off[0], 0, "at Contrast 0 B's phrase is as long as A's");
        assert!(off.windows(2).all(|w| w[1] > w[0]), "{off:?}");
        // In built forms: at Contrast 0 B's phrases are A's length in each section.
        for seed in 0..100 {
            let f = built(&FormSpec { contrast: 0, ..spec(seed, (seed % 5) as u8, (seed / 5 % 5) as u8) });
            let mut at = 0u16;
            let mut len = [[0u8; 2]; SECTIONS];
            for s in f.pattern() {
                let sec = (at / SECTION_BEATS) as usize;
                match s.letter {
                    Letter::A => len[sec][0] = s.phrase,
                    Letter::B => len[sec][1] = s.phrase,
                    _ => {}
                }
                at += s.beats;
            }
            for l in len.iter().filter(|l| l[0] > 0 && l[1] > 0) {
                assert_eq!(l[0], l[1], "seed {seed}: {f}");
            }
        }
    }

    #[test]
    fn shape_tilts_the_phrase_lengths() {
        let mean = |shape: u8| (0..2000u32).map(|s| phrase(s, Letter::A, 0, shape, false).beats as f32).sum::<f32>() / 2000.0;
        let m: Vec<f32> = (0..BANDS).map(mean).collect();
        assert!(m.windows(2).all(|w| w[1] > w[0]), "the mean phrase grows with Shape: {m:?}");
        assert!(m[0] < 5.0 && m[4] > 8.0, "{m:?}");
    }

    #[test]
    fn every_form_keeps_the_16_bar_grid() {
        for seed in 0..300 {
            for drums in [false, true] {
                let f = built(&FormSpec { drums, ..spec(seed, (seed % 5) as u8, (seed / 5 % 5) as u8) });
                for pass in [f.pattern(), f.second_pass()] {
                    let mut at = 0u16;
                    let mut sections = 0;
                    for s in pass {
                        assert!(s.beats > 0 && s.phrase > 0 && s.phrase.is_power_of_two(), "{f}");
                        assert!(s.skip < s.phrase && s.offset < s.phrase);
                        if at % SECTION_BEATS == 0 {
                            sections += 1;
                        }
                        // No segment crosses a section seam.
                        assert_eq!(at / SECTION_BEATS, (at + s.beats - 1) / SECTION_BEATS, "seed {seed}: {f}");
                        at += s.beats;
                    }
                    assert_eq!((at, sections), (PASS_BEATS, 4), "seed {seed}: {f}");
                }
                // It prints and parses back.
                assert_eq!(Form::parse(&f.to_string()).unwrap().to_string(), f.to_string());
            }
        }
    }

    #[test]
    fn forms_differ_by_seed_and_fills_sit_at_the_seams() {
        let mut texts = std::collections::BTreeSet::new();
        let mut at_seams = 0;
        for seed in 0..100 {
            let f = built(&spec(seed, 2, 2));
            texts.insert(f.to_string());
            let mut at = 0u16;
            for s in f.pattern() {
                if s.letter.class() == SeedClass::Filler && (at + s.beats) % SECTION_BEATS == 0 {
                    at_seams += 1;
                }
                at += s.beats;
            }
        }
        assert_eq!(texts.len(), 100, "every seed its own form");
        assert!(at_seams >= 250, "fills end the sections: {at_seams}");
    }

    /// The fill ending at each section end of `pass` (letter, its marks, beats).
    fn seam_fills(pass: &[Segment]) -> [Option<(Letter, bool, OpChain, u16)>; SECTIONS] {
        let mut out = [None; SECTIONS];
        let mut at = 0u16;
        for s in pass {
            at += s.beats;
            if s.letter.class() == SeedClass::Filler && at % SECTION_BEATS == 0 {
                out[(at / SECTION_BEATS - 1) as usize] = Some((s.letter, s.swap, s.ops, s.beats));
            }
        }
        out
    }

    #[test]
    fn c_ends_each_motive_and_d_the_pass() {
        // User, 2026-10-26: C a short fill at a motive's end, D a longer one
        // at the pass's, each its own pattern; inner seams rare and brief.
        let mut b = FormBuilder::new();
        let mut f = Form::generated();
        let (mut kinds, mut last_d, mut ends, mut inner) = (std::collections::BTreeSet::new(), 0u32, (0u32, 0u32), (0u32, 0u32, 0u32));
        for seed in 0..200 {
            b.build(&spec(seed, 2, 2), &mut f);
            let bars = b.motive_bars()[0];
            let fills = seam_fills(f.pattern());
            last_d += fills[SECTIONS - 1].is_some_and(|x| x.0 == Letter::D) as u32;
            for (s, fill) in fills.iter().enumerate().take(SECTIONS - 1) {
                let end = (s as u16 + 1) * 16;
                if end % bars == 0 {
                    ends.0 += 1;
                    let x = fill.unwrap_or_else(|| panic!("seed {seed}: no fill at bar {end}: {f}"));
                    assert_eq!(x.0, Letter::C, "seed {seed}: {f}");
                    assert!(x.3 <= SEAM_BEATS[1] as u16);
                    kinds.insert(format!("{:?}", (x.1, x.2)));
                    ends.1 += x.3 as u32;
                } else {
                    inner.0 += 1;
                    if let Some(x) = fill {
                        assert!(x.3 <= SEAM_BEATS[0] as u16 || x.0 == Letter::D, "seed {seed}: {f}");
                        inner.1 += 1;
                        inner.2 += x.3 as u32;
                    }
                }
            }
        }
        assert!(last_d >= 195, "the pass ends on D: {last_d} of 200");
        assert!(kinds.len() >= 4, "the motive seams take several kinds: {kinds:?}");
        assert!(inner.1 * 2 < inner.0, "inner seams fill rarely: {inner:?}");
        assert!(inner.2 * ends.0 < ends.1 * inner.1.max(1), "and shorter: {inner:?} vs {ends:?}");
    }

    #[test]
    fn the_seed_picks_the_split_and_contrast_leaves_it() {
        // User, 2026-10-26: Contrast does not move the split.
        let mut shares = std::collections::BTreeSet::new();
        for cycle in [8u16, 16, 32, 64] {
            for seed in 0..300u32 {
                let a = split(cycle, rng::hash(seed, 0, 0, 0, 1));
                assert!(a > 0 && a < cycle, "{cycle} {a}");
                shares.insert(a * 8 / cycle);
            }
        }
        assert_eq!(shares.len(), SPLITS.len(), "{shares:?}");
        for seed in 0..100 {
            let parts = |contrast: u8| {
                let l = Layout::draw(&FormSpec { contrast, ..spec(seed, 2, 2) }, &PLANS[seed as usize % PLANS.len()], seed, 0, None);
                l.motives.map(|m| m.parts[..m.n].iter().map(|p| p.1).collect::<Vec<_>>())
            };
            assert_eq!(parts(0), parts(BANDS - 1), "seed {seed}");
        }
    }

    #[test]
    fn the_line_turns_around_before_the_seams() {
        // User, 2026-10-26: more operators and variety in the line at the
        // seams. The main part before a motive's or the pass's seam varies.
        let mut b = FormBuilder::new();
        let mut f = Form::generated();
        let (mut turned, mut seams, mut ops) = (0, 0, std::collections::BTreeSet::new());
        for seed in 0..200 {
            b.build(&spec(seed, BANDS - 1, 2), &mut f);
            let bars = b.motive_bars()[0];
            let mut at = 0u16;
            let mut last: Option<Segment> = None;
            for s in f.pattern() {
                if s.letter.class() == SeedClass::Main {
                    last = Some(*s);
                }
                at += s.beats;
                let end = at / BEATS_PER_BAR;
                if at % SECTION_BEATS == 0 && end % bars == 0 {
                    seams += 1;
                    if let Some(l) = last.filter(|l| !l.ops.is_empty()) {
                        turned += 1;
                        ops.extend(l.ops.iter().map(|o| o.as_char()));
                    }
                }
            }
        }
        assert!(turned * 10 > seams * 8, "{turned} of {seams} seams turn around");
        assert!(ops.len() >= 8, "{ops:?}");
    }

    #[test]
    fn the_drums_play_long_a_and_b_seldom() {
        // User, 2026-10-26: the drums much longer on A, B sparingly or not
        // at all most of the time.
        let mut b = FormBuilder::new();
        let mut f = Form::generated();
        let (mut a_beats, mut b_beats, mut without_b) = (0u32, 0u32, 0);
        for seed in 0..200 {
            b.build(&FormSpec { drums: true, ..spec(seed, 2, 2) }, &mut f);
            let mut has_b = false;
            for s in f.pattern() {
                match s.letter {
                    Letter::A => a_beats += s.beats as u32,
                    Letter::B => {
                        b_beats += s.beats as u32;
                        has_b = true;
                    }
                    _ => {}
                }
            }
            without_b += !has_b as u32;
            assert!(b.cycle_beats()[0].iter().all(|c| *c >= 32), "seed {seed}: {f}");
        }
        assert!(b_beats * 12 < a_beats, "B {b_beats} of {} main beats", a_beats + b_beats);
        assert!(without_b > 80, "{without_b} of 200 first passes without B");
    }


    /// The main beats of section `s` of `pass`: letter, pattern, place in its phrase.
    fn section_mains(pass: &[Segment], s: usize) -> Vec<(Letter, u8, u8)> {
        let mut out = Vec::new();
        let mut at = 0u16;
        for seg in pass {
            for k in 0..seg.beats {
                if (at + k) / SECTION_BEATS == s as u16 {
                    let p = seg.phrase.max(1) as u16;
                    out.push(if seg.letter.class() == SeedClass::Main { (seg.letter, seg.variant, ((seg.skip as u16 + k) % p) as u8) } else { (Letter::C, 0, 0) });
                }
            }
            at += seg.beats;
        }
        out
    }

    #[test]
    fn motives_lay_out_by_their_length() {
        // User, 2026-10-26: 16-bar motives X Y X Y (or X X Y Y), 32 X Y, 64
        // X with Y in the second pass; Y plays the second patterns.
        let mut b = FormBuilder::new();
        let mut f = Form::generated();
        let mut seen = std::collections::BTreeMap::<u16, usize>::new();
        // Over the beats both play a main letter (the fills are drawn apart).
        let same = |x: &[(Letter, u8, u8)], y: &[(Letter, u8, u8)]| {
            let both: Vec<_> = x.iter().zip(y).filter(|(a, b)| a.0 != Letter::C && b.0 != Letter::C).collect();
            both.iter().filter(|(a, b)| a == b).count() as f32 / both.len().max(1) as f32
        };
        for seed in 0..300 {
            b.build(&spec(seed, 1, 2), &mut f);
            let [first, second] = b.motive_bars();
            *seen.entry(first).or_default() += 1;
            let variant = |s: usize, pass: &[Segment]| section_mains(pass, s).iter().find(|m| m.0 != Letter::C).map(|m| m.1);
            let v: Vec<_> = (0..SECTIONS).map(|s| variant(s, f.pattern())).collect();
            match first {
                16 => {
                    assert!(v == [Some(0), Some(1), Some(0), Some(1)] || v == [Some(0), Some(0), Some(1), Some(1)], "seed {seed}: {v:?} {f}");
                    let (a, c) = (section_mains(f.pattern(), 0), section_mains(f.pattern(), if v[1] == Some(0) { 1 } else { 2 }));
                    assert!(same(&a, &c) > 0.75, "seed {seed}: X comes back: {f}");
                }
                32 => assert_eq!(v, [Some(0), Some(0), Some(1), Some(1)], "seed {seed}: {f}"),
                _ => {
                    assert_eq!(v, [Some(0); 4], "seed {seed}: {f}");
                    assert_eq!(second, 64);
                    let w: Vec<_> = (0..SECTIONS).map(|s| variant(s, f.second_pass())).collect();
                    assert_eq!(w, [Some(1); 4], "seed {seed}: Y answers in the second pass: {f}");
                }
            }
        }
        assert!(seen.len() == 3 && seen.values().all(|n| *n > 20), "{seen:?}");
    }

    #[test]
    fn variation_fills_more_slots_and_fills() {
        let count = |variation: u8| {
            let mut n = 0;
            for seed in 0..200 {
                let f = built(&spec(seed, variation, 2));
                // The variants and the fills.
                n += f.pattern().iter().chain(f.second_pass()).filter(|s| !s.ops.is_empty() || s.letter.class() == SeedClass::Filler).count();
            }
            n
        };
        let (low, high) = (count(0), count(BANDS - 1));
        assert!(low > 0 && high as f32 > 1.15 * low as f32, "variants and fills: {low} at Variation 0, {high} at 1");
    }

    #[test]
    fn forms_may_vary_from_bar_one() {
        // No opening rule: the first 8 bars may carry operators.
        let opening = (0..200).filter(|&seed| {
            let f = built(&spec(seed, 4, 2));
            (0..32u64).any(|b| !f.pos_at(b).ops.is_empty())
        }).count();
        assert!(opening > 60, "{opening} of 200 forms vary their first 8 bars");
    }

    /// Beats where the two passes of `f` differ (letter, marks or phrase).
    fn differing_beats(f: &Form) -> usize {
        (0..PASS_BEATS as u64)
            .filter(|&b| {
                let (x, y) = (f.pos_varied_at(b, 0, 0), f.pos_varied_at(b + PASS_BEATS as u64, 0, 0));
                (x.letter, x.ops, x.phrase, x.prime, x.swap) != (y.letter, y.ops, y.phrase, y.prime, y.swap)
            })
            .count()
    }

    #[test]
    fn the_second_pass_transforms_the_first_by_a_recipe() {
        let mut used = std::collections::BTreeMap::<&str, usize>::new();
        let mut b = FormBuilder::new();
        let mut f = Form::generated();
        for seed in 0..300 {
            b.build(&spec(seed, 2, 2), &mut f);
            assert!(differing_beats(&f) > 0, "seed {seed}: {f}");
            for r in b.recipe_names().into_iter().flatten() {
                *used.entry(r).or_default() += 1;
            }
            // The second pass plays as built, flagged.
            assert!(f.pos_varied(FORM_BARS as u64, 0, 0).second);
        }
        assert_eq!(used.len(), RECIPES.len(), "every recipe plays: {used:?}");
        // User, 2026-10-26: the second pass draws its motives freely.
        let redrawn = (0..200)
            .filter(|&seed| {
                b.build(&spec(seed, 0, 2), &mut f);
                b.cycle_beats()[0] != b.cycle_beats()[1] || b.motive_bars()[0] != b.motive_bars()[1]
            })
            .count();
        assert!(redrawn > 80, "{redrawn} of 200 second passes lay out their own motives");
    }

    #[test]
    fn forms_keep_the_rules_and_build_the_same_again() {
        let mut b = FormBuilder::new();
        let (mut f, mut g) = (Form::generated(), Form::generated());
        let (mut broken, mut again, mut reps) = (0, 0, Vec::new());
        for seed in 0..600 {
            let s = FormSpec { drums: seed % 3 == 0, ..spec(seed, (seed % 5) as u8, (seed / 5 % 5) as u8) };
            b.build(&s, &mut f);
            let (rules, attempt) = b.broken();
            assert_eq!(rules & (Rule::GRID | Rule::CHANGES), 0, "seed {seed}: by construction: {f}");
            broken += (rules != 0) as usize;
            again += (attempt > 0) as usize;
            reps.push(b.repetition());
            // The same seeds and knobs, the same form.
            b.build(&s, &mut g);
            assert_eq!(f, g, "seed {seed}");
        }
        reps.sort_by(f32::total_cmp);
        assert!(broken <= 6, "{broken} of 600 forms break a rule after {MAX_ATTEMPTS} attempts ({again} built again)");
        assert!(reps[300] > 0.5 && reps[300] < 0.9, "median repetition {}", reps[300]);
    }

    #[test]
    fn a_letter_plays_one_operator_per_kind_of_place() {
        // User, 2026-10-21: snapshotted randomness: the seed fixes each
        // letter's operator, Variation only how many slots play it.
        let mut seg = Segment::new(Letter::A, 4, 0);
        seg.ops.push(Op::Seeded);
        let played = |seed: u32, slot: f32| {
            let mut pass: Vec<Segment> = (0..PASS_BEATS / 4).map(|k| Segment { letter: if k % 8 < 4 { Letter::A } else { Letter::B }, ..seg }).collect();
            let mut m = Mods::plain(seed, slot, 0.0, 0.0, 0.0);
            m.snap = seed;
            resolve(&mut pass, &[WINDOW_BEATS; SECTIONS], &m, 0);
            pass.iter().map(|s| (s.letter, s.ops.iter().next())).collect::<Vec<_>>()
        };
        for seed in 0..40 {
            let (low, high) = (played(seed, 0.3), played(seed, 0.8));
            // Raising Variation adds slots and keeps the ones played.
            for (a, b) in low.iter().zip(&high) {
                if a.1.is_some() {
                    assert_eq!(a, b, "seed {seed}");
                }
            }
            // Per letter few operators (one per kind of place).
            for l in [Letter::A, Letter::B] {
                let mut ops: Vec<Op> = high.iter().filter(|(x, o)| *x == l && o.is_some()).filter_map(|(_, o)| *o).collect();
                ops.sort_by_key(|o| o.as_char());
                ops.dedup();
                assert!(ops.len() <= 4, "seed {seed} {l:?}: {ops:?}");
            }
            assert!(high.iter().filter(|x| x.1.is_some()).count() > low.iter().filter(|x| x.1.is_some()).count());
        }
    }

    #[test]
    fn a_counter_follower_with_the_hosts_seeds_answers_its_letters() {
        // User, 2026-10-21: two instances on the same seeds; Splice must
        // change the pattern, not only thin it.
        let mut b = FormBuilder::new();
        let (mut host, mut out) = (Form::generated(), Form::generated());
        let (mut swapped, mut mains) = ([0usize; 3], 0usize);
        for seed in 0..60 {
            b.build(&spec(seed, 2, 2), &mut host);
            let own = host.clone();
            for (k, relation) in [0.0f32, 0.5, 1.0].into_iter().enumerate() {
                b.follow(&host, &own, relation, seed, false, &mut out);
                for beat in 0..2 * PASS_BEATS as u64 {
                    let (x, y) = (out.pos_varied_at(beat, 0, 0).letter, host.pos_varied_at(beat, 0, 0).letter);
                    if matches!(y, Letter::A | Letter::B) {
                        mains += (k == 0) as usize;
                        swapped[k] += (matches!(x, Letter::A | Letter::B) && x != y) as usize;
                    }
                }
            }
        }
        assert_eq!(swapped[0], 0, "the middle plays its own form");
        assert!(swapped[1] > mains / 8 && swapped[2] > swapped[1] + mains / 8, "{swapped:?} of {mains}");
    }

    #[test]
    fn a_follower_derives_its_form_from_the_hosts_by_the_relation() {
        let mut b = FormBuilder::new();
        let (mut host, mut own, mut out) = (Form::generated(), Form::generated(), Form::generated());
        // Per relation: beats like the host's, 4-bar windows where both
        // fill at their end, windows where one does.
        let mut stats = [(0usize, 0usize, 0usize); 3];
        for seed in 0..120 {
            b.build(&spec(seed, 2, 2), &mut host);
            b.build(&spec(seed + 1000, 2, 2), &mut own);
            for (k, relation) in [-1.0f32, 0.0, 1.0].into_iter().enumerate() {
                b.follow(&host, &own, relation, seed, false, &mut out);
                if relation == 0.0 {
                    assert_eq!(out.to_string(), own.to_string(), "the middle plays its own form");
                }
                for beat in 0..2 * PASS_BEATS as u64 {
                    let (x, y) = (out.pos_varied_at(beat, 0, 0), host.pos_varied_at(beat, 0, 0));
                    stats[k].0 += ((x.letter, x.ops, x.phrase) == (y.letter, y.ops, y.phrase)) as usize;
                }
                for w in 0..2 * (PASS_BEATS / WINDOW_BEATS) as u64 {
                    let fill = |f: &Form| (w * 16 + 12..w * 16 + 16).any(|b| f.pos_varied_at(b, 0, 0).letter.class() == SeedClass::Filler);
                    let (a, h) = (fill(&out), fill(&host));
                    stats[k].1 += (a && h) as usize;
                    stats[k].2 += (a != h) as usize;
                }
                // The grid holds.
                for pass in [out.pattern(), out.second_pass()] {
                    assert_eq!(pass.iter().map(|s| s.beats as u32).sum::<u32>(), PASS_BEATS as u32);
                }
            }
        }
        let beats = (120 * 2 * PASS_BEATS as usize) as f32;
        let (layer, middle, counter) = (stats[0], stats[1], stats[2]);
        assert!(layer.0 as f32 > 0.75 * beats, "stacked: {} of {beats} beats as the host's", layer.0);
        assert!(middle.0 < layer.0 / 2, "the middle its own: {}", middle.0);
        assert!(counter.1 < middle.1 / 2, "in the gaps the fills leave the host's: {} vs {}", counter.1, middle.1);
        assert!(counter.2 > middle.2, "and go where it plays: {} vs {}", counter.2, middle.2);
        assert!(layer.1 > middle.1, "stacked the fills overlap: {} vs {}", layer.1, middle.1);
    }

    #[test]
    fn building_does_not_allocate() {



        let mut b = FormBuilder::new();
        let mut f = Form::generated();
        b.build(&spec(1, 2, 2), &mut f);
        let (_, allocs) = crate::test_support::count_allocs(|| {
            for seed in 0..50 {
                b.build(&spec(seed, (seed % 5) as u8, 2), &mut f);
            }
        });
        assert_eq!(allocs, 0);
    }
}
