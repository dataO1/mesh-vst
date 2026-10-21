//! Lane forms: which seed plays in which bar (design §6.6).
//!
//! Every lane has one **master seed**. Four letter seeds are derived from it:
//! A and B are **main** seeds (repetitive, mostly on the grid, normal sounds),
//! C and D are **filler** seeds (triplets, heavy swing, push and pull, busier
//! patterns, odd sample picks). A **form** says which letter plays in which
//! bar, as segments of a letter and a bar count: `A15 C1` is fifteen bars of
//! A and one bar of C, 16 bars in total, repeating. A primed letter (`A'`)
//! plays the same seed with a different second half, so long forms develop
//! without changing the idea.
//!
//! Forms are aligned to the global bar (`bar % form length`), so every lane's
//! form starts on the same bars. Everything here is pure and allocation-free
//! once a [`Form`] is built.

use std::fmt;

use crate::event::{BarEvents, EventKind, LiveEvent};
use crate::harmony::seeded_chord;
use crate::ops::{Op, OpChain};
use crate::rng;
use crate::{TICKS_PER_BAR, TICKS_PER_STEP};

/// Longest form in bars.
pub const MAX_FORM_BARS: u16 = 64;
/// Bars of a phrase; a form change lands on the next phrase boundary.
pub const FORM_CHANGE_BARS: u64 = 4;

// MUSICAL: tune by ear (user, 2026-10-21: forms are the deterministic big
// picture, 64 bars a pass, the second pass varied; A and B are distinct
// motifs that ask and answer, C and D the stark changes at the transitions;
// related variants of a letter come from its operators, often and in short
// steps; focused repetition, not random variation; each form its own
// character; music-theory §1 §4 §5). One line per 16 bars, each ending on
// a transition. Marks: `'` another second half, `*` another sound, the
// operators of D47 (`q` asks, `a` answers, `m` mutates a repeat, `x` and
// `h` double or halve the time before a change, ...).
pub const DEFAULT_FORMS: [&str; 10] = [
    // Steady roller: A repeats, a mutation on the 4th bar, B answers.
    concat!(
        "A3 Am1 A3 Ba1 A3 Am1 A2 A'1 C1 ",
        "A3 Ar1 A3 Ba1 A3 Am1 A3 D1 ",
        "B3 Bm1 B3 Aa1 B3 Bi1 B2 B'1 C*1 ",
        "A3 Am1 A3 Ba1 A2 Ax2 A2 Aq.5 C.5 D*1"
    ),
    // Question and answer: two-bar periods, A asks and B answers in the
    // last half bar, the answers developing.
    concat!(
        "A1.5 Aq.5 B1.5 Ba.5 A1.5 Aq.5 B1.5 Bia.5 A1.5 Aq.5 B1.5 Ba.5 A1.5 Ar.5 C2 ",
        "A1.5 Aq.5 Bt1.5 Ba.5 A1.5 Aq.5 Bc1.5 Ba.5 A3 Ai1 B3 D1 ",
        "B1.5 Bq.5 A1.5 Aa.5 B1.5 Bq.5 Ai1.5 Aa.5 B2 Bm2 C*2 D2 ",
        "A1.5 Aq.5 B1.5 Ba.5 A1.5 Aqy.5 B1.5 Ba.5 A1 Ax1 Am1 Aa1 C2 D*2"
    ),
    // Slow build: eight plain bars, then passing notes and double time
    // creep in, the last 16 changing every bar.
    concat!(
        "A8 A7 Am1 ",
        "A4 A'4 A3 Ap1 A3 Am1 ",
        "A2 Am2 A2 Ap2 B2 Bp2 A2 Ax1 C1 ",
        "A1 Ax1 A1 Ap1 B1 Bx1 A1 Ad1 A1 Ax1 B1 By1 C1 Cx1 D*1 Dx1"
    ),
    // Two-step cells: 4-bar cells whose last half bar pushes (syncopated or
    // rotated), the third phrase in 2-bar cells.
    concat!(
        "A3.5 Ay.5 A3.5 Bo.5 A3.5 Ay.5 A3 C1 ",
        "B3.5 By.5 B3.5 Ao.5 B'3.5 Bq.5 B3 D1 ",
        "A2 Ao2 A2 AR2 B2 Bo2 A2 C*.5 Cx.5 D1 ",
        "A3.5 Ay.5 B3.5 Ba.5 A3.5 Aoy.5 A2 Ax1 D*1"
    ),
    // Breakdown: a first drop, 16 bars of the fillers in half time and held
    // long, the drop back with answers.
    concat!(
        "A4 A'4 A3 Aq1 A3 C1 ",
        "C'h4 Ch4 Dh4 Dhl3.5 Ax.5 ",
        "A3 Aa1 A3 Bi1 A3 Am1 A2 Ax1 C1 ",
        "B3 Ba1 A3 Ai1 B3 Bq1 Ax1 C1 D*2"
    ),
    // B-led: A opens and asks, B carries the rest, A answers it.
    concat!(
        "A4 Aq1 B3 Ba1 B3 Bm1 B2 C1 ",
        "B3 Am1 B3 Br1 B3 Ai1 B3 D1 ",
        "B2 Bt2 B2 B't2 A3 Aa1 B3 C1 ",
        "B3 Ba1 B3 Bi1 B3 Bmx1 B2 C*1 D*1"
    ),
    // Stabs and cells: short notes, cells rotated and pushed every other
    // bar, thinned, the fillers in triplets.
    concat!(
        "As2 A2 As2 Asn2 As2 Bs2 As3 C1 ",
        "As1 Aso1 As1 Asy1 As1 Aso1 As1 Ba1 As1 Asx1 As1 Asy1 As2 Ce1 D1 ",
        "B2 Bs2 B2 Bsd2 A2 As2 Asd2 C*1 D1 ",
        "As1 Aso1 As1 Asy1 Bs1 Bso1 Bs1 Ba1 As1 Asx1 As1 Asdy1 Ce2 Dx1 D*1"
    ),
    // Space: thinned and held (what is left out is the tension), few and
    // long changes.
    concat!("An8 An7 Aln1 ", "An4 Anl4 An3 Ana1 An3 C1 ", "An4 Bnl4 An4 Bna3 D1 ", "An6 Anm2 An4 Anq2 Cn1 Dn1"),
    // Switch-up: 32 bars of the plain motifs, then the second drop re-cuts
    // them (reversed, inverted, dotted, rotated).
    concat!(
        "A7 Aa1 A6 Ax1 C1 ",
        "B7 Ba1 A6 Am1 D1 ",
        "Ar2 Ai2 Ard2 Aia2 Ao2 Ar2 Aex1 C*1 D*2 ",
        "Bi2 Bri2 Ar2 Aa2 Bd2 Bdi2 Ax1 Aq1 C*1 D*1"
    ),
    // Bounce: every other bar pushed, dotted or an octave up, B answering
    // at each 4th bar.
    concat!(
        "A1 Ay1 A1 Au1 A1 Ay1 B1 Ba1 A1 Ay1 A1 Au1 A1 Ad1 C1 Cx1 ",
        "A1 Ad1 A1 Auy1 B1 Bd1 B1 Ba1 A1 Ad1 A1 Auy1 B1 Bdq1 D*1 Dx1 ",
        "B1 By1 B1 Bu1 A1 Ay1 A1 Aa1 B1 By1 B1 Bu1 A1 Ade1 C*1 D1 ",
        "A1 Ay1 A1 Au1 B1 Bd1 B1 Ba1 A1 Ax1 A1 Aux1 C1 Cx1 D*1 Dx1"
    ),
];

