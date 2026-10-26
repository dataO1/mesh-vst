//! Operators (design D47, D57 R9): related variants of a letter's motif,
//! worked on the full notes of a span (pitches, rhythm and lengths as the
//! chain made them).
//!
//! A form writes them as marks after the letter, `A3 Ar1 A3 Ai1 B2 Bq1`:
//! the segment plays its letter's first bars transformed. Marks chain left
//! to right (`A3rb`: the pitch order reversed, then the whole span played
//! backwards); a later mark wins where two set the same thing. `'` (another
//! second half) and `*` (another sound) stay what they were. The timing
//! itself (pushes, triplet and dotted feels) is the chain's Groove, not an
//! operator (D57). Pure and allocation-free.

use crate::event::{flags, EventKind, LiveEvent};
use crate::harmony::HarmonyCtx;
use crate::{rng, TICKS_PER_BAR, TICKS_PER_STEP};

/// One operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    /// `r`: the pitch order reversed, the rhythm kept (the drums: the hits
    /// backwards in time).
    Retrograde,
    /// `i`: the contour mirrored inside the scale, the rhythm kept.
    Inversion,
    /// `t`: the motif moved by a seeded number of scale steps (a sequence).
    Sequence,
    /// `c`: a counter line, contrary motion against the original.
    Counter,
    /// `u` / `w`: the mid line an octave up or down (never the downbeat root).
    OctaveUp,
    OctaveDown,
    /// `q`: an open ending (the 2nd or the 5th, shortened).
    Question,
    /// `a`: a closed ending (the root, held).
    Answer,
    /// `h`: augmentation (half time: the span's first half stretched over it).
    HalfTime,
    /// `x`: diminution (double time: the span compressed and played twice).
    DoubleTime,
    /// `p`: passing notes between wider steps.
    Passing,
    /// `m`: a few seeded notes change (pitch, length or presence).
    Mutate,
    /// `b`: the span backwards in time: pitches, rhythm and lengths.
    Reverse,
    /// `v`: backwards and mirrored (the retrograde inversion).
    RetroInversion,
    /// `z`: displacement: the window's notes an 8th later (its start rests).
    Displacement,
    /// `j`: hemiola: the window's first dotted quarter repeated across it,
    /// three against the bar's four.
    Hemiola,
    /// `g`: truncation: the motif cut short, a rest to the span's end.
    Truncation,
    /// `N`: neighbour tones: some notes step to a scale neighbour and back.
    Neighbour,
    /// `P`: pedal point: every other note of the line on the root.
    Pedal,
    /// `Y`: arpeggiation: the line on the chord's tones, up and down.
    Arpeggio,
    /// `F`: fragmentation: the window's first beat repeated over it.
    Fragment,
    /// `K`: call and response: the span's second half answers its first
    /// (its rhythm, the contour mirrored, ending home).
    CallResponse,
    /// `?`: the seed picks one, by where the segment sits in the form.
    Seeded,
}

impl Op {
    pub const ALL: [Op; 23] = [
        Op::Retrograde,
        Op::Inversion,
        Op::Sequence,
        Op::Counter,
        Op::OctaveUp,
        Op::OctaveDown,
        Op::Question,
        Op::Answer,
        Op::HalfTime,
        Op::DoubleTime,
        Op::Passing,
        Op::Mutate,
        Op::Reverse,
        Op::RetroInversion,
        Op::Displacement,
        Op::Hemiola,
        Op::Truncation,
        Op::Neighbour,
        Op::Pedal,
        Op::Arpeggio,
        Op::Fragment,
        Op::CallResponse,
        Op::Seeded,
    ];

    pub fn as_char(self) -> char {
        match self {
            Op::Retrograde => 'r',
            Op::Inversion => 'i',
            Op::Sequence => 't',
            Op::Counter => 'c',
            Op::OctaveUp => 'u',
            Op::OctaveDown => 'w',
            Op::Question => 'q',
            Op::Answer => 'a',
            Op::HalfTime => 'h',
            Op::DoubleTime => 'x',
            Op::Passing => 'p',
            Op::Mutate => 'm',
            Op::Reverse => 'b',
            Op::RetroInversion => 'v',
            Op::Displacement => 'z',
            Op::Hemiola => 'j',
            Op::Truncation => 'g',
            Op::Neighbour => 'N',
            Op::Pedal => 'P',
            Op::Arpeggio => 'Y',
            Op::Fragment => 'F',
            Op::CallResponse => 'K',
            Op::Seeded => '?',
        }
    }

    pub fn from_char(c: char) -> Option<Self> {
        Op::ALL.iter().copied().find(|o| o.as_char() == c)
    }

    /// Whether it moves pitches of the line (not the endings `q` and `a`).
    pub fn moves_pitch(self) -> bool {
        matches!(
            self,
            Op::Retrograde | Op::Inversion | Op::Sequence | Op::Counter | Op::OctaveUp | Op::OctaveDown | Op::RetroInversion | Op::Neighbour | Op::Pedal | Op::Arpeggio
        )
    }

    fn index(self) -> u8 {
        Op::ALL.iter().position(|o| *o == self).unwrap_or(0) as u8
    }
}

/// Most operators one segment chains.
pub const MAX_OPS: usize = 6;

/// The operators of a segment, in the order they apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct OpChain {
    ops: [u8; MAX_OPS],
    len: u8,
}

impl OpChain {
    /// No operators.
    pub const EMPTY: OpChain = OpChain { ops: [0; MAX_OPS], len: 0 };

    /// Append `op`; `false` when the chain is full.
    pub fn push(&mut self, op: Op) -> bool {
        if self.len as usize >= MAX_OPS {
            return false;
        }
        self.ops[self.len as usize] = op.index();
        self.len += 1;
        true
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, i: usize) -> Option<Op> {
        (i < self.len as usize).then(|| Op::ALL[self.ops[i] as usize])
    }

    pub fn iter(&self) -> impl Iterator<Item = Op> + '_ {
        (0..self.len as usize).map(|i| Op::ALL[self.ops[i] as usize])
    }

    pub fn contains(&self, op: Op) -> bool {
        self.iter().any(|o| o == op)
    }

    /// Replace operator `i`.
    pub fn set(&mut self, i: usize, op: Op) {
        if i < self.len as usize {
            self.ops[i] = op.index();
        }
    }

    /// The chain with every question an answer and every answer a question.
    pub fn swap_question_answer(mut self) -> Self {
        for i in 0..self.len as usize {
            match self.get(i) {
                Some(Op::Question) => self.set(i, Op::Answer),
                Some(Op::Answer) => self.set(i, Op::Question),
                _ => {}
            }
        }
        self
    }

    /// A stable fingerprint of the chain.
    pub fn fingerprint(&self) -> u64 {
        self.ops[..self.len as usize].iter().fold(self.len as u64, |h, o| h.wrapping_mul(31).wrapping_add(*o as u64 + 1))
    }
}

