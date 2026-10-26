//! Pattern slots (design D60): the eight patterns a lane offers the DAW,
//! two per letter (A1, A2 .. D1, D2), what a DAW note asks for, and the
//! patterns the user edited in the plugin.
//!
//! A DAW note picks a slot by its key ([`BASE_KEY`] up) and an operator by
//! its velocity ([`VelOp`]); it plays the slot's pattern from its start for
//! as long as it is held. A slot plays its seeded pattern until the user
//! edits it: then it plays the [`UserPattern`], a whole source pattern and
//! the window of it that loops. Everything after the skeleton (speed,
//! recursion, Density, Length, Groove, operators, expression) still applies.

use serde::{Deserialize, Serialize};

use crate::form::Letter;
use crate::ops::Op;
use crate::TICKS_PER_BAR;

/// Slots: two patterns per letter.
pub const SLOTS: usize = 8;
/// Patterns per letter.
pub const VARIANTS: u8 = 2;
/// The DAW key of slot A1 (C1 in most DAWs); B1 is two keys up, D2 seven.
pub const BASE_KEY: u8 = 36;

/// One of a lane's patterns: a letter and which of its two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Slot {
    pub letter: Letter,
    pub variant: u8,
}

impl Slot {
    pub const ALL: [Slot; SLOTS] = {
        let mut all = [Slot { letter: Letter::A, variant: 0 }; SLOTS];
        let mut i = 0;
        while i < SLOTS {
            all[i] = Slot::from_index(i);
            i += 1;
        }
        all
    };

    /// Slot `i` (0 = A1, 1 = A2, 2 = B1 .. 7 = D2), wrapping.
    pub const fn from_index(i: usize) -> Self {
        let i = i % SLOTS;
        let letter = match i / VARIANTS as usize {
            0 => Letter::A,
            1 => Letter::B,
            2 => Letter::C,
            _ => Letter::D,
        };
        Slot { letter, variant: (i % VARIANTS as usize) as u8 }
    }

    pub fn index(self) -> usize {
        self.letter.index() as usize * VARIANTS as usize + (self.variant % VARIANTS) as usize
    }

    /// "A1" .. "D2".
    pub fn name(self) -> &'static str {
        ["A1", "A2", "B1", "B2", "C1", "C2", "D1", "D2"][self.index()]
    }

    /// The DAW key that plays the slot.
    pub fn key(self) -> u8 {
        BASE_KEY + self.index() as u8
    }

    /// The slot DAW key `key` plays, if any.
    pub fn of_key(key: u8) -> Option<Self> {
        key.checked_sub(BASE_KEY).filter(|i| (*i as usize) < SLOTS).map(|i| Slot::from_index(i as usize))
    }
}

/// What a DAW note's velocity asks of its pattern (D60).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelOp {
    /// The pattern as it is.
    Plain,
    /// `′`: the other second half.
    Prime,
    Op(Op),
}

// MUSICAL: tune by ear (user, 2026-10-24: velocity picks the operator; keep
// the ones that sound good). The lowest velocity of each band, low to high;
// everything from the last band's bottom up plays the pattern as it is,
// so a note the DAW draws at its default velocity (about 100) does.
pub const VELOCITY_BANDS: [(u8, VelOp); 10] = [
    (1, VelOp::Prime),
    (10, VelOp::Op(Op::Retrograde)),
    (20, VelOp::Op(Op::Inversion)),
    (30, VelOp::Op(Op::Reverse)),
    (40, VelOp::Op(Op::RetroInversion)),
    (50, VelOp::Op(Op::HalfTime)),
    (60, VelOp::Op(Op::DoubleTime)),
    (70, VelOp::Op(Op::Question)),
    (80, VelOp::Op(Op::Answer)),
    (91, VelOp::Plain),
];

impl VelOp {
    /// What velocity `vel` (1..=127) asks for.
    pub fn of_velocity(vel: u8) -> Self {
        VELOCITY_BANDS.iter().rev().find(|(lo, _)| vel >= *lo).map_or(VelOp::Prime, |(_, op)| *op)
    }

    /// A velocity that asks for `self`: its band's middle, or 100 for the
    /// plain pattern (`None`: no band has it).
    pub fn velocity(self) -> Option<u8> {
        if self == VelOp::Plain {
            return Some(100);
        }
        let i = VELOCITY_BANDS.iter().position(|(_, op)| *op == self)?;
        let hi = VELOCITY_BANDS.get(i + 1).map_or(128, |(lo, _)| *lo as u16);
        Some(((VELOCITY_BANDS[i].0 as u16 + hi - 1) / 2) as u8)
    }

    /// Short label: "", "′", "r" ...
    pub fn mark(self) -> String {
        match self {
            VelOp::Plain => String::new(),
            VelOp::Prime => "′".into(),
            VelOp::Op(op) => op.as_char().to_string(),
        }
    }
}

/// One note of a user pattern. `row`: the bass's semitones over the home
/// root (it moves with the key), the drums' sample target. `len` 0 is a
/// drum one-shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserNote {
    pub tick: u32,
    pub len: u32,
    pub row: i16,
    pub vel: u8,
    /// Drums: the hit's position in a loop and its choke group, as the
    /// source played them.
    #[serde(default)]
    pub start: u16,
    #[serde(default)]
    pub choke: u8,
}

/// A pattern the user edited: the whole source and the window of it that
/// plays, in ticks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPattern {
    /// Sorted by tick.
    pub notes: Vec<UserNote>,
    /// Length of the whole source.
    pub ticks: u32,
    /// The window: its start and length.
    pub start: u32,
    pub len: u32,
}