/// A seed letter of a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Letter {
    A,
    B,
    C,
    D,
}

impl Letter {
    pub const ALL: [Letter; 4] = [Letter::A, Letter::B, Letter::C, Letter::D];

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn class(self) -> SeedClass {
        match self {
            Letter::A | Letter::B => SeedClass::Main,
            Letter::C | Letter::D => SeedClass::Filler,
        }
    }

    /// The other letter of the same class (A and B, C and D).
    pub fn partner(self) -> Letter {
        match self {
            Letter::A => Letter::B,
            Letter::B => Letter::A,
            Letter::C => Letter::D,
            Letter::D => Letter::C,
        }
    }

    pub fn as_char(self) -> char {
        (b'A' + self as u8) as char
    }

    fn from_char(c: char) -> Option<Self> {
        Some(match c.to_ascii_uppercase() {
            'A' => Letter::A,
            'B' => Letter::B,
            'C' => Letter::C,
            'D' => Letter::D,
            _ => return None,
        })
    }
}

/// What kind of material a letter seed renders (Q23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeedClass {
    /// Repetitive, mostly on the grid, normal sounds (A, B).
    #[default]
    Main,
    /// Triplets, swing, push and pull, busier, odd sample picks (C, D).
    Filler,
}

/// Whether a segment plays (lead and extra forms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Play {
    /// The lane plays the letter.
    #[default]
    All,
    /// `-8`: the lane rests.
    Rest,
    /// `~A8`: the lane plays the letter only in the drums' break bars (their
    /// C and D segments) and rests elsewhere.
    Breaks,
}

/// One segment of a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub letter: Letter,
    pub play: Play,
    /// `A'`: same seed, different second half.
    pub prime: bool,
    /// `A*`: the lane plays another of its sounds here (instrument option or
    /// drum kit, seeded).
    pub swap: bool,
    /// `Ar`, `A2id`: operators on the letter's first bars (D47).
    pub ops: OpChain,
    /// Length in beats (quarter bars): `A7` is 28, `C*.5` is 2.
    pub beats: u16,
    /// Index of the written segment this one comes from, [`NO_SOURCE`] for a
    /// break the form's structure inserted.
    pub source: u8,
}

impl Segment {
    /// Bars the segment touches, rounded up.
    pub fn bars(&self) -> u16 {
        self.beats.div_ceil(BEATS_PER_BAR)
    }
}

/// [`Segment::source`] of an inserted break.
pub const NO_SOURCE: u8 = u8::MAX;
/// Form resolution: segments are whole beats (`.25` of a bar).
pub const BEATS_PER_BAR: u16 = 4;

/// A lane form, e.g. `A7 B1 A'6.5 C*.5 C1`. A pattern shorter than
/// [`FORM_BARS`] repeats to it, with breaks at the structure points
/// ([`STRUCTURE_BREAKS`]). The form then plays a second time as a seeded
/// variation ([`Form::pos_varied`]): every form is a 128-bar cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// As written (display).
    pattern: Vec<Segment>,
    /// The form of the synth lanes (lead and extra), written after a `|`:
    /// `A15 C1 | -16 A16 ~B16 A'16`. None: they play the main form.
    lead: Option<Box<Form>>,
    /// Expanded to [`FORM_BARS`].
    segments: Vec<Segment>,
    /// Beats of one pass.
    beats: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormError {
    #[error("empty form")]
    Empty,
    #[error("bad segment '{0}' (expected a letter A-D, optional ' and *, operators (r i t c u w q a R o d e y h x l s n p m ?) and a length in bars in quarters, in any order, like A7, B'4, C*.5, A2r or Ar2; -8 rests, ~A8 plays only in the drums' breaks; a | starts the synth lanes' form)")]
    Segment(String),
    #[error("form is {0} bars, at most {MAX_FORM_BARS} allowed")]
    TooLong(u32),
}

/// Bars one pass of a form fills (the cycle is twice this).
pub const FORM_BARS: u16 = 64;
/// Bars of a period and of a section, for the seeded operator's view of
/// where a segment sits (D47).
const PERIOD_BARS: u16 = 4;
const SECTION_BARS: u16 = 16;

// MUSICAL: first draft, tune by ear (user: steady base, breaks at the
// 16-bar points, a bigger breakout at the end of 64, like an EDM build).
/// Breaks the structure puts over main bars of a pattern shorter than 64
/// bars: bar range in the pass, letter, and whether the lane swaps its sound.
pub const STRUCTURE_BREAKS: [(u16, u16, Letter, bool); 5] = [
    (15, 16, Letter::C, false),
    (30, 32, Letter::D, false),
    (47, 48, Letter::C, false),
    (60, 62, Letter::C, false),
    (62, 64, Letter::D, true),
];

/// One part of a bar that plays one segment: ticks `from..to` and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarPart {
    pub from: u16,
    pub to: u16,
    pub pos: FormPos,
}

/// Most parts in a bar (one per beat).
pub const MAX_BAR_PARTS: usize = BEATS_PER_BAR as usize;