impl std::fmt::Display for OpChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for op in self.iter() {
            write!(f, "{}", op.as_char())?;
        }
        Ok(())
    }
}

// MUSICAL: tune by ear (user, 2026-10-21: the seed picks `?` by where the
// segment sits: answers at period ends, mutations on repeats, diminution
// before a section change; 2026-10-23: musical operators, the timing is the
// Groove's). The weights of each place.
const SEEDED_BEFORE_CHANGE: [(Op, f32); 5] = [(Op::DoubleTime, 2.0), (Op::Truncation, 1.0), (Op::Fragment, 1.0), (Op::Question, 1.0), (Op::HalfTime, 1.0)];
const SEEDED_PERIOD_END: [(Op, f32); 6] = [(Op::Answer, 3.0), (Op::Inversion, 2.0), (Op::Question, 1.0), (Op::Mutate, 1.0), (Op::Retrograde, 1.0), (Op::CallResponse, 1.0)];
const SEEDED_REPEAT: [(Op, f32); 6] = [(Op::Mutate, 3.0), (Op::Retrograde, 1.0), (Op::Neighbour, 1.0), (Op::Pedal, 1.0), (Op::Displacement, 1.0), (Op::Hemiola, 0.5)];
const SEEDED_ANY: [(Op, f32); 12] = [
    (Op::Retrograde, 1.0),
    (Op::Inversion, 1.0),
    (Op::Sequence, 1.0),
    (Op::Reverse, 1.0),
    (Op::Mutate, 2.0),
    (Op::Passing, 1.0),
    (Op::Neighbour, 1.0),
    (Op::Arpeggio, 1.0),
    (Op::Pedal, 0.7),
    (Op::Displacement, 0.7),
    (Op::Hemiola, 0.7),
    (Op::Fragment, 0.7),
];
/// The melodic operators of a generated form's slots, by place.
const MELODIC_BEFORE_CHANGE: [(Op, f32); 5] = [(Op::Question, 1.0), (Op::Sequence, 1.0), (Op::OctaveUp, 1.0), (Op::Truncation, 1.0), (Op::Fragment, 1.0)];
const MELODIC_REPEAT: [(Op, f32); 5] = [(Op::Mutate, 3.0), (Op::Retrograde, 1.0), (Op::Neighbour, 1.0), (Op::Pedal, 1.0), (Op::Passing, 1.0)];
const MELODIC_ANY: [(Op, f32); 10] = [
    (Op::Retrograde, 1.0),
    (Op::Inversion, 1.0),
    (Op::Sequence, 1.0),
    (Op::Reverse, 1.0),
    (Op::RetroInversion, 0.5),
    (Op::Mutate, 2.0),
    (Op::Passing, 1.0),
    (Op::Arpeggio, 1.0),
    (Op::Displacement, 0.7),
    (Op::Hemiola, 0.7),
];

/// A pick from `table` by `h`.
fn pick(h: u64, table: &[(Op, f32)]) -> Op {
    let mut weights = [0.0f32; 16];
    for (w, (_, x)) in weights.iter_mut().zip(table) {
        *w = *x;
    }
    table[rng::weighted_pick(h, &weights[..table.len()]).unwrap_or(0)].0
}

/// An operator for a generated form's slot, by its place.
pub fn seeded_melodic(h: u64, place: Place) -> Op {
    pick(
        h,
        if place.before_change {
            &MELODIC_BEFORE_CHANGE
        } else if place.period_end {
            &SEEDED_PERIOD_END
        } else if place.repeat {
            &MELODIC_REPEAT
        } else {
            &MELODIC_ANY
        },
    )
}

/// Where a segment sits in its form, for [`seeded`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Place {
    pub period_end: bool,
    pub repeat: bool,
    pub before_change: bool,
}

/// The operator `?` stands for, from a seed and the segment's place.
pub fn seeded(h: u64, place: Place) -> Op {
    pick(
        h,
        if place.before_change {
            &SEEDED_BEFORE_CHANGE
        } else if place.period_end {
            &SEEDED_PERIOD_END
        } else if place.repeat {
            &SEEDED_REPEAT
        } else {
            &SEEDED_ANY
        },
    )
}

/// Bars a span holds at most: a longer segment repeats its first
/// [`MAX_SPAN_BARS`] transformed.
pub const MAX_SPAN_BARS: usize = 16;

/// One event of a span: its tick from the span's start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanNote {
    pub at: u32,
    /// Order among events on the same tick (as the generator wrote them).
    pub seq: u32,
    pub ev: LiveEvent,
}

/// What the operators of one span work with.
#[derive(Debug, Clone, Copy)]
pub struct OpCtx {
    /// Length of the span in ticks.
    pub ticks: u32,
    /// How far the operators go, 0 (a little) .. 1 (in full): from Variation.
    pub amount: f32,
    /// The letter seed and the lane (the operators' seeded details).
    pub seed: u32,
    pub lane: u8,
    /// Drums: the pitch operators do nothing, the others act on the hits.
    pub drums: bool,
    pub harmony: HarmonyCtx,
}

/// Apply `chain` to the span `notes` (sorted by tick), left to right.
/// RT-safe: `notes` never grows past its capacity.
pub fn apply(chain: &OpChain, notes: &mut Vec<SpanNote>, ctx: &OpCtx) {
    for (k, op) in chain.iter().enumerate() {
        apply_one(op, k as u8, notes, ctx);
        sort(notes);
    }
}

/// Sort by tick, events on one tick in their written order.
pub fn sort(notes: &mut [SpanNote]) {
    notes.sort_unstable_by_key(|n| (n.at, n.seq));
}

const BAR: u32 = TICKS_PER_BAR as u32;
const BEAT: u32 = BAR / 4;
const STEP: u32 = TICKS_PER_STEP as u32;
const S_OPS: u32 = 0xF70;