/// Shortest window: a beat.
pub const MIN_WINDOW: u32 = TICKS_PER_BAR as u32 / 4;
/// Longest source: 16 bars.
pub const MAX_SOURCE_TICKS: u32 = 16 * TICKS_PER_BAR as u32;

impl UserPattern {
    /// Keep the window inside the source and the notes sorted, inside it
    /// and at least a tick long.
    pub fn normalize(&mut self) {
        self.ticks = self.ticks.clamp(MIN_WINDOW, MAX_SOURCE_TICKS);
        self.len = self.len.clamp(MIN_WINDOW, self.ticks);
        self.start = self.start.min(self.ticks - self.len);
        let ticks = self.ticks;
        self.notes.retain(|n| n.tick < ticks);
        for n in &mut self.notes {
            n.len = n.len.min(ticks - n.tick);
            n.vel = n.vel.max(1);
        }
        self.notes.sort_by_key(|n| (n.tick, n.row));
        self.notes.dedup_by_key(|n| (n.tick, n.row));
    }

    /// The window's beats (rounded up).
    pub fn window_beats(&self) -> u32 {
        self.len.div_ceil(TICKS_PER_BAR as u32 / 4)
    }

    /// The notes of bar `bar` of the looping window: `f(tick in the bar,
    /// gate, note)`, the gate ending with the window.
    pub fn bar_notes(&self, bar: u32, mut f: impl FnMut(u16, u32, &UserNote)) {
        let (bar_ticks, len) = (TICKS_PER_BAR as u32, self.len.max(1));
        let lo = bar as u64 * bar_ticks as u64;
        let hi = lo + bar_ticks as u64;
        for n in self.notes.iter().filter(|n| n.tick >= self.start && n.tick < self.start + len) {
            let rel = (n.tick - self.start) as u64;
            // The laps of the window that fall in this bar.
            let first = if lo > rel { (lo - rel).div_ceil(len as u64) } else { 0 };
            let mut at = rel + first * len as u64;
            while at < hi {
                let gate = n.len.min(len - rel as u32);
                f((at - lo) as u16, gate, n);
                at += len as u64;
            }
        }
    }
}

/// The user's patterns, one per slot (`None`: the seeded one), and each
/// slot's reroll: 0 plays the seed's pattern, another value another one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternBook {
    pub slots: [Option<UserPattern>; SLOTS],
    #[serde(default)]
    pub rerolls: [u32; SLOTS],
}

impl PatternBook {
    pub fn get(&self, slot: Slot) -> Option<&UserPattern> {
        self.slots[slot.index()].as_ref()
    }

    /// The seed slot `slot` plays, from its letter's `seed`.
    pub fn seed_of(&self, slot: Slot, seed: u32, lane: u8) -> u32 {
        let seed = crate::form::variant_seed(seed, lane, slot.variant);
        match self.rerolls[slot.index()] {
            0 => seed,
            r => crate::rng::hash(seed, lane, slot.index() as u8, (r & 0xff) as u8, S_REROLL ^ (r >> 8)) as u32,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none) && self.rerolls.iter().all(|r| *r == 0)
    }
}

const S_REROLL: u32 = 0xF03;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_name_the_slots() {
        assert_eq!(Slot::of_key(36), Some(Slot { letter: Letter::A, variant: 0 }));
        assert_eq!(Slot::of_key(37), Some(Slot { letter: Letter::A, variant: 1 }));
        assert_eq!(Slot::of_key(43).map(Slot::name), Some("D2"));
        assert_eq!(Slot::of_key(35), None);
        assert_eq!(Slot::of_key(44), None);
        for s in Slot::ALL {
            assert_eq!(Slot::of_key(s.key()), Some(s));
            assert_eq!(Slot::from_index(s.index()), s);
        }
    }

    #[test]
    fn velocity_bands_round_trip_and_the_default_plays_plain() {
        assert_eq!(VelOp::of_velocity(100), VelOp::Plain);
        assert_eq!(VelOp::of_velocity(96), VelOp::Plain);
        assert_eq!(VelOp::of_velocity(127), VelOp::Plain);
        assert_eq!(VelOp::of_velocity(1), VelOp::Prime);
        assert_eq!(VelOp::of_velocity(15), VelOp::Op(Op::Retrograde));
        for (_, op) in VELOCITY_BANDS {
            assert_eq!(VelOp::of_velocity(op.velocity().unwrap()), op, "{op:?}");
        }
        assert_eq!(VelOp::Op(Op::Pedal).velocity(), None);
    }

    #[test]
    fn a_window_loops_over_the_bars() {
        let n = |tick, row| UserNote { tick, len: 24, row, vel: 100, start: 0, choke: 0 };
        // Two bars of source, a two-beat window from beat 4 (bar 2).
        let p = UserPattern { notes: vec![n(0, 0), n(384, 1), n(480, 2), n(576, 3)], ticks: 768, start: 384, len: 192 };
        let mut got = Vec::new();
        p.bar_notes(0, |t, _, note| got.push((t, note.row)));
        assert_eq!(got, vec![(0, 1), (192, 1), (96, 2), (288, 2)]);
        got.clear();
        p.bar_notes(1, |t, _, note| got.push((t, note.row)));
        assert_eq!(got, vec![(0, 1), (192, 1), (96, 2), (288, 2)]);
    }

    #[test]
    fn normalize_keeps_the_window_inside() {
        let mut p = UserPattern { notes: vec![UserNote { tick: 900, len: 500, row: 0, vel: 0, start: 0, choke: 0 }], ticks: 768, start: 700, len: 5 };
        p.normalize();
        assert!(p.notes.is_empty());
        assert_eq!((p.start, p.len), (672, MIN_WINDOW));
    }
}