impl Form {
    /// Parse `"A7 B1 A'6.5 C*.5"`. A segment without a length is one bar;
    /// lengths are bars in quarters (`.25` = one beat).
    pub fn parse(text: &str) -> Result<Self, FormError> {
        if let Some((main, lead)) = text.split_once('|') {
            let mut form = Self::parse(main)?;
            form.lead = Some(Box::new(Self::parse(lead)?));
            return Ok(form);
        }
        let mut pattern = Vec::new();
        let mut total = 0u32;
        for token in text.split_whitespace() {
            let bad = || FormError::Segment(token.to_string());
            // `-8` rests, `~A8` plays only in the drums' breaks.
            let (play, body) = match token.strip_prefix('-') {
                Some(b) => (Play::Rest, b),
                None => match token.strip_prefix('~') {
                    Some(b) => (Play::Breaks, b),
                    None => (Play::All, token),
                },
            };
            let mut chars = body.chars();
            let letter = match chars.clone().next().and_then(Letter::from_char) {
                Some(l) => {
                    chars.next();
                    l
                }
                None if play == Play::Rest => Letter::A,
                None => return Err(bad()),
            };
            // Marks and the length in any order (`A'7`, `A2r`, `Ar2`), the
            // length one run of digits.
            let (mut prime, mut swap, mut ops) = (false, false, OpChain::default());
            let (mut count, mut counted) = (String::new(), false);
            for ch in chars {
                if ch.is_ascii_digit() || ch == '.' {
                    if counted {
                        return Err(bad());
                    }
                    count.push(ch);
                    continue;
                }
                counted |= !count.is_empty();
                match ch {
                    '\'' | '′' => prime = true,
                    '*' => swap = true,
                    c => match Op::from_char(c) {
                        Some(op) if play != Play::Rest && ops.push(op) => {}
                        _ => return Err(bad()),
                    },
                }
            }
            let rest = count.as_str();
            let beats = if rest.is_empty() {
                BEATS_PER_BAR as u32
            } else {
                let bars: f32 = rest.parse().map_err(|_| bad())?;
                let beats = bars * BEATS_PER_BAR as f32;
                if !beats.is_finite() || (beats - beats.round()).abs() > 1e-3 || beats.round() < 1.0 {
                    return Err(bad());
                }
                beats.round() as u32
            };
            total += beats;
            if total > (MAX_FORM_BARS * BEATS_PER_BAR) as u32 {
                return Err(FormError::TooLong(total.div_ceil(BEATS_PER_BAR as u32)));
            }
            let source = pattern.len().min(NO_SOURCE as usize - 1) as u8;
            pattern.push(Segment { letter, play, prime, swap, ops, beats: beats as u16, source });
        }
        if pattern.is_empty() {
            return Err(FormError::Empty);
        }
        let segments = expand(&pattern, total as u16);
        Ok(Self { pattern, lead: None, segments, beats: FORM_BARS * BEATS_PER_BAR })
    }

    /// One bar of A forever (a style without forms).
    pub fn single() -> Self {
        let seg = Segment { letter: Letter::A, play: Play::All, prime: false, swap: false, ops: OpChain::default(), beats: BEATS_PER_BAR, source: 0 };
        Self { pattern: vec![seg], lead: None, segments: vec![seg], beats: BEATS_PER_BAR }
    }

    /// The form a lane plays: synth lanes (lead, extra) play the `|` part when
    /// the form has one.
    pub fn for_lane(&self, synth: bool) -> &Form {
        match &self.lead {
            Some(lead) if synth => lead,
            _ => self,
        }
    }

    /// The synth lanes' form, if the form writes one.
    pub fn lead(&self) -> Option<&Form> {
        self.lead.as_deref()
    }

    /// The segments one pass plays (the pattern repeated, with breaks).
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The segments as written.
    pub fn pattern(&self) -> &[Segment] {
        &self.pattern
    }

    /// Length of one pass in bars.
    pub fn bars(&self) -> u16 {
        self.beats / BEATS_PER_BAR
    }

    /// Where the start of global bar `bar` falls in the form's first pass.
    pub fn pos(&self, bar: u64) -> FormPos {
        self.pos_at(bar * BEATS_PER_BAR as u64)
    }

    /// Where global beat `beat` falls in the form's first pass.
    pub fn pos_at(&self, beat: u64) -> FormPos {
        let at = (beat % self.beats as u64) as u16;
        let mut start = 0u16;
        let mut main_start = 0u16;
        for (i, seg) in self.segments.iter().enumerate() {
            if seg.letter.class() == SeedClass::Main {
                main_start = start;
            }
            if at < start + seg.beats {
                let next = self.segments[(i + 1) % self.segments.len()];
                let prev = self.segments[(i + self.segments.len() - 1) % self.segments.len()];
                let in_segment = (at - start) / BEATS_PER_BAR;
                let end = start + seg.beats;
                return FormPos {
                    segment: i as u8,
                    source: seg.source,
                    letter: seg.letter,
                    play: seg.play,
                    prime: seg.prime,
                    swap: seg.swap,
                    ops: seg.ops,
                    beat_in_segment: at - start,
                    segment_beats: seg.beats,
                    start_beat: start,
                    period_end: end.is_multiple_of(PERIOD_BARS * BEATS_PER_BAR),
                    repeat: prev.letter == seg.letter && self.segments.len() > 1,
                    before_change: end.is_multiple_of(SECTION_BARS * BEATS_PER_BAR) || next.letter.class() != seg.letter.class(),
                    bar_in_segment: in_segment,
                    segment_bars: seg.bars(),
                    // Filler bars keep counting from the last main segment.
                    bars_since_main: if seg.letter.class() == SeedClass::Main {
                        in_segment
                    } else {
                        (at - main_start) / BEATS_PER_BAR
                    },
                    // The bar that holds the segment's last beat.
                    last_of_segment: (start + seg.beats - 1) / BEATS_PER_BAR == at / BEATS_PER_BAR,
                    next_letter: next.letter,
                    second: false,
                };
            }
            start += seg.beats;
        }
        // Unreachable for a parsed form (at < beats).
        FormPos::default()
    }

    /// Where global bar `bar` falls in the lane's form cycle: the form as
    /// written, then once more as a seeded variation of the same length
    /// (a song-like second half). The variation keeps the segments and the
    /// letter seeds and swaps parts of segments to the other letter of the
    /// same class: `A3 C1` may come back as `A1 B1 A1 D1`. A one-bar form
    /// (a style without forms) never varies.
    pub fn pos_varied(&self, bar: u64, seed: u32, lane: u8) -> FormPos {
        self.pos_varied_at(bar * BEATS_PER_BAR as u64, seed, lane)
    }

    /// [`Self::pos_varied`] at global beat `beat`.
    pub fn pos_varied_at(&self, beat: u64, seed: u32, lane: u8) -> FormPos {
        let len = self.beats as u64;
        let mut pos = self.pos_at(beat);
        if len <= BEATS_PER_BAR as u64 || (beat % (2 * len)) < len {
            return pos;
        }
        pos.second = true;
        let forced = !(0..self.segments.len()).any(|i| vary_pattern(seed, lane, i, self.segments[i], false) != Vary::Keep);
        let i = pos.segment as usize;
        let pattern = vary_pattern(seed, lane, i, self.segments[i], forced && i == 0);
        if pattern.swaps(pos.bar_in_segment, pos.segment_bars) {
            pos.letter = pos.letter.partner();
        }
        pos.play = vary_play(seed, lane, i, pos.play);
        pos
    }

    /// The parts of global bar `bar`: one per segment it touches, in order,
    /// with their tick ranges. Returns the parts and how many there are.
    pub fn parts(&self, bar: u64, seed: u32, lane: u8) -> ([BarPart; MAX_BAR_PARTS], usize) {
        let mut parts = [BarPart { from: 0, to: TICKS_PER_BAR, pos: FormPos::default() }; MAX_BAR_PARTS];
        let mut n = 0usize;
        let beat_ticks = TICKS_PER_BAR / BEATS_PER_BAR;
        for b in 0..BEATS_PER_BAR {
            let pos = self.pos_varied_at(bar * BEATS_PER_BAR as u64 + b as u64, seed, lane);
            let from = b * beat_ticks;
            if n > 0 {
                let last = &mut parts[n - 1];
                if (last.pos.segment, last.pos.letter) == (pos.segment, pos.letter) {
                    last.to = from + beat_ticks;
                    continue;
                }
            }
            parts[n] = BarPart { from, to: from + beat_ticks, pos };
            n += 1;
        }
        parts[n - 1].to = TICKS_PER_BAR;
        (parts, n)
    }
}