// MUSICAL: first draft, tune by ear (user, 2026-10-21: at 0 % Variation a
// little, at 100 % the full variation; the scaling of design D47's table).
/// Inversion: the intervals mirrored at half size at the least.
const INVERT_LEAST: f32 = 0.5;
/// Sequence steps (scale degrees) a seed moves its motif by.
const SEQUENCE_STEPS: [i32; 6] = [2, 3, -2, 4, -3, 1];
/// Counter line: the share of moves made contrary, at the least; the
/// largest contrary step (degrees).
const COUNTER_LEAST: f32 = 0.5;
const COUNTER_STEP_MAX: i32 = 3;
/// Question: the open degrees (the 2nd and the 5th over the key root).
const OPEN_DEGREES: [i32; 2] = [1, 4];
/// Ending notes the question and the answer rebuild at full amount.
const ENDING_NOTES_MAX: f32 = 3.0;
/// Share of the notes passing and neighbour tones take, at the least and
/// in full; how much of the window truncation cuts at the least and in
/// full; the hemiola's cell (a dotted quarter) and the fragment (a beat).
const PASSING_SHARE: (f32, f32) = (0.2, 1.0);
const NEIGHBOUR_SHARE: (f32, f32) = (0.2, 0.6);
const TRUNCATE_SHARE: (f32, f32) = (0.25, 0.5);
/// The longest rest truncation leaves (D61: a cycle's part runs to 60
/// beats; a share of it would silence bars).
const TRUNCATE_MAX: u32 = 2 * BEAT;
const HEMIOLA_CELL: u32 = 6 * STEP;
const FRAGMENT: u32 = BEAT;
/// Mutation: the share of the notes changed in full (at least one).
const MUTATE_SHARE: f32 = 0.33;
/// Drums: a ghost's velocity share, and the open ending's span (ticks).
const GHOST_SHARE: f32 = 0.35;
const OPEN_END_TICKS: (f32, f32) = (48.0, 192.0);
/// Lowest and highest key an octave move may land on.
const KEY_RANGE: (i32, i32) = (24, 100);
/// Note ids of the notes an operator adds (a repeat, a passing note).
const ADDED_ID: u16 = 0x3000;

fn lerp(a: (f32, f32), t: f32) -> f32 {
    a.0 + (a.1 - a.0) * t.clamp(0.0, 1.0)
}

fn is_note(n: &SpanNote) -> bool {
    matches!(n.ev.kind, EventKind::NoteOn | EventKind::Trigger)
}

/// A downbeat note of a pitched lane: the root lock, pitch operators leave it.
fn is_anchor(n: &SpanNote, ctx: &OpCtx) -> bool {
    !ctx.drums && n.ev.kind == EventKind::NoteOn && n.at % BAR == 0
}

/// A pitched note the pitch operators move.
fn is_line(n: &SpanNote, ctx: &OpCtx) -> bool {
    !ctx.drums && n.ev.kind == EventKind::NoteOn && n.at % BAR != 0
}

/// The last `len` ticks of the span (at least `least`), scaled by the amount.
fn window(ctx: &OpCtx, least: u32) -> (u32, u32) {
    let least = least.min(ctx.ticks) as f32;
    let len = least + (ctx.ticks as f32 - least) * ctx.amount.clamp(0.0, 1.0);
    (ctx.ticks - (len.round() as u32).min(ctx.ticks), ctx.ticks)
}

fn in_window(n: &SpanNote, (from, to): (u32, u32)) -> bool {
    n.at >= from && n.at < to
}

fn h(ctx: &OpCtx, k: u8, stream: u32, n: u32) -> u64 {
    rng::derive(rng::hash(ctx.seed, ctx.lane, k, 0, S_OPS + stream), n)
}

/// A key as a scale degree of the key (and the semitones off the scale).
pub fn degree(key: u8, harmony: &HarmonyCtx) -> (i32, i32) {
    let scale = harmony.scale();
    let rel = key as i32 - harmony.key.root as i32;
    let (oct, pc) = (rel.div_euclid(12), rel.rem_euclid(12));
    let idx = (0..7).rev().find(|&i| scale[i] as i32 <= pc).unwrap_or(0);
    (oct * 7 + idx as i32, pc - scale[idx] as i32)
}

pub fn key_of(deg: i32, off: i32, harmony: &HarmonyCtx) -> u8 {
    let scale = harmony.scale();
    let (oct, idx) = (deg.div_euclid(7), deg.rem_euclid(7) as usize);
    (harmony.key.root as i32 + 12 * oct + scale[idx] as i32 + off).clamp(1, 127) as u8
}

/// The degree `target` (mod 7) nearest to `near`.
fn nearest(near: i32, target: i32) -> i32 {
    let base = near.div_euclid(7) * 7 + target.rem_euclid(7);
    [base - 7, base, base + 7].into_iter().min_by_key(|d| (d - near).abs()).unwrap_or(base)
}

/// Push a note within the capacity.
fn add(notes: &mut Vec<SpanNote>, n: SpanNote) {
    if notes.len() < notes.capacity() {
        notes.push(n);
    }
}

/// The room of note `i` (ticks to the next note, or to the span's end).
fn room(notes: &[SpanNote], i: usize, ticks: u32) -> u32 {
    notes[i + 1..].iter().find(|n| is_note(n) && n.at > notes[i].at).map_or(ticks, |n| n.at) - notes[i].at
}

/// One event per tick and target (or key): a grid that put two together
/// keeps the first.
fn dedupe(notes: &mut Vec<SpanNote>) {
    sort(notes);
    let mut last: Option<(u32, u8, u8, EventKind)> = None;
    notes.retain(|n| {
        if !is_note(n) {
            return true;
        }
        let id = (n.at, n.ev.target, n.ev.key, n.ev.kind);
        let keep = last != Some(id);
        last = Some(id);
        keep
    });
}

