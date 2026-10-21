//! Generated forms (design D48): four phrases, a fill plan that lays them
//! over 64 bars, and a recipe for the second 64.
//!
//! A, B, C and D are phrase slots. A phrase is its letter's notes (the
//! letter seed, from the pattern seed) looped over a seeded length in beats,
//! a power of two, from a seeded offset into them. A fill plan of one
//! character (a roller, question and answer, a slow build, ...) repeats a
//! unit of phrases over 16-beat cycles, four cycles to a 16-bar section, and
//! puts the fills at the seams. The 16-bar grid holds by construction: every
//! section is built on its own and fills overwrite, so no section starts
//! anywhere but bars 1, 17, 33 and 49. Operators live in the plan only: its
//! fixed marks and its `?` slots, which the seed fills. Variation sets how
//! much of the plan's room for variants and extra fills is used, Shape tilts
//! the phrase lengths.
//!
//! Building is allocation-free into a [`FormBuilder`] and a form from
//! [`Form::generated`].

use crate::form::{Form, Letter, SeedClass, Segment, BEATS_PER_BAR, FORM_BARS, MAX_PASS_SEGMENTS};
use crate::ops::{self, Op, OpChain, Place};
use crate::rng;

/// Beats of a pass, of a section and of a cycle (a 4-bar phrase).
pub const PASS_BEATS: u16 = FORM_BARS * BEATS_PER_BAR;
pub const SECTION_BEATS: u16 = 16 * BEATS_PER_BAR;
pub const CYCLE_BEATS: u16 = 4 * BEATS_PER_BAR;
const SECTIONS: usize = (PASS_BEATS / SECTION_BEATS) as usize;
const WINDOWS: usize = (SECTION_BEATS / CYCLE_BEATS) as usize;

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
    // One offset per letter, whatever the section's length; the drums move
    // by half bars, so the snare stays on the backbeat.
    let raw = rng::below(rng::hash(seed, drums as u8, 0, 0, S_OFFSET), 16) as u8;
    let raw = if drums { raw & !1 } else { raw };
    Phrase { beats, offset: raw % beats }
}

/// One 16-bar section of a plan.
struct Sect {
    /// Unit shapes the section may repeat (a form that varies the letters'
    /// lengths per section plays the first).
    units: &'static [&'static str],
    /// The fill at its seam (empty: none).
    seam: &'static str,
}

/// A fill plan: a character, four sections, and the pickup the form may
/// open with.
struct Plan {
    name: &'static str,
    sections: [Sect; SECTIONS],
    head: &'static str,
}