/// Repeat `pattern` (`total` beats) to [`FORM_BARS`] and put the structure
/// breaks over its main bars. A pattern that already fills the pass keeps
/// its own breaks.
fn expand(pattern: &[Segment], total: u16) -> Vec<Segment> {
    let pass = (FORM_BARS * BEATS_PER_BAR) as usize;
    let mut beats: Vec<Segment> = Vec::with_capacity(pass);
    while beats.len() < pass {
        for seg in pattern {
            for _ in 0..seg.beats {
                beats.push(Segment { beats: 1, ..*seg });
            }
        }
    }
    beats.truncate(pass);
    let repeats = (total as usize) < pass;
    if repeats {
        for (from, to, letter, swap) in STRUCTURE_BREAKS {
            for b in &mut beats[(from * BEATS_PER_BAR) as usize..(to * BEATS_PER_BAR) as usize] {
                if b.letter.class() == SeedClass::Main && b.play == Play::All {
                    *b = Segment { letter, play: Play::All, prime: false, swap, ops: OpChain::default(), beats: 1, source: NO_SOURCE };
                }
            }
        }
    }
    // Merge runs of the same segment; a repeat of the pattern starts anew.
    let mut out: Vec<Segment> = Vec::new();
    let mut run_start = 0usize;
    for (i, b) in beats.iter().enumerate() {
        let restart = repeats && i % total as usize == 0 && i > 0;
        match out.last_mut() {
            Some(last)
                if !restart
                    && (last.letter, last.play, last.prime, last.swap, last.ops, last.source) == (b.letter, b.play, b.prime, b.swap, b.ops, b.source)
                    && pattern_beats(pattern, last.source) > (i - run_start) as u16 =>
            {
                last.beats += 1;
            }
            _ => {
                out.push(*b);
                run_start = i;
            }
        }
    }
    out
}

/// Beats of written segment `source` (inserted breaks: no limit).
fn pattern_beats(pattern: &[Segment], source: u8) -> u16 {
    pattern.get(source as usize).map_or(u16::MAX, |s| s.beats)
}

/// How the second pass changes one segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vary {
    Keep,
    /// The whole segment takes the partner letter.
    Swap,
    /// Every second bar.
    Alternate,
    /// The last quarter of the segment (at least one bar).
    Last,
    /// The second half.
    SecondHalf,
}

impl Vary {
    fn swaps(self, bar: u16, bars: u16) -> bool {
        match self {
            Vary::Keep => false,
            Vary::Swap => true,
            Vary::Alternate => bar % 2 == 1,
            Vary::Last => bar >= bars - (bars / 4).max(1),
            Vary::SecondHalf => bar >= bars / 2,
        }
    }
}

// MUSICAL: first draft, tune by ear (user: song-like second half).
/// Chance a one-bar (or shorter) main segment swaps letters in the second
/// pass, and a filler segment.
const VARY_MAIN_SWAP: f32 = 0.35;
const VARY_FILLER_SWAP: f32 = 0.5;
/// Weights of keep, alternate, last and second half for longer main segments.
const VARY_MAIN: [(Vary, f32); 4] = [(Vary::Keep, 1.0), (Vary::Alternate, 1.5), (Vary::Last, 1.0), (Vary::SecondHalf, 1.0)];

/// The second-pass change of segment `i`. `force`: never keep.
fn vary_pattern(seed: u32, lane: u8, i: usize, seg: Segment, force: bool) -> Vary {
    let h = rng::hash(seed, lane, i as u8, 0, S_VARY);
    if seg.letter.class() == SeedClass::Filler {
        return if force || rng::chance(h, VARY_FILLER_SWAP) { Vary::Swap } else { Vary::Keep };
    }
    if seg.bars() == 1 {
        return if force || rng::chance(h, VARY_MAIN_SWAP) { Vary::Swap } else { Vary::Keep };
    }
    let mut weights = [0.0f32; VARY_MAIN.len()];
    for (w, &(v, weight)) in weights.iter_mut().zip(&VARY_MAIN) {
        // Halves need four bars; a forced change never keeps.
        *w = if (v == Vary::SecondHalf && seg.bars() < 4) || (force && v == Vary::Keep) { 0.0 } else { weight };
    }
    VARY_MAIN[rng::weighted_pick(rng::derive(h, 1), &weights).unwrap_or(1)].0
}

// MUSICAL: first draft (user: the riff often does not play at all in the
// first or the second 64 bars, or only in breaks).
/// Second pass: chance a rest becomes break-only, and break-only becomes
/// fully played.
const VARY_REST_TO_BREAKS: f32 = 0.4;
const VARY_BREAKS_TO_ALL: f32 = 0.4;

/// The second-pass play of segment `i`: some rests open into break-only
/// answers, some break-only segments into the full riff, so the two halves
/// differ in where the synth plays.
fn vary_play(seed: u32, lane: u8, i: usize, play: Play) -> Play {
    let h = rng::hash(seed, lane, i as u8, 1, S_VARY);
    match play {
        Play::Rest if rng::chance(h, VARY_REST_TO_BREAKS) => Play::Breaks,
        Play::Breaks if rng::chance(h, VARY_BREAKS_TO_ALL) => Play::All,
        p => p,
    }
}

/// A length in beats as bars in quarters: `7`, `6.5`, `.25`.
fn fmt_beats(beats: u16) -> String {
    let (whole, frac) = (beats / BEATS_PER_BAR, beats % BEATS_PER_BAR);
    let frac = ["", ".25", ".5", ".75"][frac as usize];
    if whole == 0 { frac.to_string() } else { format!("{whole}{frac}") }
}

impl fmt::Display for Form {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.pattern.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            let (prime, swap, ops) = (if s.prime { "'" } else { "" }, if s.swap { "*" } else { "" }, s.ops);
            match s.play {
                Play::Rest => write!(f, "-{}", fmt_beats(s.beats))?,
                Play::Breaks => write!(f, "~{}{prime}{swap}{ops}{}", s.letter.as_char(), fmt_beats(s.beats))?,
                Play::All => write!(f, "{}{prime}{swap}{ops}{}", s.letter.as_char(), fmt_beats(s.beats))?,
            }
        }
        if let Some(lead) = &self.lead {
            write!(f, " | {lead}")?;
        }
        Ok(())
    }
}