fn apply_one(op: Op, k: u8, notes: &mut Vec<SpanNote>, ctx: &OpCtx) {
    let a = ctx.amount.clamp(0.0, 1.0);
    let hm = &ctx.harmony;
    match op {
        Op::Seeded => {}
        Op::Retrograde | Op::Reverse if ctx.drums => reverse(notes, ctx, true),
        Op::Retrograde => {
            // The pitches of the window, in reverse order.
            let w = window(ctx, BEAT);
            let (ids, len) = notes.iter().enumerate().filter(|(_, n)| is_line(n, ctx) && in_window(n, w)).map(|(i, _)| i).collect_array();
            for j in 0..len / 2 {
                let (x, y) = (ids[j], ids[len - 1 - j]);
                let kx = notes[x].ev.key;
                notes[x].ev.key = notes[y].ev.key;
                notes[y].ev.key = kx;
            }
        }
        Op::Inversion if !ctx.drums => invert(notes, ctx, a),
        Op::Reverse => reverse(notes, ctx, true),
        Op::RetroInversion if !ctx.drums => {
            reverse(notes, ctx, true);
            sort(notes);
            invert(notes, ctx, a);
        }
        Op::Sequence if !ctx.drums => {
            let steps = SEQUENCE_STEPS[rng::below(h(ctx, k, 1, 0), SEQUENCE_STEPS.len() as u32) as usize];
            let w = window(ctx, ctx.ticks / 2);
            for n in notes.iter_mut().filter(|n| is_line(n, ctx) && in_window(n, w)) {
                let (d, off) = degree(n.ev.key, hm);
                n.ev.key = key_of(d + steps, off, hm);
            }
        }
        Op::Counter if !ctx.drums => {
            let p = COUNTER_LEAST + (1.0 - COUNTER_LEAST) * a;
            let mut prev: Option<(i32, i32)> = None;
            for (i, n) in notes.iter_mut().enumerate() {
                if is_anchor(n, ctx) {
                    prev = None;
                    continue;
                }
                if !is_line(n, ctx) {
                    continue;
                }
                let (d, off) = degree(n.ev.key, hm);
                let new = match prev {
                    None => d,
                    Some((orig, new)) => {
                        let delta = d - orig;
                        if delta != 0 && rng::chance(h(ctx, k, 2, i as u32), p) {
                            new - delta.signum() * delta.abs().min(COUNTER_STEP_MAX)
                        } else {
                            new + delta
                        }
                    }
                };
                n.ev.key = key_of(new, off, hm);
                prev = Some((d, new));
            }
        }
        Op::OctaveUp | Op::OctaveDown if !ctx.drums => {
            let shift = if op == Op::OctaveUp { 12 } else { -12 };
            let w = window(ctx, BEAT);
            for n in notes.iter_mut().filter(|n| is_line(n, ctx) && in_window(n, w)) {
                let to = n.ev.key as i32 + shift;
                if (KEY_RANGE.0..=KEY_RANGE.1).contains(&to) {
                    n.ev.key = to as u8;
                }
            }
        }
        Op::Question | Op::Answer if ctx.drums => drum_ending(notes, ctx, op == Op::Answer),
        Op::Question | Op::Answer => {
            let answer = op == Op::Answer;
            let target = if answer { 0 } else { OPEN_DEGREES[rng::below(h(ctx, k, 3, 0), 2) as usize] };
            let ending = 1 + (a * (ENDING_NOTES_MAX - 1.0)).round() as usize;
            let (ids, len) = notes.iter().enumerate().filter(|(_, n)| n.ev.kind == EventKind::NoteOn).map(|(i, _)| i).collect_array();
            let Some(&last) = ids[..len].last() else { return };
            let goal = nearest(degree(notes[last].ev.key, hm).0, target);
            for back in 0..ending.min(len) {
                let i = ids[len - 1 - back];
                if back > 0 && is_anchor(&notes[i], ctx) {
                    break;
                }
                // The last note on the goal, the ones before stepping down to it.
                notes[i].ev.key = key_of(goal + back as i32, 0, hm);
            }
            let rest = ctx.ticks - notes[last].at;
            let g = &mut notes[last].ev.gate;
            if answer {
                *g = rest.min(u16::MAX as u32) as u16;
            } else {
                *g = (*g / 2).max(STEP as u16 / 2);
                notes[last].ev.loop_len = 0;
            }
        }
        Op::HalfTime => {
            let (from, to) = window(ctx, BAR);
            let half = (to - from) / 2;
            notes.retain(|n| !is_note(n) || n.at < from || n.at - from < half);
            for n in notes.iter_mut().filter(|n| is_note(n) && n.at >= from) {
                n.at = from + 2 * (n.at - from);
                n.ev.gate = n.ev.gate.saturating_mul(2);
            }
        }
        Op::DoubleTime => {
            let (from, to) = window(ctx, BEAT);
            let half = (to - from) / 2;
            let len = notes.len();
            for i in 0..len {
                if !is_note(&notes[i]) || notes[i].at < from {
                    continue;
                }
                let mut n = notes[i];
                n.at = from + (n.at - from) / 2;
                if n.ev.gate > 0 {
                    n.ev.gate = (n.ev.gate / 2).max(STEP as u16 / 2);
                }
                n.ev.loop_len = 0;
                notes[i] = n;
                let mut again = n;
                again.at += half;
                again.seq += 1 << 20;
                again.ev.note_id = again.ev.note_id.wrapping_add(ADDED_ID);
                add(notes, again);
            }
        }
        Op::Passing if !ctx.drums => {
            let share = lerp(PASSING_SHARE, a);
            sort(notes);
            let len = notes.len();
            for i in 0..len {
                if notes[i].ev.kind != EventKind::NoteOn {
                    continue;
                }
                let Some(j) = (i + 1..len).find(|&j| notes[j].ev.kind == EventKind::NoteOn) else { continue };
                let (from, to) = (degree(notes[i].ev.key, hm).0, degree(notes[j].ev.key, hm).0);
                let gap = notes[j].at - notes[i].at;
                if (to - from).abs() < 2 || gap < 2 * STEP || !rng::chance(h(ctx, k, 8, i as u32), share) {
                    continue;
                }
                let at = notes[i].at + (gap / 2) / STEP * STEP;
                insert_between(notes, i, j, at, key_of(from + (to - from).signum(), 0, hm));
            }
        }
        Op::Neighbour if !ctx.drums => {
            // A note steps to its upper or lower neighbour half way to the
            // next one, and the next one returns.
            let share = lerp(NEIGHBOUR_SHARE, a);
            sort(notes);
            let len = notes.len();
            for i in 0..len {
                if !is_line(&notes[i], ctx) && !is_anchor(&notes[i], ctx) {
                    continue;
                }
                let Some(j) = (i + 1..len).find(|&j| notes[j].ev.kind == EventKind::NoteOn) else { continue };
                let gap = notes[j].at - notes[i].at;
                let hk = h(ctx, k, 12, i as u32);
                if gap < 2 * STEP || !rng::chance(hk, share) {
                    continue;
                }
                let (d, _) = degree(notes[i].ev.key, hm);
                let step = if rng::chance(rng::derive(hk, 1), 0.5) { 1 } else { -1 };
                let at = notes[i].at + (gap / 2) / STEP * STEP;
                insert_between(notes, i, j, at, key_of(d + step, 0, hm));
            }
        }
        Op::Pedal if !ctx.drums => {
            // Every other note of the line on the root (the nearest to it).
            let w = window(ctx, BAR);
            let mut odd = false;
            for n in notes.iter_mut().filter(|n| n.ev.kind == EventKind::NoteOn && in_window(n, w)) {
                if odd && !is_anchor(n, ctx) {
                    let (d, _) = degree(n.ev.key, hm);
                    n.ev.key = key_of(nearest(d, hm.chord_degree as i32), 0, hm);
                }
                odd = !odd;
            }
        }
        Op::Arpeggio if !ctx.drums => {
            // The line on the chord's tones (root, 3rd, 5th, octave), up then
            // down, from the window's first note.
            let w = window(ctx, BAR);
            let chord = hm.chord_degree as i32;
            let Some(first) = notes.iter().find(|n| n.ev.kind == EventKind::NoteOn && in_window(n, w)).map(|n| degree(n.ev.key, hm).0) else { return };
            let base = nearest(first, chord);
            const SHAPE: [i32; 6] = [0, 2, 4, 7, 4, 2];
            let start = rng::below(h(ctx, k, 13, 0), SHAPE.len() as u32) as usize;
            for (i, n) in notes.iter_mut().filter(|n| is_line(n, ctx) && in_window(n, w)).enumerate() {
                n.ev.key = key_of(base + SHAPE[(start + i) % SHAPE.len()], 0, hm);
            }
        }
        Op::Displacement => {
            let (from, to) = window(ctx, BAR);
            notes.retain(|n| !(is_note(n) && n.at >= from && n.at + 2 * STEP >= to && n.at < to));
            for n in notes.iter_mut().filter(|n| is_note(n) && n.at >= from && n.at < to) {
                n.at += 2 * STEP;
                if n.ev.gate > 0 {
                    n.ev.gate = n.ev.gate.min((to - n.at).min(u16::MAX as u32) as u16);
                }
            }
            dedupe(notes);
        }
        Op::Hemiola => repeat_cell(notes, ctx, HEMIOLA_CELL),
        Op::Fragment => repeat_cell(notes, ctx, FRAGMENT),
        Op::Truncation => {
            let rest = ((ctx.ticks as f32 * lerp(TRUNCATE_SHARE, a)) as u32).min(TRUNCATE_MAX);
            let cut = ctx.ticks.saturating_sub(rest) / STEP * STEP;
            notes.retain(|n| !is_note(n) || n.at < cut);
            for n in notes.iter_mut().filter(|n| is_note(n) && n.ev.gate > 0) {
                n.ev.gate = n.ev.gate.min((cut.saturating_sub(n.at)).max(1).min(u16::MAX as u32) as u16);
            }
        }
        Op::CallResponse => call_response(notes, ctx),
        Op::Mutate => mutate(notes, ctx, k),
        // Pitch operators on the drums.
        _ => {}
    }
}