// MUSICAL: tune by ear (user, 2026-10-21: the plans are the macro level,
// each its own character; A asks and B answers, C and D the transitions,
// fills mostly at the 16-bar seams; music-theory §5: focused repetition,
// one change per 4-bar phrase, the second drop re-cuts). Tokens: a letter,
// its marks (`'` `*` and the operators) and `?` slots the seed fills.
const PLANS: [Plan; 10] = [
    Plan {
        name: "steady roller",
        sections: [
            Sect { units: &["A A A? B?a", "A A? A Ba"], seam: "C" },
            Sect { units: &["A A A? Ba", "A A? A B?"], seam: "D" },
            Sect { units: &["B B B? A?a", "B B? A Ba"], seam: "C*" },
            Sect { units: &["A A A? B?a", "A A? A Bx"], seam: "D*" },
        ],
        head: "",
    },
    Plan {
        name: "question and answer",
        sections: [
            Sect { units: &["Aq Ba", "Aq B?a"], seam: "C" },
            Sect { units: &["Aq Bta", "Aq B?a", "A?q Bia"], seam: "D" },
            Sect { units: &["Bq Aa", "B?q Aa"], seam: "C*" },
            Sect { units: &["Aq Ba", "Aq B?a", "A?q Bxa"], seam: "D*" },
        ],
        head: "C",
    },
    Plan {
        name: "slow build",
        sections: [
            Sect { units: &["A"], seam: "" },
            Sect { units: &["A A?", "A A A?"], seam: "C" },
            Sect { units: &["A A? B A?", "A Ap? B Bp"], seam: "C" },
            Sect { units: &["A Ax? B Ap?", "A Ax B Bx?"], seam: "D*" },
        ],
        head: "",
    },
    Plan {
        name: "two-step cells",
        sections: [
            Sect { units: &["A A A Ay?", "A A B Ao?"], seam: "C" },
            Sect { units: &["B B B By?", "B B A Ao?"], seam: "D" },
            Sect { units: &["A Ao A AR?", "A Ao C B?"], seam: "Cx" },
            Sect { units: &["A A B Ay?", "A A? B Ba"], seam: "D*" },
        ],
        head: "C",
    },
    Plan {
        name: "breakdown",
        sections: [
            Sect { units: &["A A A? Bq", "A A? A B"], seam: "C" },
            Sect { units: &["Ch Dh", "Chl Dh", "Ch C?h Dhl"], seam: "Ax" },
            Sect { units: &["A Aa A? Bi", "A A? Bi A"], seam: "C" },
            Sect { units: &["B Ba A Ai?", "B A? Bq A"], seam: "D*" },
        ],
        head: "",
    },
    Plan {
        name: "B-led",
        sections: [
            Sect { units: &["A Aq B B?a", "A Aq B Ba"], seam: "C" },
            Sect { units: &["B B? A B", "B B A? Ba"], seam: "D" },
            Sect { units: &["B Bt B? Aa", "B B't B Aa"], seam: "C" },
            Sect { units: &["B Ba B B?", "B B? B Bx"], seam: "D*" },
        ],
        head: "",
    },
    Plan {
        name: "stabs and cells",
        sections: [
            Sect { units: &["As As As? Bs", "As As? As Bs"], seam: "C" },
            Sect { units: &["As Aso As Asy?", "As Aso? Bs Ba"], seam: "Ce" },
            Sect { units: &["Bs Bs Bsd? As", "Bs Bs? As Asd"], seam: "C*" },
            Sect { units: &["As Aso Bs Asy?", "As Asx? As Asdy"], seam: "Dx" },
        ],
        head: "C",
    },
    Plan {
        name: "space",
        sections: [
            Sect { units: &["An", "An Anl"], seam: "" },
            Sect { units: &["An Anl An? Ana", "An An? Anl"], seam: "C" },
            Sect { units: &["An Bnl An Bna?", "Bn An? Bnl"], seam: "D" },
            Sect { units: &["An An? An Anq", "An Anm? An"], seam: "Cn" },
        ],
        head: "",
    },
    Plan {
        name: "switch-up",
        sections: [
            Sect { units: &["A A A? Ba", "A A? A Ba"], seam: "C" },
            Sect { units: &["B B B? Aa", "B B A Am?"], seam: "D" },
            Sect { units: &["Ar Ar Ar? Ai", "Ard Ard Ard? Aa", "Ai Ai Ai? Ar"], seam: "C*" },
            Sect { units: &["Bi Bi Bri? Aa", "Ar Ar Aa? Bd"], seam: "D*" },
        ],
        head: "",
    },
    Plan {
        name: "bounce",
        sections: [
            Sect { units: &["A Ay A Au?", "A Ay B Ba"], seam: "C" },
            Sect { units: &["A Ad A Auy?", "B Bd B Ba"], seam: "D" },
            Sect { units: &["B By B Bu?", "A Ay C Aa"], seam: "C*" },
            Sect { units: &["A Ay B Bd?", "A Ax A Aux?"], seam: "D*" },
        ],
        head: "C",
    },
];

/// Plans there are.
pub const PLAN_COUNT: usize = PLANS.len();

// MUSICAL: tune by ear (user, 2026-10-21: Variation scales the form's
// variation too, at 0 a little, at 1 the full set). Share of `?` slots the
// seed fills, chance of an extra fill at a 4-bar seam inside a section, at
// Variation 0 and 1.
const SLOT_FILL: (f32, f32) = (0.25, 0.85);
const EXTRA_FILL: (f32, f32) = (0.04, 0.3);
/// Chance the form opens with its plan's pickup, and its longest length.
const HEAD_CHANCE: f32 = 0.6;
const HEAD_BEATS: u8 = 4;
/// Longest extra fill, and a seam's (beats).
const EXTRA_BEATS: u8 = 4;
const SEAM_BEATS: u8 = 8;
/// Chance an extra fill is D (else C).
const EXTRA_D: f32 = 0.3;

/// A plan token: a letter, its marks; `?` in `ops` is a slot.
#[derive(Debug, Clone, Copy)]
struct Tok {
    letter: Letter,
    prime: bool,
    swap: bool,
    ops: OpChain,
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
    let mut t = Tok { letter, prime: false, swap: false, ops: OpChain::default() };
    for c in chars {
        match c {
            '\'' => t.prime = true,
            '*' => t.swap = true,
            c => {
                t.ops.push(Op::from_char(c)?);
            }
        }
    }
    Some(t)
}