/// A bar's position inside a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormPos {
    pub segment: u8,
    /// Written segment it comes from ([`NO_SOURCE`] for a structure break).
    pub source: u8,
    pub letter: Letter,
    /// Whether the lane plays here (rests and break-only segments).
    pub play: Play,
    pub prime: bool,
    /// The lane plays another of its sounds (`*`).
    pub swap: bool,
    /// The segment's operators (D47).
    pub ops: OpChain,
    /// Beats from the segment's start to here, and the segment's length.
    pub beat_in_segment: u16,
    pub segment_beats: u16,
    /// The segment's first beat in the pass.
    pub start_beat: u16,
    /// Where the segment sits, for the seeded operator (`?`): it ends a
    /// 4-bar period, repeats the letter before it, or ends before a change
    /// (a 16-bar seam or another class of letter).
    pub period_end: bool,
    pub repeat: bool,
    pub before_change: bool,
    pub bar_in_segment: u16,
    pub segment_bars: u16,
    /// Bars since the last main segment (A/B) started. The chord
    /// progression counts from there.
    pub bars_since_main: u16,
    pub last_of_segment: bool,
    /// Letter of the segment after this one (wrapping).
    pub next_letter: Letter,
    /// In the second pass of the form (its variation, see [`Form::pos_varied`]).
    pub second: bool,
}

impl Default for FormPos {
    fn default() -> Self {
        Self {
            segment: 0,
            source: 0,
            letter: Letter::A,
            play: Play::All,
            prime: false,
            swap: false,
            ops: OpChain::default(),
            beat_in_segment: 0,
            segment_beats: BEATS_PER_BAR,
            start_beat: 0,
            period_end: false,
            repeat: false,
            before_change: false,
            bar_in_segment: 0,
            segment_bars: 1,
            bars_since_main: 0,
            last_of_segment: true,
            next_letter: Letter::A,
            second: false,
        }
    }
}

// RNG streams (form level).
const S_LETTER: u32 = 0xF00;
const S_PRIME: u32 = 0xF01;
const S_FEEL_SWING: u32 = 0xF10;
const S_FEEL_SWING_AMT: u32 = 0xF11;
const S_FEEL_TRIPLET: u32 = 0xF12;
const S_FEEL_DRIFT: u32 = 0xF13;
const S_DRIFT_STEP: u32 = 0xF14;
const S_CHORD_BPC: u32 = 0xF20;
const S_CHORD_TURN: u32 = 0xF21;
const S_CHORD_PASS: u32 = 0xF22;
const S_CHORD_PASS_DIR: u32 = 0xF23;
const S_LETTER_PARAM: u32 = 0xF30;
const S_LETTER_PARAM_SIGN: u32 = 0xF31;
const S_LETTER_PARAM_SECOND: u32 = 0xF32;
const S_VARY: u32 = 0xF40;

/// Seed of `letter` for a lane's master seed. A keeps the master seed itself,
/// so a style's default seeds sound like the old single-seed lanes.
pub fn letter_seed(master: u32, lane: u8, letter: Letter) -> u32 {
    match letter {
        Letter::A => master,
        _ => rng::hash(master, lane, letter.index(), 0, S_LETTER) as u32,
    }
}

/// Seed of the second half of a primed letter.
pub fn prime_seed(seed: u32, lane: u8) -> u32 {
    rng::hash(seed, lane, 0, 0, S_PRIME) as u32
}

/// Timing feel a letter seed carries (Q23). Main seeds: mostly straight,
/// sometimes light swing, a few ms of drift. Filler seeds: triplets, heavy
/// swing, larger push and pull.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Feel {
    /// Swing of the off 16ths, 0..1 of [`MAX_SWING_TICKS`].
    pub swing: f32,
    /// Reflow the bar onto a triplet grid.
    pub triplet: bool,
    /// Largest timing drift of an event in ticks (seeded per step).
    pub drift: u16,
}

// MUSICAL: first draft, tune by ear.
/// Largest swing delay of an off 16th (a third of a step).
pub const MAX_SWING_TICKS: u16 = TICKS_PER_STEP / 3;
const MAIN_SWING_PROB: f32 = 0.5;
const MAIN_SWING: (f32, f32) = (0.1, 0.55);
const MAIN_DRIFT_PROB: f32 = 0.5;
const MAIN_DRIFT_TICKS: u16 = 1;
const FILLER_TRIPLET_PROB: f32 = 0.4;
const FILLER_SWING_PROB: f32 = 0.6;
const FILLER_SWING: (f32, f32) = (0.45, 1.0);
const FILLER_DRIFT_TICKS: u16 = 3;

impl Feel {
    /// The feel a letter seed draws.
    pub fn of(seed: u32, lane: u8, class: SeedClass) -> Self {
        let h = |stream| rng::hash(seed, lane, 0, 0, stream);
        match class {
            SeedClass::Main => Self {
                swing: if rng::chance(h(S_FEEL_SWING), MAIN_SWING_PROB) {
                    rng::range_f32(h(S_FEEL_SWING_AMT), MAIN_SWING.0, MAIN_SWING.1)
                } else {
                    0.0
                },
                triplet: false,
                drift: if rng::chance(h(S_FEEL_DRIFT), MAIN_DRIFT_PROB) { MAIN_DRIFT_TICKS } else { 0 },
            },
            SeedClass::Filler => {
                let triplet = rng::chance(h(S_FEEL_TRIPLET), FILLER_TRIPLET_PROB);
                Self {
                    swing: if !triplet && rng::chance(h(S_FEEL_SWING), FILLER_SWING_PROB) {
                        rng::range_f32(h(S_FEEL_SWING_AMT), FILLER_SWING.0, FILLER_SWING.1)
                    } else {
                        0.0
                    },
                    triplet,
                    drift: FILLER_DRIFT_TICKS,
                }
            }
        }
    }

    /// Apply the feel to a rendered bar, keeping it sorted. The first event
    /// of every beat-one hit stays on the grid, so lanes keep locking.
    pub fn apply(&self, out: &mut BarEvents, seed: u32, lane: u8) {
        if self.swing <= 0.0 && !self.triplet && self.drift == 0 {
            return;
        }
        let beat = TICKS_PER_BAR / 4;
        let mut moved = BarEvents::default();
        let mut last: Option<(u16, u8, EventKind)> = None;
        for ev in out.as_slice() {
            let mut e: LiveEvent = *ev;
            let in_beat = e.tick % beat;
            let beat_start = e.tick - in_beat;
            if self.triplet {
                // 16ths of a beat onto 8th triplets: 0 → 0, 1 → ⅓, 2 and 3 → ⅔.
                let third = beat / 3;
                let slot = match in_beat * 4 / beat {
                    0 => 0,
                    1 => 1,
                    _ => 2,
                };
                e.tick = beat_start + slot * third + (in_beat % TICKS_PER_STEP) * 4 / 3;
                if e.gate > 0 {
                    e.gate = (e.gate as u32 * 4 / 3).min(u16::MAX as u32) as u16;
                }
            } else if self.swing > 0.0 && (e.tick / TICKS_PER_STEP) % 2 == 1 {
                e.tick += (self.swing * MAX_SWING_TICKS as f32).round() as u16;
            }
            if self.drift > 0 && e.tick % beat != 0 {
                let h = rng::hash(seed, lane, 0, (e.tick / TICKS_PER_STEP) as u8, S_DRIFT_STEP);
                let d = rng::range_i32(h, -(self.drift as i32), self.drift as i32);
                e.tick = (e.tick as i32 + d).max(0) as u16;
            }
            e.tick = e.tick.min(TICKS_PER_BAR - 1);
            e.gate = e.gate.min(TICKS_PER_BAR - e.tick);
            // Two onsets of one target merged onto the same triplet: keep one.
            if matches!(e.kind, EventKind::Trigger | EventKind::NoteOn) && last == Some((e.tick, e.target, e.kind)) {
                continue;
            }
            last = Some((e.tick, e.target, e.kind));
            moved.push(e);
        }
        moved.sort();
        *out = moved;
    }
}