/// Insert a note of `key` at `at` between note `i` and note `j` (`i` ends
/// there, the new one holds to `j`).
fn insert_between(notes: &mut Vec<SpanNote>, i: usize, j: usize, at: u32, key: u8) {
    let mut x = notes[i];
    x.at = at;
    x.seq += 1 << 20;
    x.ev.key = key;
    x.ev.gate = (notes[j].at - at).min(u16::MAX as u32) as u16;
    x.ev.note_id = x.ev.note_id.wrapping_add(ADDED_ID);
    x.ev.flags &= !(flags::TIE | flags::ACCENT);
    x.ev.loop_len = 0;
    x.ev.origin = crate::event::origin::OPERATOR;
    notes[i].ev.gate = notes[i].ev.gate.min((at - notes[i].at) as u16);
    add(notes, x);
}

/// The contour of the line mirrored around its first note, at `a` (half
/// size at the least).
fn invert(notes: &mut [SpanNote], ctx: &OpCtx, a: f32) {
    let hm = &ctx.harmony;
    let f = INVERT_LEAST + (1.0 - INVERT_LEAST) * a;
    let Some(axis) = notes.iter().find(|n| is_line(n, ctx)).map(|n| degree(n.ev.key, hm).0) else { return };
    for n in notes.iter_mut().filter(|n| is_line(n, ctx)) {
        let (d, off) = degree(n.ev.key, hm);
        n.ev.key = key_of(axis - ((d - axis) as f32 * f).round() as i32, off, hm);
    }
}

/// `b` (and `r` on the drums): the window backwards in time, every note
/// with its pitch and length (`whole`), the downbeat root staying.
fn reverse(notes: &mut [SpanNote], ctx: &OpCtx, whole: bool) {
    let (from, to) = window(ctx, BEAT);
    let moves = |n: &SpanNote| is_note(n) && in_window(n, (from, to)) && !is_anchor(n, ctx);
    for n in notes.iter_mut().filter(|n| moves(n)) {
        let dur = if n.ev.gate > 0 { (n.ev.gate as u32).min(to - n.at) } else { STEP };
        n.at = (from + to).saturating_sub(n.at + dur).max(from);
        if whole && n.ev.gate > 0 {
            n.ev.gate = dur.min(u16::MAX as u32) as u16;
        }
        n.ev.flags &= !flags::TIE;
    }
}

/// The window's first `cell` ticks repeated across it (the rest of the
/// window's notes give way): a hemiola for a dotted quarter, a fragment
/// for a beat.
fn repeat_cell(notes: &mut Vec<SpanNote>, ctx: &OpCtx, cell: u32) {
    let (from, to) = window(ctx, BAR);
    if to - from <= cell {
        return;
    }
    let in_cell = |n: &SpanNote| is_note(n) && n.at >= from && n.at < from + cell;
    if !notes.iter().any(in_cell) {
        return;
    }
    notes.retain(|n| !(is_note(n) && n.at >= from + cell && n.at < to));
    let len = notes.len();
    let mut lap = cell;
    while from + lap < to {
        for i in 0..len {
            if !in_cell(&notes[i]) {
                continue;
            }
            let mut x = notes[i];
            x.at += lap;
            if x.at >= to {
                continue;
            }
            x.seq += (lap / cell) << 20;
            x.ev.note_id = x.ev.note_id.wrapping_add(ADDED_ID + (lap / cell) as u16);
            if x.ev.gate > 0 {
                x.ev.gate = x.ev.gate.min((to - x.at).min(u16::MAX as u32) as u16);
            }
            x.ev.origin = crate::event::origin::OPERATOR;
            add(notes, x);
        }
        lap += cell;
    }
}