/// Most tokens a unit has.
const MAX_UNIT: usize = 8;

/// The tokens of `unit`.
fn tokens(unit: &str) -> ([Tok; MAX_UNIT], usize) {
    let mut out = [Tok { letter: Letter::A, prime: false, swap: false, ops: OpChain::default() }; MAX_UNIT];
    let mut n = 0;
    for t in unit.split_whitespace().filter_map(tok) {
        if n < MAX_UNIT {
            out[n] = t;
            n += 1;
        }
    }
    if n == 0 {
        n = 1;
    }
    (out, n)
}

/// How a pass is built: the first as the plan says, the second through its
/// recipe ([`Mods`] fields the recipe sets).
#[derive(Debug, Clone, Copy, Default)]
struct Mods {
    /// The first pass: its first 8 bars play A unchanged (music theory §5).
    first: bool,
    /// Seed of the slots and the extra fills.
    salt: u32,
    /// Share of slots filled, chance of an extra fill.
    slot: f32,
    extra: f32,
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
const RECUTS: [Op; 5] = [Op::Retrograde, Op::Inversion, Op::Dotted, Op::Rotation, Op::RhythmRetrograde];
const ANSWERS: [Op; 4] = [Op::Sequence, Op::Inversion, Op::Counter, Op::Passing];
const PUSHES: [Op; 3] = [Op::DoubleTime, Op::Syncopate, Op::Passing];

fn pick<T: Copy>(h: u64, from: &[T]) -> T {
    from[rng::below(h, from.len() as u32) as usize]
}

impl Mods {
    /// No recipe.
    fn plain(first: bool, salt: u32, slot: f32, extra: f32) -> Self {
        Self { first, salt, slot, extra, recut_from: SECTIONS, swap_from: SECTIONS, answer_from: SECTIONS, ..Self::default() }
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
            t.ops.push(Op::Thin);
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
    plan: usize,
    recipes: [Option<Recipe>; 2],
    attempts: u32,
    broken: u32,
    repetition: f32,
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
            cycles: [[CYCLE_BEATS; SECTIONS]; 2],
            plan: 0,
            recipes: [None; 2],
            attempts: 0,
            broken: 0,
            repetition: 0.0,
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
    }

    /// The rules the last form broke ([`Rule`] bits) and the attempt it is.
    pub fn broken(&self) -> (u32, u32) {
        (self.broken, self.attempts)
    }