/// Splice: keep `out` before the half bar, take `second` from it on.
pub fn splice_second_half(out: &mut BarEvents, second: &BarEvents) {
    splice_from(out, second, TICKS_PER_BAR / 2);
}

/// `out` up to tick `from`, then `second` (the primed letter, D47).
pub fn splice_from(out: &mut BarEvents, second: &BarEvents, from: u16) {
    let mut spliced = BarEvents::default();
    for e in out.as_slice().iter().filter(|e| e.tick < from) {
        spliced.push(*e);
    }
    for e in second.as_slice().iter().filter(|e| e.tick >= from) {
        spliced.push(*e);
    }
    *out = spliced;
}

// MUSICAL: tune by ear (user, 2026-10-21: `'` is one of the operators, so
// Variation scales it too). Where a primed bar takes the other seed: the
// last beat at Variation 0, the second half in full.
const PRIME_FROM_BEATS: (f32, f32) = (3.0, 2.0);

/// The tick a primed bar switches at, for operator amount `amount`.
pub fn prime_from(amount: f32) -> u16 {
    let beats = PRIME_FROM_BEATS.0 + (PRIME_FROM_BEATS.1 - PRIME_FROM_BEATS.0) * amount.clamp(0.0, 1.0);
    (beats.round() as u16) * (TICKS_PER_BAR / BEATS_PER_BAR)
}

// MUSICAL: first draft, tune by ear (Q32).
/// Chords in a lane's seeded progression.
const PROGRESSION_CHORDS: u8 = 4;
/// Share of filler bars that play a passing chord.
const PASSING_CHORD_PROB: f32 = 0.35;
/// Turnaround degrees: IV and V.
const TURNAROUND_DEGREES: [u8; 2] = [3, 4];
/// Degrees per octave of a diatonic scale.
const SCALE_DEGREES: u8 = 7;

/// The chord degree a pitched lane plays in a bar (Q32).
///
/// The master seed draws a 4-chord progression, 1 or 2 bars per chord,
/// starting near the tonic. It restarts at every main segment (A/B) and keeps
/// running through filler segments, which may add a passing chord. The bar
/// before the form returns to A plays a turnaround (IV or V) so A lands on
/// the tonic again.
pub fn form_chord(master: u32, lane: u8, pos: &FormPos, letter_seed: u32, minor: bool) -> u8 {
    let dim = if minor { 1 } else { 6 };
    let turnaround = pos.last_of_segment && pos.next_letter == Letter::A && pos.letter != Letter::A;
    if turnaround {
        let h = rng::hash(master, lane, 0, 0, S_CHORD_TURN);
        return TURNAROUND_DEGREES[rng::below(h, TURNAROUND_DEGREES.len() as u32) as usize];
    }
    let bars_per_chord = 1 + rng::below(rng::hash(master, lane, 0, 0, S_CHORD_BPC), 2) as u16;
    let index = (pos.bars_since_main / bars_per_chord) as u8 % PROGRESSION_CHORDS;
    let degree = seeded_chord(master, lane, index, minor);
    if pos.letter.class() == SeedClass::Filler {
        let step = pos.bar_in_segment as u8;
        if rng::chance(rng::hash(letter_seed, lane, 0, step, S_CHORD_PASS), PASSING_CHORD_PROB) {
            let up = rng::chance(rng::hash(letter_seed, lane, 0, step, S_CHORD_PASS_DIR), 0.5);
            let mut passing = if up { (degree + 1) % SCALE_DEGREES } else { (degree + SCALE_DEGREES - 1) % SCALE_DEGREES };
            if passing == dim {
                passing = if up { (passing + 1) % SCALE_DEGREES } else { (passing + SCALE_DEGREES - 1) % SCALE_DEGREES };
            }
            return passing;
        }
    }
    degree
}

// MUSICAL: first draft, tune by ear. Generator parameters a filler bar
// pushes up (by name, added to the control value, clamped to 0..1).
pub const FILLER_BOOSTS: [(&str, f32); 7] = [
    ("density", 0.15),
    ("mutate", 0.45),
    ("chop", 0.35),
    ("variation", 0.5),
    ("complexity", 0.3),
    ("spread", 0.25),
    ("accent", 0.2),
];

// MUSICAL: first draft, tune by ear. Generator parameters a letter other
// than A moves away from the control value (by name, largest offset). A
// plays the controls as set; B, C and D sit around them, so each block
// sounds like a different setting, and the controls still move all of them
// (user, 2026-10-20: the right-hand knobs, Permute and Groove most). New
// entries go at the end: an entry's index seeds its offset.
pub const LETTER_OFFSETS: [(&str, f32); 13] = [
    ("density", 0.25),
    ("complexity", 0.6),
    ("variation", 0.35),
    ("slide", 0.55),
    ("spread", 0.55),
    ("accent", 0.3),
    ("swing", 0.45),
    ("chop", 0.45),
    ("mutate", 0.3),
    // Bass Shape and Groove.
    ("length", 0.35),
    ("syncopation", 0.45),
    // Not generator parameters: the lane moves them per block (knob values).
    ("morph", 0.25),
    ("permute", 0.4),
];
/// Share of the largest letter offset each note reads its parameters away
/// by (user: notes within a block differ too).
pub const NOTE_JITTER_SHARE: f32 = 0.5;

/// Smallest share of the largest offset a letter moves a parameter, so a
/// block never lands next to the control value by chance.
const LETTER_OFFSET_MIN: f32 = 0.4;

/// Share of the largest offset the second pass of a form moves a letter on
/// top (A too): same seeds, slightly different settings.
const SECOND_PASS_SHARE: f32 = 0.35;