/// `K`: the span's second half replaced by its first, the contour mirrored
/// around its first note, ending on the root (the drums: the first half's
/// hits again).
fn call_response(notes: &mut Vec<SpanNote>, ctx: &OpCtx) {
    let half = ctx.ticks / 2;
    if half < BEAT {
        return;
    }
    let hm = &ctx.harmony;
    notes.retain(|n| !(is_note(n) && n.at >= half));
    let len = notes.len();
    let axis = notes.iter().find(|n| is_line(n, ctx) || is_anchor(n, ctx)).map(|n| degree(n.ev.key, hm).0);
    let mut last: Option<usize> = None;
    for i in 0..len {
        if !is_note(&notes[i]) {
            continue;
        }
        let mut x = notes[i];
        x.at += half;
        x.seq += 1 << 20;
        x.ev.note_id = x.ev.note_id.wrapping_add(ADDED_ID);
        x.ev.origin = crate::event::origin::OPERATOR;
        if let (false, Some(axis)) = (ctx.drums, axis) {
            if x.ev.kind == EventKind::NoteOn {
                let (d, off) = degree(x.ev.key, hm);
                x.ev.key = key_of(axis - (d - axis), off, hm);
            }
        }
        add(notes, x);
        last = Some(notes.len() - 1);
    }
    // The answer comes home.
    if let (false, Some(l)) = (ctx.drums, last) {
        if notes[l].ev.kind == EventKind::NoteOn {
            let d = degree(notes[l].ev.key, hm).0;
            notes[l].ev.key = key_of(nearest(d, 0), 0, hm);
        }
    }
}

/// Indices of matching notes, without allocating (at most a span's notes).
trait CollectArray {
    fn collect_array(self) -> ([usize; MAX_LINE], usize);
}

/// Most notes one pitch operator reorders.
const MAX_LINE: usize = 512;

impl<I: Iterator<Item = usize>> CollectArray for I {
    fn collect_array(self) -> ([usize; MAX_LINE], usize) {
        let mut out = [0usize; MAX_LINE];
        let mut n = 0;
        for i in self.take(MAX_LINE) {
            out[n] = i;
            n += 1;
        }
        (out, n)
    }
}

/// `q` / `a` on the drums: an open ending (the last hits gone) or a closed
/// one (a strong hit on the last beat, nothing after it).
fn drum_ending(notes: &mut Vec<SpanNote>, ctx: &OpCtx, closed: bool) {
    let a = ctx.amount.clamp(0.0, 1.0);
    let from = ctx.ticks.saturating_sub(lerp(OPEN_END_TICKS, a).round() as u32);
    if closed {
        let last_beat = ctx.ticks.saturating_sub(BEAT);
        let Some(mut hit) = notes.iter().find(|n| is_note(n)).copied() else { return };
        notes.retain(|n| !is_note(n) || n.at < last_beat || n.at < from);
        hit.at = last_beat;
        hit.seq += 1 << 20;
        hit.ev.vel = hit.ev.vel.max(110);
        add(notes, hit);
    } else {
        notes.retain(|n| !is_note(n) || n.at < from);
    }
}