    fn attempt(&mut self, spec: &FormSpec, attempt: u32) {
        self.attempts = attempt;
        let lane = spec.drums as u8;
        let fill = spec.seeds.fill;
        let salt = |seed: u32| if attempt == 0 { seed } else { rng::hash(seed, lane, attempt as u8, 0, S_ATTEMPT) as u32 };
        self.plan = rng::below(rng::hash(fill, lane, 0, 0, S_PLAN), PLANS.len() as u32) as usize;
        let plan = &PLANS[self.plan];

        // Per form either the unit shapes vary per section (the letters keep
        // their lengths) or the lengths do (the shapes stay), never both.
        let by_section = rng::chance(rng::hash(fill, lane, 0, 0, S_MODE), 0.5);
        let mut lengths = [[Phrase::default(); 4]; SECTIONS];
        let mut units = [0usize; SECTIONS];
        for s in 0..SECTIONS {
            for l in Letter::ALL {
                let section = if by_section { s as u8 } else { 0 };
                lengths[s][l.index() as usize] = phrase(spec.seeds.phrases[l.index() as usize], l, section, spec.shape, spec.drums);
            }
            let n = plan.sections[s].units.len() as u32;
            units[s] = if by_section { 0 } else { rng::below(rng::hash(fill, lane, s as u8, 0, S_UNIT), n) as usize };
        }
        let v = share(spec.variation);
        let (slot, extra) = (lerp(SLOT_FILL, v), lerp(EXTRA_FILL, v));
        let first = Mods::plain(true, salt(fill), slot, extra);
        let head = rng::chance(rng::hash(fill, lane, 0, 0, S_HEAD), HEAD_CHANCE).then_some(plan.head).and_then(tok);
        self.cycles[0] = build_pass(&mut self.first, plan, &units, &lengths, head, &first, lane);
        // The second pass: one recipe, a second one further up Variation.
        let sec = salt(spec.seeds.second);
        let mut second = Mods::plain(false, sec, slot + SECOND_SLOT, extra + SECOND_EXTRA);
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
        self.cycles[1] = build_pass(&mut self.second, plan, &units, &lengths, None, &second, lane);
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
        // The first 8 bars: A (and its answer) unchanged, no fill but the
        // pickup.
        let opening = (HEAD_BEATS as usize..2 * CYCLE_BEATS as usize).all(|i| !a[i].filler)
            && (self.cycles[0][0] != CYCLE_BEATS || (HEAD_BEATS as usize..CYCLE_BEATS as usize).all(|i| a[i] == a[i + CYCLE_BEATS as usize]));
        if !opening {
            broken |= Rule::OPENING;
        }
        if changes(&a, &self.cycles[0]) > 1 || changes(&b, &self.cycles[1]) > 1 {
            broken |= Rule::CHANGES;
        }
        let rep = repetition(&a);


        self.repetition = rep;
        if !(REPETITION.0..=REPETITION.1).contains(&rep) {
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
    /// The first pass's first 8 bars repeat unchanged (music theory §5).
    pub const OPENING: u32 = 2;
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
// never change and the ones that never repeat. Attempts before the fewest
// broken rules win.
const REPETITION: (f32, f32) = (0.1, 0.95);
const MAX_ATTEMPTS: u32 = 8;

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
            let from = base + w * CYCLE_BEATS as usize;
            if from < base + cycle as usize {
                continue;
            }
            let (mut runs, mut inside) = (0, false);
            for i in from..from + CYCLE_BEATS as usize {
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
        sum += at_lag(CYCLE_BEATS as usize).max(at_lag(2 * CYCLE_BEATS as usize));
    }
    sum / SECTIONS as f32
}

/// The operators the beats play, a fill's beat as one.
fn busy(pass: &[Beat; PASS_BEATS as usize]) -> usize {
    pass.iter().map(|b| b.filler as usize + b.ops.len()).sum()
}


/// A segment of token `t` from `lengths`, `beats` long, entering its phrase
/// `skip` beats in.
fn segment(t: &Tok, lengths: &[Phrase; 4], beats: u16, skip: u16) -> Segment {
    let p = lengths[t.letter.index() as usize];
    Segment {
        prime: t.prime,
        swap: t.swap,
        ops: t.ops,
        phrase: p.beats,
        offset: p.offset,
        skip: (skip % p.beats.max(1) as u16) as u8,
        ..Segment::new(t.letter, beats, 0)
    }
}

/// Repeat the unit `toks` over pass beats `from..to` in cycles of `cycle`
/// beats, each cycle starting the unit again, the last phrase of a cycle
/// clipped.
fn lay(out: &mut Vec<Segment>, toks: &[Tok], lengths: &[Phrase; 4], from: u16, to: u16, cycle: u16) {
    let mut cs = from;
    while cs < to {
        let ce = (cs + cycle).min(to);
        let (mut at, mut i) = (cs, 0usize);
        while at < ce {
            let t = &toks[i % toks.len()];
            let len = (lengths[t.letter.index() as usize].beats as u16).max(1);
            let beats = len.min(ce - at);
            if out.len() < out.capacity() {
                out.push(segment(t, lengths, beats, 0));
            }
            at += beats;
            i += 1;
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
fn build_pass(out: &mut Vec<Segment>, plan: &Plan, units: &[usize; SECTIONS], lengths: &[[Phrase; 4]; SECTIONS], head: Option<Tok>, m: &Mods, lane: u8) -> [u16; SECTIONS] {
    out.clear();
    let mut cycles = [CYCLE_BEATS; SECTIONS];
    for (s, sect) in plan.sections.iter().enumerate() {
        let unit = |k: usize| {
            let (mut toks, n) = tokens(plan.sections[k].units[units[k].min(plan.sections[k].units.len() - 1)]);
            for t in toks.iter_mut() {
                *t = m.token(*t, s);
            }
            (toks, n)
        };
        let (toks, n) = unit(s);
        let toks = &toks[..n];
        let len = |t: &Tok| (lengths[s][t.letter.index() as usize].beats as u16).max(1);
        let sum: u16 = toks.iter().map(len).sum();
        let cycle = sum.max(CYCLE_BEATS).next_power_of_two().min(SECTION_BEATS);
        cycles[s] = cycle;
        let base = s as u16 * SECTION_BEATS;
        if m.hook_back == Some(s) {
            // Cut short: the opening's unit comes back halfway.
            let (hook, k) = unit(0);
            let half = base + SECTION_BEATS / 2;
            lay(out, toks, &lengths[s], base, half, cycle.min(SECTION_BEATS / 2));
            let hook_sum: u16 = hook[..k].iter().map(len).sum();
            lay(out, &hook[..k], &lengths[s], half, base + SECTION_BEATS, hook_sum.max(CYCLE_BEATS).next_power_of_two().min(SECTION_BEATS / 2));
            cycles[s] = cycle.min(SECTION_BEATS / 2);
        } else {
            lay(out, toks, &lengths[s], base, base + SECTION_BEATS, cycle);
        }
        // Extra fills at the 4-bar seams inside the section (not in the
        // first pass's first 8 bars).
        for w in 1..WINDOWS as u16 {
            let at = base + w * CYCLE_BEATS;
            let h = rng::hash(m.salt, lane, s as u8, w as u8, S_EXTRA);
            if (m.first && at <= 2 * CYCLE_BEATS) || !rng::chance(h, m.extra) {
                continue;
            }
            let letter = if rng::chance(rng::derive(h, 1), EXTRA_D) { Letter::D } else { Letter::C };
            let t = Tok { letter, prime: false, swap: false, ops: OpChain::default() };
            let beats = lengths[s][letter.index() as usize].beats.min(EXTRA_BEATS) as u16;
            overwrite(out, at - beats, segment(&t, &lengths[s], beats, 0));
        }
        // The seam: it pads what the unit leaves of a cycle when that is
        // short, else it takes the section's last beats.
        if let Some(t) = tok(sect.seam) {
            let left = if sum < cycle { cycle % sum } else { 0 };
            let own = lengths[s][t.letter.index() as usize].beats.min(SEAM_BEATS) as u16;
            let beats = if (1..=SEAM_BEATS as u16).contains(&left) { left } else { own };
            overwrite(out, base + SECTION_BEATS - beats, segment(&t, &lengths[s], beats, 0));
        }
    }
    // The pickup the form opens with.
    if let Some(t) = head {
        let beats = lengths[0][t.letter.index() as usize].beats.min(HEAD_BEATS) as u16;
        overwrite(out, 0, segment(&t, &lengths[0], beats, 0));
    }
    // The last 4 bars doubled up.
    if m.double_end {
        let mut at = 0u16;
        for seg in out.iter_mut() {
            if at >= PASS_BEATS - CYCLE_BEATS && seg.letter.class() == SeedClass::Main {
                seg.ops.push(Op::DoubleTime);
            }
            at += seg.beats;
        }
    }
    resolve(out, &cycles, m, lane);
    merge(out);
    cycles
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

/// Fill the slots and keep the rules (music theory §5): a slot plays an
/// operator by its place (or none); inside a section each cycle repeats the
/// one before but for at most one change; the first pass's first 8 bars
/// repeat unchanged.
fn resolve(out: &mut [Segment], cycles: &[u16; SECTIONS], m: &Mods, lane: u8) {
    let mut starts = [0u16; MAX_PASS_SEGMENTS + 8];
    let mut at = 0u16;
    for (i, s) in out.iter().enumerate() {
        starts[i] = at;
        at += s.beats;
    }
    let n = out.len();
    for i in 0..n {
        if !out[i].ops.contains(Op::Seeded) {
            continue;
        }
        let start = starts[i];
        let end = start + out[i].beats;
        let next = out.get(i + 1).copied();
        let place = Place {
            period_end: end % CYCLE_BEATS == 0,
            repeat: i > 0 && out[i - 1].letter == out[i].letter,
            before_change: end % SECTION_BEATS == 0 || next.is_some_and(|x| x.letter.class() != out[i].letter.class()),
        };
        let h = rng::hash(m.salt, lane, (start / BEATS_PER_BAR) as u8, (start % 256) as u8, S_SLOT);
        let filled = !(m.first && start < 2 * CYCLE_BEATS) && rng::chance(h, m.slot);
        let push = rng::chance(rng::derive(h, 2), m.escalate);
        let mut chain = OpChain::default();
        for op in out[i].ops.iter() {
            match op {
                Op::Seeded if filled => {
                    chain.push(if push { pick(rng::derive(h, 3), &PUSHES) } else { ops::seeded(rng::derive(h, 1), place) });
                }
                Op::Seeded => {}
                op => {
                    chain.push(op);
                }
            }
        }
        out[i].ops = chain;
    }
    // One change per cycle against the one before it, in the section.
    let mut changed = [false; SECTIONS * WINDOWS];
    for i in 0..n {
        let (start, seg) = (starts[i], out[i]);
        let s = (start / SECTION_BEATS) as usize;
        let w = ((start % SECTION_BEATS) / CYCLE_BEATS) as usize;
        let cycle = cycles[s.min(SECTIONS - 1)];
        if seg.letter.class() == SeedClass::Filler || w + 1 == WINDOWS || start % SECTION_BEATS < cycle {
            continue;
        }
        let Some(j) = (0..i).rev().find(|&j| starts[j] + cycle == start) else { continue };
        let prev = out[j];
        if (prev.letter, prev.skip) != (seg.letter, seg.skip) || (prev.ops, prev.prime, prev.swap) == (seg.ops, seg.prime, seg.swap) {
            continue;
        }
        let k = s * WINDOWS + w;
        if changed[k] || (m.first && start < 2 * CYCLE_BEATS) {
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

/// The window's (beats `from..from + CYCLE_BEATS` of `pass`) fill at its
/// end, and whether it plays a variant (an operator but `q` and `a`).
fn window_shape(pass: &[Segment], from: u16) -> (bool, bool) {
    let (mut fill, mut varied, mut at) = (false, false, 0u16);
    let to = from + CYCLE_BEATS;
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
    let windows = PASS_BEATS / CYCLE_BEATS;
    for w in 0..windows {
        let from = w * CYCLE_BEATS;
        let to = from + CYCLE_BEATS;
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
            s.offset &= !1;
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
const S_MODE: u32 = 0xF84;
const S_UNIT: u32 = 0xF85;
const S_EXTRA: u32 = 0xF86;
const S_HEAD: u32 = 0xF87;
const S_SLOT: u32 = 0xF88;
const S_RECIPE: u32 = 0xF89;
const S_ATTEMPT: u32 = 0xF8A;
const S_FOLLOW: u32 = 0xF8B;




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
        FormSpec { seeds: FormSeeds::new(seed, seed.wrapping_mul(7).wrapping_add(1), seed ^ 0x55), variation, shape, drums: false }
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
                    if drums {
                        assert_eq!(p.offset % 2, 0, "the drums keep the backbeat");
                    }
                }
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

    #[test]
    fn variation_fills_more_slots_and_fills() {
        let count = |variation: u8| {
            let mut n = 0;
            for seed in 0..200 {
                let f = built(&spec(seed, variation, 2));
                n += f.pattern().iter().chain(f.second_pass()).filter(|s| !s.ops.is_empty() || s.letter.class() == SeedClass::Filler).count();
            }
            n
        };
        let (low, high) = (count(0), count(BANDS - 1));
        assert!(low > 0 && high as f32 > 1.15 * low as f32, "variants and fills: {low} at Variation 0, {high} at 1");
    }

    #[test]
    fn the_first_eight_bars_repeat_and_cycles_change_once() {
        for seed in 0..300 {
            let (f, cycles) = built_with(&spec(seed, 4, (seed % 5) as u8));
            let at = |beat: u16| f.pos_at(beat as u64);
            // Bars 1-4 and 5-8 play the same (a pickup aside) when the unit
            // is a 4-bar cycle.
            for b in (4..16u16).filter(|_| cycles[0][0] == CYCLE_BEATS) {

                let (x, y) = (at(b), at(b + 16));
                assert_eq!((x.letter, x.ops, x.phrase), (y.letter, y.ops, y.phrase), "seed {seed} beat {b}: {f}");
            }
            assert!((0..32u64).all(|b| f.pos_at(b).letter.class() == SeedClass::Main || b < HEAD_BEATS as u64), "seed {seed}: {f}");
        }
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
        // Variation sets how far it goes.
        let far = |variation: u8| (0..200).map(|seed| differing_beats(&built(&spec(seed, variation, 2)))).sum::<usize>();
        let (little, full) = (far(0), far(BANDS - 1));
        assert!(little > 0 && full as f32 > 1.3 * little as f32, "{little} beats differ at Variation 0, {full} at 1");
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
            assert_eq!(rules & (Rule::GRID | Rule::OPENING | Rule::CHANGES), 0, "seed {seed}: by construction: {f}");
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
                for w in 0..2 * (PASS_BEATS / CYCLE_BEATS) as u64 {
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
