//! Form operators (design D47): related variants of a letter's motif.
//!
//! A form writes them as marks after the letter, `A3 Ar1 A3 Ai1 B2 Bq1`:
//! the segment plays its letter's first bars transformed. Marks chain left
//! to right (`A3rd`: reversed, then dotted); a later mark wins where two
//! set the same thing. `'` (another second half) and `*` (another sound)
//! stay what they were. Pure and allocation-free.

use crate::event::LiveEvent;
use crate::harmony::HarmonyCtx;

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

fn apply_one(_op: Op, _k: u8, _notes: &mut Vec<SpanNote>, _ctx: &OpCtx) {}

#[cfg(test)]
mod tests {
    use super::*;

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