/// `m`: a few seeded notes change. Pitched: a neighbour, an octave, half or
/// twice the length, or gone; drums: a ghost, gone, or a ghost after it.
fn mutate(notes: &mut Vec<SpanNote>, ctx: &OpCtx, k: u8) {
    let a = ctx.amount.clamp(0.0, 1.0);
    let movable = |n: &SpanNote| is_note(n) && !is_anchor(n, ctx);
    let count = notes.iter().filter(|n| movable(n)).count();
    if count == 0 {
        return;
    }
    let changes = ((count as f32 * MUTATE_SHARE * a).round() as usize).max(1);
    let hm = ctx.harmony;
    for c in 0..changes {
        let hc = h(ctx, k, 9, c as u32);
        let pick = rng::below(hc, count as u32) as usize;
        let Some(i) = notes.iter().enumerate().filter(|(_, n)| movable(n)).nth(pick).map(|(i, _)| i) else { continue };
        let kind = rng::below(rng::derive(hc, 1), 4);
        if ctx.drums {
            match kind {
                0 | 1 => notes[i].ev.vel = ((notes[i].ev.vel as f32 * GHOST_SHARE) as u8).max(1),
                2 if notes[i].at % BEAT != 0 => notes[i].ev.vel = 0,
                _ => {
                    let mut ghost = notes[i];
                    ghost.at += STEP;
                    ghost.seq += 1 << 20;
                    ghost.ev.vel = ((ghost.ev.vel as f32 * GHOST_SHARE) as u8).max(1);
                    if ghost.at < ctx.ticks {
                        add(notes, ghost);
                    }
                }
            }
            continue;
        }
        let r = room(notes, i, ctx.ticks);
        let n = &mut notes[i].ev;
        match kind {
            0 => {
                let (d, off) = degree(n.key, &hm);
                let step = if rng::chance(rng::derive(hc, 2), 0.5) { 1 } else { -1 };
                n.key = key_of(d + step, off, &hm);
            }
            1 => {
                let to = n.key as i32 + if rng::chance(rng::derive(hc, 2), 0.5) { 12 } else { -12 };
                if (KEY_RANGE.0..=KEY_RANGE.1).contains(&to) {
                    n.key = to as u8;
                }
            }
            2 if n.gate > 0 => {
                n.gate = if rng::chance(rng::derive(hc, 2), 0.5) { (n.gate / 2).max(STEP as u16 / 2) } else { (n.gate as u32 * 2).min(r) as u16 };
            }
            _ => n.vel = 0,
        }
    }
    // Velocity 0 marks a dropped note.
    notes.retain(|n| !(is_note(n) && n.ev.vel == 0));
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::key::MusicalKey;

    /// C minor, two bars: the root on each downbeat, a rising line on the
    /// other beats (quarter notes).
    fn line() -> Vec<SpanNote> {
        let keys = [36u8, 39, 41, 43, 36, 44, 46, 48];
        let mut v = Vec::with_capacity(64);
        for (i, k) in keys.iter().enumerate() {
            let ev = LiveEvent { kind: EventKind::NoteOn, key: *k, vel: 100, gate: 72, ..Default::default() };
            v.push(SpanNote { at: i as u32 * BEAT, seq: i as u32, ev });
        }
        v
    }

    fn ctx(amount: f32) -> OpCtx {
        let harmony = HarmonyCtx { key: MusicalKey::new(0, true), chord_degree: 0, progression: 0, mode: 0 };
        OpCtx { ticks: 2 * BAR, amount, seed: 7, lane: 1, drums: false, harmony }
    }

    fn run(op: Op, amount: f32, notes: &[SpanNote]) -> Vec<SpanNote> {
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(notes);
        let mut chain = OpChain::default();
        chain.push(op);
        apply(&chain, &mut v, &ctx(amount));
        v
    }

    fn keys(v: &[SpanNote]) -> Vec<u8> {
        v.iter().map(|n| n.ev.key).collect()
    }

    #[test]
    fn pitch_operators_keep_the_downbeat_roots_and_scale_with_the_amount() {
        let src = line();
        let line_keys = |v: &[SpanNote]| v.iter().filter(|n| n.at % BAR != 0).map(|n| n.ev.key).collect::<Vec<_>>();
        for op in [Op::Retrograde, Op::Inversion, Op::Sequence, Op::Counter, Op::OctaveUp, Op::OctaveDown, Op::Question, Op::Answer] {
            for amount in [0.0, 1.0] {
                let v = run(op, amount, &src);
                assert_eq!(v.iter().map(|n| n.at).collect::<Vec<_>>(), src.iter().map(|n| n.at).collect::<Vec<_>>(), "{op:?}: the rhythm stays");
                assert!(v.iter().filter(|n| n.at % BAR == 0).all(|n| n.ev.key == 36), "{op:?}: the roots stay");
            }
            let (little, full) = (line_keys(&run(op, 0.0, &src)), line_keys(&run(op, 1.0, &src)));
            let changed = |k: &[u8]| k.iter().zip(line_keys(&src)).filter(|(a, b)| **a != *b).count();
            assert!(changed(&full) >= changed(&little) && changed(&full) > 0, "{op:?}: {little:?} .. {full:?}");
        }
        // Retrograde in full: the line backwards.
        assert_eq!(line_keys(&run(Op::Retrograde, 1.0, &src)), [48, 46, 44, 43, 41, 39]);
        // Inversion in full mirrors every step in the scale around the first
        // note (C minor: Eb F G Ab Bb C becomes Eb D C Bb Ab G).
        assert_eq!(line_keys(&run(Op::Inversion, 1.0, &src)), [39, 38, 36, 34, 32, 31]);
        // The answer ends on the root, held to the end; the question on the
        // 2nd or the 5th, shorter.
        let a = run(Op::Answer, 0.0, &src);
        let last = a.last().unwrap();
        assert_eq!((last.ev.key % 12, last.at + last.ev.gate as u32), (0, 2 * BAR));
        let q = run(Op::Question, 0.0, &src);
        let last = q.last().unwrap();
        assert!([2, 7].contains(&(last.ev.key % 12)) && last.ev.gate < 72, "{:?}", last.ev);
        // Octaves: up an octave.
        assert!(line_keys(&run(Op::OctaveUp, 1.0, &src)).iter().zip(line_keys(&src)).all(|(a, b)| *a == b + 12));
    }

    fn ats(v: &[SpanNote]) -> Vec<u32> {
        v.iter().map(|n| n.at).collect()
    }

    #[test]
    fn time_operators_move_onsets_and_lengths() {
        let src = line();
        let pairs = |v: &[SpanNote]| v.iter().map(|n| (n.at, n.ev.key)).collect::<Vec<_>>();
        // b: the window backwards in time, every note with its pitch.
        let b = run(Op::Reverse, 1.0, &src);
        assert_ne!(ats(&b), ats(&src));
        let line_keys = |v: &[SpanNote]| v.iter().filter(|n| n.at % BAR != 0).map(|n| n.ev.key).collect::<Vec<_>>();
        let mut back = line_keys(&src);
        back.reverse();
        assert_eq!(line_keys(&b), back, "the notes play backwards");
        // v: backwards and mirrored.
        assert_ne!(pairs(&run(Op::RetroInversion, 1.0, &src)), pairs(&b));
        // h: half time, the first half stretched; x: double time, twice.
        let h = run(Op::HalfTime, 1.0, &src);
        assert_eq!(h.len(), src.len() / 2);
        assert!(h.iter().all(|n| n.at % (2 * BEAT) == 0 && n.ev.gate == 144));
        let x = run(Op::DoubleTime, 1.0, &src);
        assert_eq!(x.len(), 2 * src.len());
        // z: an 8th later, the window's start rests.
        let z = run(Op::Displacement, 1.0, &src);
        assert!(z.iter().all(|n| n.at % BEAT == 2 * STEP), "{:?}", ats(&z));
        // j and F: the first cell repeated over the window.
        let mut busy = line();
        for k in 0..8u32 {
            let ev = LiveEvent { kind: EventKind::NoteOn, key: 50, vel: 90, gate: 24, ..Default::default() };
            busy.push(SpanNote { at: k * BEAT + 2 * STEP, seq: 100 + k, ev });
        }
        sort(&mut busy);
        for (op, cell) in [(Op::Hemiola, HEMIOLA_CELL), (Op::Fragment, FRAGMENT)] {
            let v = run(op, 1.0, &busy);
            let (from, _) = window(&ctx(1.0), BAR);
            let first: Vec<u32> = v.iter().filter(|n| n.at >= from && n.at < from + cell).map(|n| n.at - from).collect();
            for n in v.iter().filter(|n| n.at >= from) {
                assert!(first.contains(&((n.at - from) % cell)), "{op:?}: {:?}", ats(&v));
            }
        }
        // g: cut short, a rest to the end.
        let g = run(Op::Truncation, 1.0, &src);
        assert!(g.len() < src.len() && g.iter().all(|n| n.at + n.ev.gate as u32 <= 2 * BAR * 3 / 4));
        // K: the second half answers the first, home at its end.
        let k = run(Op::CallResponse, 1.0, &src);
        assert_eq!(ats(&k), ats(&src));
        assert_eq!(k.last().unwrap().ev.key % 12, 0, "the answer comes home");
        // p and N: passing and neighbour tones between the notes.
        assert!(run(Op::Passing, 1.0, &src).len() > src.len());
        assert!(run(Op::Neighbour, 1.0, &src).len() > src.len());
        // m: at least one note changes, more in full.
        let diff = |v: &[SpanNote]| src.len().abs_diff(v.len()) + v.iter().zip(&src).filter(|(a, b)| a.ev != b.ev).count();
        let (little, full) = (diff(&run(Op::Mutate, 0.0, &src)), diff(&run(Op::Mutate, 1.0, &src)));
        assert!(little >= 1 && full >= little, "{little} .. {full}");
    }

    #[test]
    fn line_operators_on_the_harmony() {
        let src = line();
        let line_keys = |v: &[SpanNote]| v.iter().filter(|n| n.at % BAR != 0).map(|n| n.ev.key).collect::<Vec<_>>();
        // P: every other note of the line on the root.
        let p = run(Op::Pedal, 1.0, &src);
        assert!(p.iter().skip(1).step_by(2).all(|n| n.ev.key % 12 == 0), "{:?}", keys(&p));
        // Y: the line on the chord's tones (C minor: C Eb G).
        let y = run(Op::Arpeggio, 1.0, &src);
        assert!(line_keys(&y).iter().all(|k| [0, 3, 7].contains(&(k % 12))), "{:?}", line_keys(&y));
    }

    #[test]
    fn operators_chain_left_to_right() {
        let src = line();
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(&src);
        let mut chain = OpChain::default();
        chain.push(Op::Retrograde);
        chain.push(Op::Displacement);
        apply(&chain, &mut v, &ctx(1.0));
        let both = v.clone();
        let mut w = run(Op::Retrograde, 1.0, &src);
        let mut d = OpChain::default();
        d.push(Op::Displacement);
        apply(&d, &mut w, &ctx(1.0));
        assert_ne!(ats(&both), ats(&src));
        assert_eq!(keys(&both).len(), keys(&w).len());
    }

    #[test]
    fn pitch_operators_leave_drum_hits() {
        let hits: Vec<SpanNote> = line().into_iter().map(|mut n| {
            n.ev.kind = EventKind::Trigger;
            n.ev.key = 0;
            n
        }).collect();
        for op in [Op::Inversion, Op::Sequence, Op::Counter, Op::OctaveUp, Op::OctaveDown, Op::Passing, Op::Neighbour, Op::Pedal, Op::Arpeggio, Op::RetroInversion] {
            let mut v = Vec::with_capacity(64);
            v.extend_from_slice(&hits);
            let mut chain = OpChain::default();
            chain.push(op);
            apply(&chain, &mut v, &OpCtx { drums: true, ..ctx(1.0) });
            assert_eq!(v, hits, "{op:?}");
        }
    }

    fn kit() -> Vec<SpanNote> {
        // Kick, hats on the 8ths, snare on 2 and 4, two bars.
        let mut v = Vec::with_capacity(128);
        for bar in 0..2u32 {
            for step in 0..16u32 {
                let at = bar * BAR + step * STEP;
                let target = match step {
                    0 | 10 => Some(0),
                    4 | 12 => Some(1),
                    s if s % 2 == 0 => Some(2),
                    _ => None,
                };
                if let Some(t) = target {
                    let ev = LiveEvent { kind: EventKind::Trigger, target: t, vel: 100, ..Default::default() };
                    v.push(SpanNote { at, seq: v.len() as u32, ev });
                }
            }
        }
        v
    }

    fn drums(op: Op, amount: f32) -> Vec<SpanNote> {
        let mut v = Vec::with_capacity(128);
        v.extend_from_slice(&kit());
        let mut chain = OpChain::default();
        chain.push(op);
        apply(&chain, &mut v, &OpCtx { drums: true, ..ctx(amount) });
        v
    }

    #[test]
    fn drum_operators_have_their_own_meanings() {
        let src = kit();
        // m: ghosts, drops or a ghost after a hit.
        let m = drums(Op::Mutate, 1.0);
        assert!(m.len() != src.len() || m.iter().any(|n| n.ev.vel < 100));
        // z: an 8th displacement.
        let o = drums(Op::Displacement, 1.0);
        let hits = |v: &[SpanNote]| v.iter().map(|n| (n.at, n.ev.target)).collect::<Vec<_>>();
        assert_ne!(hits(&o), hits(&src));
        assert!(o.iter().all(|n| n.at % STEP == 0));
        // q: the ending opens (the last hits gone); a: a strong hit on the
        // last beat closes it.
        let q = drums(Op::Question, 1.0);
        assert!(q.iter().all(|n| n.at < 2 * BAR - BEAT) && q.len() < src.len());
        let a = drums(Op::Answer, 1.0);
        let last = a.iter().filter(|n| n.at >= 2 * BAR - BEAT).collect::<Vec<_>>();
        assert!(last.len() == 1 && last[0].ev.vel >= 110 && last[0].ev.target == 0, "{last:?}");
        // r and b: the kit backwards in time.
        for op in [Op::Retrograde, Op::Reverse] {
            assert_ne!(ats(&drums(op, 1.0)), ats(&src), "{op:?}");
        }
    }

    #[test]
    fn the_seeded_operator_follows_the_place() {
        let most = |place: Place| {
            let mut count = std::collections::BTreeMap::new();
            for i in 0..400u64 {
                let op = seeded(rng::derive(i, 3), place);
                assert_ne!(op, Op::Seeded);
                *count.entry(op.as_char()).or_insert(0) += 1;
            }
            count.into_iter().max_by_key(|(_, n)| *n).unwrap().0
        };
        assert_eq!(most(Place { period_end: true, ..Place::default() }), 'a', "answers end periods");
        assert_eq!(most(Place { repeat: true, ..Place::default() }), 'm', "repeats mutate");
        assert_eq!(most(Place { before_change: true, period_end: true, ..Place::default() }), 'x', "double time before a change");
    }

    #[test]
    fn melodic_picks_never_seed_again() {
        for place in [Place::default(), Place { repeat: true, ..Place::default() }, Place { period_end: true, ..Place::default() }, Place { before_change: true, ..Place::default() }] {
            for i in 0..400u64 {
                assert_ne!(seeded_melodic(rng::derive(i, 4), place), Op::Seeded);
            }
        }
    }

    #[test]
    fn every_operator_has_its_own_mark() {
        for op in Op::ALL {
            assert_eq!(Op::from_char(op.as_char()), Some(op));
        }
        let marks: std::collections::BTreeSet<char> = Op::ALL.iter().map(|o| o.as_char()).collect();
        assert_eq!(marks.len(), Op::ALL.len());
        assert!(Op::from_char('\'').is_none() && Op::from_char('*').is_none() && Op::from_char('7').is_none());
        // D57: the timing marks are gone.
        for c in ['d', 'e', 'o', 'R', 'y', 'l', 's', 'n', 'k'] {
            assert!(Op::from_char(c).is_none(), "{c}");
        }
    }

    #[test]
    fn chains_keep_their_order_and_swap_questions() {
        let mut c = OpChain::default();
        for op in [Op::Retrograde, Op::Reverse, Op::Question] {
            assert!(c.push(op));
        }
        assert_eq!(c.to_string(), "rbq");
        assert_eq!(c.swap_question_answer().to_string(), "rba");
        let mut full = OpChain::default();
        assert!((0..MAX_OPS).all(|_| full.push(Op::Mutate)));
        assert!(!full.push(Op::Mutate));
    }
}
