//! Form operators (design D47): related variants of a letter's motif.
//!
//! A form writes them as marks after the letter, `A3 Ar1 A3 Ai1 B2 Bq1`:
//! the segment plays its letter's first bars transformed. Marks chain left
//! to right (`A3rd`: reversed, then dotted); a later mark wins where two
//! set the same thing. `'` (another second half) and `*` (another sound)
//! stay what they were. Pure and allocation-free.

use crate::event::{flags, EventKind, LiveEvent};
use crate::harmony::HarmonyCtx;
use crate::{rng, TICKS_PER_BAR, TICKS_PER_STEP};

/// One operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    /// `r`: the pitch order reversed, the rhythm kept.
    Retrograde,
    /// `i`: the contour mirrored inside the scale.
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
    /// `R`: the rhythm reversed, the pitches kept.
    RhythmRetrograde,
    /// `o`: rotated by a seeded number of 16ths.
    Rotation,
    /// `d`: onsets on a dotted-8th grid.
    Dotted,
    /// `e`: onsets on 8th triplets.
    Triplets,
    /// `y`: onsets on the beats pushed to the 16th before (anticipation).
    Syncopate,
    /// `h`: half time (the span's first half stretched over it).
    HalfTime,
    /// `x`: double time (the span compressed and played twice).
    DoubleTime,
    /// `l` / `s`: notes held into the next, or short.
    Legato,
    Staccato,
    /// `n`: weak notes dropped, more space.
    Thin,
    /// `p`: passing notes between wider steps.
    Passing,
    /// `m`: a few seeded notes change (pitch, length or presence).
    Mutate,
    /// `?`: the seed picks one, by where the segment sits in the form.
    Seeded,
}

impl Op {
    pub const ALL: [Op; 21] = [
        Op::Retrograde,
        Op::Inversion,
        Op::Sequence,
        Op::Counter,
        Op::OctaveUp,
        Op::OctaveDown,
        Op::Question,
        Op::Answer,
        Op::RhythmRetrograde,
        Op::Rotation,
        Op::Dotted,
        Op::Triplets,
        Op::Syncopate,
        Op::HalfTime,
        Op::DoubleTime,
        Op::Legato,
        Op::Staccato,
        Op::Thin,
        Op::Passing,
        Op::Mutate,
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
            Op::RhythmRetrograde => 'R',
            Op::Rotation => 'o',
            Op::Dotted => 'd',
            Op::Triplets => 'e',
            Op::Syncopate => 'y',
            Op::HalfTime => 'h',
            Op::DoubleTime => 'x',
            Op::Legato => 'l',
            Op::Staccato => 's',
            Op::Thin => 'n',
            Op::Passing => 'p',
            Op::Mutate => 'm',
            Op::Seeded => '?',
        }
    }

    pub fn from_char(c: char) -> Option<Self> {
        Op::ALL.iter().copied().find(|o| o.as_char() == c)
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
// before a section change). The weights of each place.
const SEEDED_BEFORE_CHANGE: [(Op, f32); 5] = [(Op::DoubleTime, 3.0), (Op::HalfTime, 1.0), (Op::Dotted, 1.0), (Op::Triplets, 1.0), (Op::Thin, 1.0)];
const SEEDED_PERIOD_END: [(Op, f32); 5] = [(Op::Answer, 3.0), (Op::Inversion, 2.0), (Op::Question, 1.0), (Op::Mutate, 1.0), (Op::Retrograde, 1.0)];
const SEEDED_REPEAT: [(Op, f32); 5] = [(Op::Mutate, 3.0), (Op::Retrograde, 1.0), (Op::Rotation, 1.0), (Op::Syncopate, 1.0), (Op::Legato, 1.0)];
const SEEDED_ANY: [(Op, f32); 11] = [
    (Op::Retrograde, 1.0),
    (Op::Inversion, 1.0),
    (Op::Sequence, 1.0),
    (Op::RhythmRetrograde, 1.0),
    (Op::Rotation, 1.0),
    (Op::Syncopate, 1.0),
    (Op::Legato, 1.0),
    (Op::Staccato, 1.0),
    (Op::Mutate, 2.0),
    (Op::Dotted, 1.0),
    (Op::Passing, 1.0),
];

/// Where a segment sits in its form, for [`seeded`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Place {
    pub period_end: bool,
    pub repeat: bool,
    pub before_change: bool,
}