/// Seeded offset of parameter `param` (index into [`LETTER_OFFSETS`]) for a
/// letter seed. Zero for A in the first pass. `second`: the form's second
/// pass adds a smaller seeded move.
pub fn letter_offset(seed: u32, lane: u8, letter: Letter, param: usize, second: bool) -> f32 {
    let max = LETTER_OFFSETS[param].1;
    let p = param as u8;
    let mut offset = 0.0;
    if letter != Letter::A {
        let size = rng::range_f32(rng::hash(seed, lane, p, 0, S_LETTER_PARAM), LETTER_OFFSET_MIN * max, max);
        offset = if rng::chance(rng::hash(seed, lane, p, 0, S_LETTER_PARAM_SIGN), 0.5) { size } else { -size };
    }
    if second {
        let u = rng::unit_f32(rng::hash(seed, lane, p, 0, S_LETTER_PARAM_SECOND));
        offset += SECOND_PASS_SHARE * max * (2.0 * u - 1.0);
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::blocks::trigger;

    #[test]
    fn synth_forms_rest_and_play_only_in_breaks() {
        let f = Form::parse("A15 C1 A16 B16 A15 D1 | -16 ~A16 B8 -8 A'16").unwrap();
        assert_eq!(f.to_string(), "A15 C1 A16 B16 A15 D1 | -16 ~A16 B8 -8 A'16");
        let lead = f.for_lane(true);
        assert_eq!(lead.bars(), FORM_BARS);
        assert_eq!(f.for_lane(false).pattern().len(), 6, "drums and bass play the main form");
        assert_eq!(lead.pos(0).play, Play::Rest);
        assert_eq!((lead.pos(20).play, lead.pos(20).letter), (Play::Breaks, Letter::A));
        assert_eq!((lead.pos(33).play, lead.pos(33).letter), (Play::All, Letter::B));
        // A pattern with rests keeps them: no structure break is put over a rest.
        let short = Form::parse("-8 A8").unwrap();
        assert!(short.segments().iter().all(|s| s.play == Play::Rest || s.letter == Letter::A || s.letter.class() == SeedClass::Filler));
        assert_eq!(short.pos(15).play, Play::All);
        assert_eq!(short.pos(16).play, Play::Rest);
        assert!(Form::parse("~8").is_err(), "break-only needs a letter");
    }

    #[test]
    fn second_pass_opens_some_rests() {
        let f = Form::parse("-16 A16 -16 A16").unwrap();
        let mut opened = 0;
        for seed in 0..64u32 {
            let p = f.pos_varied(FORM_BARS as u64, seed, 2);
            assert!(p.second);
            if p.play != Play::Rest {
                opened += 1;
            }
        }
        assert!(opened > 5 && opened < 59, "some rests open, most stay: {opened}");
    }

    #[test]
    fn parses_and_prints_forms() {
        let f = Form::parse("A7 B1 A'7 C1").unwrap();
        assert_eq!(f.bars(), FORM_BARS, "every form fills 64 bars (128 with its second pass)");
        assert_eq!(f.to_string(), "A7 B1 A'7 C1");
        assert_eq!(Form::parse("a b2").unwrap().to_string(), "A1 B2");
        assert!(matches!(Form::parse(""), Err(FormError::Empty)));
        assert!(matches!(Form::parse("E4"), Err(FormError::Segment(_))));
        assert!(matches!(Form::parse("A0"), Err(FormError::Segment(_))));
        assert!(matches!(Form::parse("A60 C5"), Err(FormError::TooLong(65))));
        for f in DEFAULT_FORMS {
            assert_eq!(Form::parse(f).unwrap().to_string(), f, "built-in forms round-trip");
        }
    }

    #[test]
    fn operators_chain_in_any_order_with_the_length() {
        use crate::ops::Op;
        let f = Form::parse("A3 A2r A2'r Ar2 A3rd B.5q Ba.5 A?1").unwrap();
        let ops: Vec<String> = f.pattern().iter().map(|s| s.ops.to_string()).collect();
        assert_eq!(ops, ["", "r", "r", "r", "rd", "q", "a", "?"]);
        assert_eq!(f.pattern()[1].beats, 8, "A2r is two bars");
        assert!(f.pattern()[2].prime && f.pattern()[2].ops.contains(Op::Retrograde), "A2'r is primed and reversed");
        assert_eq!(f.pattern()[4].ops.get(0), Some(Op::Retrograde), "marks apply left to right");
        assert_eq!(f.pattern()[5].beats, 2);
        // Printed canonically (marks, then the length) and back.
        assert_eq!(f.to_string(), "A3 Ar2 A'r2 Ar2 Ard3 Bq.5 Ba.5 A?1");
        assert_eq!(Form::parse(&f.to_string()).unwrap(), f);
        // A position carries its segment's operators and where it sits.
        let p = f.pos(4);
        assert_eq!((p.ops.to_string(), p.beat_in_segment, p.segment_beats), ("r".to_string(), 4, 8));
        for bad in ["A2r3", "Az2", "-2r", "Arrrrrrr1", "A1.5.5", "Ar"] {
            let ok = Form::parse(bad).is_ok();
            assert_eq!(ok, bad == "Ar", "{bad}");
        }
    }

    #[test]
    fn beat_resolution_parts() {
        let f = Form::parse("A6.5 C*.5 B.25 D*.75").unwrap();
        assert_eq!(f.to_string(), "A6.5 C*.5 B.25 D*.75");
        assert!(matches!(Form::parse("A.3"), Err(FormError::Segment(_))));
        // Bar 6: A for two beats, then the swapped C.
        let (parts, n) = f.parts(6, 1, 1);
        assert_eq!(n, 2);
        assert_eq!((parts[0].from, parts[0].to, parts[0].pos.letter), (0, 192, Letter::A));
        assert_eq!((parts[1].from, parts[1].to, parts[1].pos.letter, parts[1].pos.swap), (192, 384, Letter::C, true));
        // Bar 7: one beat of B, three of swapped D.
        let (parts, n) = f.parts(7, 1, 1);
        assert_eq!(n, 2);
        assert_eq!((parts[0].to, parts[1].pos.letter, parts[1].pos.swap), (96, Letter::D, true));
        assert!(parts[0].pos.last_of_segment && parts[1].pos.last_of_segment);
        // Whole bars are one part.
        assert_eq!(f.parts(0, 1, 1).1, 1);
        // Every built-in form is a written 64 bars.
        for text in DEFAULT_FORMS {
            let form = Form::parse(text).unwrap();
            assert_eq!(form.pattern().iter().map(|s| s.beats as u32).sum::<u32>(), 256, "{text}");
        }
    }

    #[test]
    fn forms_repeat_to_64_bars_with_structure_breaks() {
        let f = Form::parse("A3 B1").unwrap();
        let letters: Vec<(Letter, bool)> = (0..64).map(|b| { let p = f.pos(b); (p.letter, p.swap) }).collect();
        assert_eq!(f.segments().iter().map(|s| s.beats as u32).sum::<u32>(), 256);
        assert_eq!(letters[0..4], [(Letter::A, false), (Letter::A, false), (Letter::A, false), (Letter::B, false)]);
        assert_eq!(letters[15].0, Letter::C, "a break at the end of 16");
        assert_eq!((letters[30].0, letters[31].0), (Letter::D, Letter::D), "a longer one at 32");
        assert_eq!(letters[47].0, Letter::C);
        assert_eq!(letters[60..64], [(Letter::C, false), (Letter::C, false), (Letter::D, true), (Letter::D, true)], "the breakout at 64 swaps the sound");
        assert_eq!(f.pos(15).source, NO_SOURCE);
        assert_eq!(f.pos(4).source, 0, "the pattern repeats from its first segment");
        // Swaps parse and print.
        let s = Form::parse("A1 A*1 A'1 A'*1").unwrap();
        assert_eq!(s.to_string(), "A1 A*1 A'1 A'*1");
        assert!(s.pos(1).swap && s.pos(3).swap && s.pos(3).prime && !s.pos(2).swap);
        // A written 64-bar form keeps its own breaks.
        let full = Form::parse("A15 C1 A'15 C1 B15 D1 A14 C*2").unwrap();
        assert_eq!((full.pos(30).letter, full.pos(31).letter), (Letter::A, Letter::C), "no D break inserted");
        assert!(full.pos(63).swap);
    }

    #[test]
    fn positions_follow_the_segments() {
        let f = Form::parse("A3 B1 A'3 C1").unwrap();
        let letters: String = (0..9).map(|b| f.pos(b).letter.as_char()).collect();
        assert_eq!(letters, "AAABAAACA");
        let p = f.pos(4);
        assert!(p.prime && p.bar_in_segment == 0 && p.segment == 2);
        let c = f.pos(7);
        assert!(c.last_of_segment && c.next_letter == Letter::A);
        assert_eq!(c.bars_since_main, 3, "filler counts from the last main start");
        assert_eq!(f.pos(3).bars_since_main, 0, "B is main and restarts");
    }

    #[test]
    fn second_pass_varies_the_form() {
        let form = Form::parse("A3 C1 A'3 D1").unwrap();
        let mut changed = 0;
        for seed in 0..50u32 {
            let mut differs = false;
            for bar in 0..64u64 {
                let first = form.pos_varied(bar, seed, 2);
                assert!(!first.second);
                assert_eq!(first, form.pos(bar), "the first pass is the form as written");
                let second = form.pos_varied(bar + 64, seed, 2);
                assert!(second.second);
                assert_eq!(second.letter.class(), first.letter.class(), "letters keep their class");
                assert_eq!((second.segment, second.prime), (first.segment, first.prime));
                differs |= second.letter != first.letter;
            }
            assert!(differs, "seed {seed}: the second pass always changes something");
            changed += differs as u32;
            // The cycle is twice the form.
            assert_eq!(form.pos_varied(128, seed, 2), form.pos(0));
        }
        assert_eq!(changed, 50);
        // One-bar forms (no forms) never vary.
        assert!((0..4).all(|b| Form::single().pos_varied(b, 3, 1).letter == Letter::A));
        // Letter offsets: the second pass moves A too, slightly.
        let k = 0;
        let o = letter_offset(7, 1, Letter::A, k, true);
        assert!(o.abs() <= SECOND_PASS_SHARE * LETTER_OFFSETS[k].1 + 1e-6);
    }

    #[test]
    fn letters_move_parameters_around_the_controls() {
        for k in 0..LETTER_OFFSETS.len() {
            assert_eq!(letter_offset(7, 1, Letter::A, k, false), 0.0, "A plays the controls");
        }
        let mut signs = [0usize; 2];
        for letter in [Letter::B, Letter::C, Letter::D] {
            for k in 0..LETTER_OFFSETS.len() {
                let max = LETTER_OFFSETS[k].1;
                let o = letter_offset(letter_seed(7, 1, letter), 1, letter, k, false);
                assert!(o.abs() >= LETTER_OFFSET_MIN * max - 1e-6 && o.abs() <= max + 1e-6, "{o}");
                signs[(o > 0.0) as usize] += 1;
            }
        }
        assert!(signs[0] > 3 && signs[1] > 3, "offsets go both ways: {signs:?}");
    }

    #[test]
    fn letter_seeds_and_classes() {
        assert_eq!(letter_seed(50, 1, Letter::A), 50);
        let b = letter_seed(50, 1, Letter::B);
        assert_ne!(b, 50);
        assert_eq!(b, letter_seed(50, 1, Letter::B));
        assert_ne!(b, letter_seed(50, 2, Letter::B));
        assert_eq!(Letter::C.class(), SeedClass::Filler);
        assert_eq!(Letter::B.class(), SeedClass::Main);
    }

    #[test]
    fn filler_feels_are_looser_than_main() {
        let mut triplets = 0;
        for seed in 0..400 {
            let m = Feel::of(seed, 0, SeedClass::Main);
            assert!(!m.triplet && m.drift <= MAIN_DRIFT_TICKS && m.swing <= MAIN_SWING.1);
            let f = Feel::of(seed, 0, SeedClass::Filler);
            triplets += f.triplet as u32;
            assert_eq!(f.drift, FILLER_DRIFT_TICKS);
        }
        assert!((100..220).contains(&triplets), "{triplets}");
    }

    #[test]
    fn triplet_feel_reflows_and_merges() {
        let mut bar = BarEvents::default();
        for s in 0..4u16 {
            bar.push(trigger(s * TICKS_PER_STEP, 8, 100));
        }
        Feel { swing: 0.0, triplet: true, drift: 0 }.apply(&mut bar, 1, 0);
        let ticks: Vec<u16> = bar.as_slice().iter().map(|e| e.tick).collect();
        assert_eq!(ticks, [0, 32, 64], "steps 3 and 4 share the last triplet");
    }

    #[test]
    fn swing_and_drift_keep_beats_and_order() {
        let mut bar = BarEvents::default();
        for s in 0..16u16 {
            bar.push(trigger(s * TICKS_PER_STEP, 8, 100));
        }
        Feel { swing: 1.0, triplet: false, drift: 3 }.apply(&mut bar, 7, 0);
        let ev = bar.as_slice();
        assert_eq!(ev.len(), 16);
        assert!(ev.windows(2).all(|w| w[0].tick <= w[1].tick));
        for b in 0..4 {
            assert!(ev.iter().any(|e| e.tick == b * 96), "beat {b} stays on the grid");
        }
        assert!(ev.iter().any(|e| e.tick % TICKS_PER_STEP != 0));
    }

    #[test]
    fn chords_restart_at_main_segments_and_turn_around() {
        let f = Form::parse("A3 B1 A3 C1").unwrap();
        for master in 0..50 {
            let at = |bar: u64| {
                let p = f.pos(bar);
                form_chord(master, 1, &p, letter_seed(master, 1, p.letter), true)
            };
            assert_eq!(at(0), at(4), "A restarts the progression");
            assert!(TURNAROUND_DEGREES.contains(&at(3)), "B before A turns around");
            assert!(TURNAROUND_DEGREES.contains(&at(7)), "C before A turns around");
            for bar in 0..8 {
                assert!(at(bar) < 7 && at(bar) != 1, "diatonic, no diminished");
            }
        }
    }

    #[test]
    fn splice_takes_the_second_half_from_the_other_bar() {
        let mut a = BarEvents::default();
        let mut b = BarEvents::default();
        for t in [0u16, 96, 192, 288] {
            a.push(trigger(t, 1, 100));
            b.push(trigger(t, 2, 100));
        }
        splice_second_half(&mut a, &b);
        let targets: Vec<u8> = a.as_slice().iter().map(|e| e.target).collect();
        assert_eq!(targets, [1, 1, 2, 2]);
    }
}