/// The operator `?` stands for, from a seed and the segment's place.
pub fn seeded(h: u64, place: Place) -> Op {
    let table: &[(Op, f32)] = if place.before_change {
        &SEEDED_BEFORE_CHANGE
    } else if place.period_end {
        &SEEDED_PERIOD_END
    } else if place.repeat {
        &SEEDED_REPEAT
    } else {
        &SEEDED_ANY
    };
    let mut weights = [0.0f32; 11];
    for (w, (_, x)) in weights.iter_mut().zip(table) {
        *w = *x;
    }
    table[rng::weighted_pick(h, &weights[..table.len()]).unwrap_or(0)].0
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
/// Rotation: the most 16ths a seed rotates by.
const ROTATE_STEPS_MAX: i32 = 16;
/// Share of the notes the grid operators (d, e, y), legato and thin move,
/// at the least and in full.
const GRID_SHARE: (f32, f32) = (0.3, 1.0);
const LEGATO_SHARE: (f32, f32) = (0.3, 1.0);
const THIN_SHARE: (f32, f32) = (0.15, 0.6);
const PASSING_SHARE: (f32, f32) = (0.2, 1.0);
/// Staccato: the gate factor at the least and in full.
const STACCATO_FACTOR: (f32, f32) = (0.7, 0.3);
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
fn degree(key: u8, harmony: &HarmonyCtx) -> (i32, i32) {
    let scale = harmony.scale();
    let rel = key as i32 - harmony.key.root as i32;
    let (oct, pc) = (rel.div_euclid(12), rel.rem_euclid(12));
    let idx = (0..7).rev().find(|&i| scale[i] as i32 <= pc).unwrap_or(0);
    (oct * 7 + idx as i32, pc - scale[idx] as i32)
}

fn key_of(deg: i32, off: i32, harmony: &HarmonyCtx) -> u8 {
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
        Op::Retrograde if ctx.drums => rhythm_retrograde(notes, ctx),
        Op::Retrograde => {
            // The pitches of the window, in reverse order.
            let w = window(ctx, BEAT);
            let idx = |notes: &[SpanNote]| notes.iter().enumerate().filter(|(_, n)| is_line(n, ctx) && in_window(n, w)).map(|(i, _)| i).collect_array();
            let (ids, len) = idx(notes);
            for j in 0..len / 2 {
                let (x, y) = (ids[j], ids[len - 1 - j]);
                let kx = notes[x].ev.key;
                notes[x].ev.key = notes[y].ev.key;
                notes[y].ev.key = kx;
            }
        }
        Op::Inversion if !ctx.drums => {
            let f = INVERT_LEAST + (1.0 - INVERT_LEAST) * a;
            let Some(axis) = notes.iter().find(|n| is_line(n, ctx)).map(|n| degree(n.ev.key, hm).0) else { return };
            for n in notes.iter_mut().filter(|n| is_line(n, ctx)) {
                let (d, off) = degree(n.ev.key, hm);
                n.ev.key = key_of(axis - ((d - axis) as f32 * f).round() as i32, off, hm);
            }
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
            let (d, off) = degree(notes[last].ev.key, hm);
            let goal = nearest(d, target);
            let _ = off;
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
        Op::RhythmRetrograde => rhythm_retrograde(notes, ctx),
        Op::Rotation => {
            let (from, to) = if ctx.drums { window(ctx, BAR) } else { (0, ctx.ticks) };
            let steps = if ctx.drums {
                2
            } else {
                let most = 2 + rng::below(h(ctx, k, 4, 0), (ROTATE_STEPS_MAX - 1) as u32) as i32;
                (1.0 + (most - 1) as f32 * a).round() as i32
            };
            let len = (to - from).max(1);
            for n in notes.iter_mut().filter(|n| in_window(n, (from, to))) {
                n.at = from + (n.at - from + steps as u32 * STEP) % len;
            }
        }
        Op::Dotted | Op::Triplets => {
            let grid = if op == Op::Dotted { 3 * STEP } else { BAR / 12 };
            let share = lerp(GRID_SHARE, a);
            for (i, n) in notes.iter_mut().enumerate() {
                // (A downbeat is on every grid; its length moves too.)
                if !is_note(n) || !rng::chance(h(ctx, k, 5, i as u32), share) {
                    continue;
                }
                n.at = (((n.at + grid / 2) / grid) * grid).min(ctx.ticks.saturating_sub(1));
                if n.ev.gate > 0 {
                    n.ev.gate = ((n.ev.gate as u32 + grid / 2) / grid * grid).max(grid) as u16;
                }
            }
            dedupe(notes);
        }
        Op::Syncopate => {
            let share = lerp(GRID_SHARE, a);
            for (i, n) in notes.iter_mut().enumerate() {
                if is_note(n) && n.at % BEAT == 0 && n.at % BAR != 0 && rng::chance(h(ctx, k, 6, i as u32), share) {
                    n.at -= STEP;
                    if n.ev.gate > 0 {
                        n.ev.gate += STEP as u16;
                    }
                }
            }
            dedupe(notes);
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
        Op::Legato if !ctx.drums => {
            let share = lerp(LEGATO_SHARE, a);
            sort(notes);
            for i in 0..notes.len() {
                if notes[i].ev.kind != EventKind::NoteOn || notes[i].ev.gate == 0 {
                    continue;
                }
                let r = room(notes, i, ctx.ticks);
                let g = notes[i].ev.gate as u32;
                if r > g {
                    notes[i].ev.gate = (g as f32 + (r - g) as f32 * share).round() as u16;
                }
            }
        }
        Op::Staccato => {
            let f = lerp(STACCATO_FACTOR, a);
            for n in notes.iter_mut().filter(|n| is_note(n) && n.ev.gate > 0) {
                n.ev.gate = ((n.ev.gate as f32 * f).round() as u16).max(STEP as u16 / 2);
                n.ev.loop_len = 0;
            }
        }
        Op::Thin => {
            let p = lerp(THIN_SHARE, a);
            let mut i = 0u32;
            notes.retain(|n| {
                i += 1;
                !(is_note(n) && n.at % BEAT != 0 && !is_anchor(n, ctx) && rng::chance(h(ctx, k, 7, i), p))
            });
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
                let mut pass = notes[i];
                pass.at = at;
                pass.seq += 1 << 20;
                pass.ev.key = key_of(from + (to - from).signum(), 0, hm);
                pass.ev.gate = (notes[j].at - at).min(u16::MAX as u32) as u16;
                pass.ev.note_id = pass.ev.note_id.wrapping_add(ADDED_ID);
                pass.ev.flags &= !(flags::TIE | flags::ACCENT);
                pass.ev.loop_len = 0;
                notes[i].ev.gate = notes[i].ev.gate.min((at - notes[i].at) as u16);
                add(notes, pass);
            }
        }
        Op::Mutate => mutate(notes, ctx, k),
        // Pitch and length operators on the drums.
        _ => {}
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

/// `R` (and `r` on the drums): the window's rhythm mirrored; a pitched
/// lane keeps its pitch order.
fn rhythm_retrograde(notes: &mut Vec<SpanNote>, ctx: &OpCtx) {
    let (from, to) = window(ctx, BEAT);
    let moves = |n: &SpanNote| is_note(n) && in_window(n, (from, to)) && !is_anchor(n, ctx);
    let (ids, len) = notes.iter().enumerate().filter(|(_, n)| moves(n)).map(|(i, _)| i).collect_array();
    let mut keys = [0u8; MAX_LINE];
    for (j, &i) in ids[..len].iter().enumerate() {
        keys[j] = notes[i].ev.key;
        let n = &mut notes[i];
        let dur = if n.ev.gate > 0 { (n.ev.gate as u32).min(to - n.at) } else { STEP };
        n.at = (from + to).saturating_sub(n.at + dur).max(from);
    }
    if !ctx.drums {
        // The pitches in their order on the mirrored rhythm.
        let (mut slots, _) = ids[..len].iter().copied().collect_array();
        slots[..len].sort_unstable_by_key(|&i| (notes[i].at, notes[i].seq));
        for (j, &i) in slots[..len].iter().enumerate() {
            notes[i].ev.key = keys[j];
        }
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
    fn rhythm_operators_move_onsets_and_lengths() {
        let src = line();
        // R: the window's rhythm mirrored, the pitch order kept.
        let r = run(Op::RhythmRetrograde, 1.0, &src);
        assert_ne!(ats(&r), ats(&src));
        let line_keys = |v: &[SpanNote]| v.iter().filter(|n| n.at % BAR != 0).map(|n| n.ev.key).collect::<Vec<_>>();
        assert_eq!(line_keys(&r), line_keys(&src), "pitch order kept");
        // o: rotated by whole 16ths.
        let o = run(Op::Rotation, 1.0, &src);
        let pairs = |v: &[SpanNote]| v.iter().map(|n| (n.at, n.ev.key)).collect::<Vec<_>>();
        assert!(o.iter().all(|n| n.at % STEP == 0) && pairs(&o) != pairs(&src));
        // d and e: onsets on the dotted-8th or the 8th-triplet grid.
        for (op, grid) in [(Op::Dotted, 3 * STEP), (Op::Triplets, BAR / 12)] {
            let v = run(op, 1.0, &src);
            assert!(v.iter().all(|n| n.at % grid == 0 || n.at % BAR == 0), "{op:?}: {:?}", ats(&v));
            assert!(v.iter().all(|n| n.ev.gate as u32 % grid == 0), "{op:?} lengths");
        }
        // y: the beats (not the downbeats) a 16th early.
        let y = run(Op::Syncopate, 1.0, &src);
        assert!(y.iter().filter(|n| n.at % BAR != 0).all(|n| n.at % BEAT == BEAT - STEP), "{:?}", ats(&y));
        // h: half time, the first half stretched; x: double time, twice.
        let h = run(Op::HalfTime, 1.0, &src);
        assert_eq!(h.len(), src.len() / 2);
        assert!(h.iter().all(|n| n.at % (2 * BEAT) == 0 && n.ev.gate == 144));
        let x = run(Op::DoubleTime, 1.0, &src);
        assert_eq!(x.len(), 2 * src.len());
        // l: held into the next note; s: short.
        assert!(run(Op::Legato, 1.0, &src).iter().all(|n| n.ev.gate as u32 == BEAT));
        assert!(run(Op::Staccato, 1.0, &src).iter().all(|n| n.ev.gate < 30));
        // n: fewer notes, the beats and roots stay; p: passing notes.
        let mut off = line();
        for n in off.iter_mut().skip(1) {
            n.at += 2 * STEP;
        }
        assert!(run(Op::Thin, 1.0, &off).len() < off.len());
        assert!(run(Op::Passing, 1.0, &src).len() > src.len());
        // m: at least one note changes, more in full.
        let diff = |v: &[SpanNote]| src.len().abs_diff(v.len()) + v.iter().zip(&src).filter(|(a, b)| a.ev != b.ev).count();
        let (little, full) = (diff(&run(Op::Mutate, 0.0, &src)), diff(&run(Op::Mutate, 1.0, &src)));
        assert!(little >= 1 && full >= little, "{little} .. {full}");
    }

    #[test]
    fn operators_chain_left_to_right() {
        let src = line();
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(&src);
        let mut chain = OpChain::default();
        chain.push(Op::Retrograde);
        chain.push(Op::Dotted);
        apply(&chain, &mut v, &ctx(1.0));
        let both = v.clone();
        let mut w = run(Op::Retrograde, 1.0, &src);
        let mut d = OpChain::default();
        d.push(Op::Dotted);
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
        for op in [Op::Inversion, Op::Sequence, Op::Counter, Op::OctaveUp, Op::OctaveDown, Op::Legato, Op::Passing] {
            let mut v = Vec::with_capacity(64);
            v.extend_from_slice(&hits);
            let mut chain = OpChain::default();
            chain.push(op);
            apply(&chain, &mut v, &OpCtx { drums: true, ..ctx(1.0) });
            assert_eq!(v, hits, "{op:?}");
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
    fn every_operator_has_its_own_mark() {
        for op in Op::ALL {
            assert_eq!(Op::from_char(op.as_char()), Some(op));
        }
        let marks: std::collections::BTreeSet<char> = Op::ALL.iter().map(|o| o.as_char()).collect();
        assert_eq!(marks.len(), Op::ALL.len());
        assert!(Op::from_char('\'').is_none() && Op::from_char('*').is_none() && Op::from_char('7').is_none());
    }

    #[test]
    fn chains_keep_their_order_and_swap_questions() {
        let mut c = OpChain::default();
        for op in [Op::Retrograde, Op::Dotted, Op::Question] {
            assert!(c.push(op));
        }
        assert_eq!(c.to_string(), "rdq");
        assert_eq!(c.swap_question_answer().to_string(), "rda");
        let mut full = OpChain::default();
        assert!((0..MAX_OPS).all(|_| full.push(Op::Mutate)));
        assert!(!full.push(Op::Mutate));
    }
}
