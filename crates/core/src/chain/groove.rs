//! R7 Groove (D57): onsets pushed, pulled and moved off the grid, lengths
//! nudged a little, on every note (the recursion's and the fills' too).
//! Groove sets how often and how far: from nothing at 0 through rare, mild
//! moves in the middle to frequent, far ones at the top. The seed and
//! Variation pick which notes and which way: at low Variation a letter's
//! moves all go its one way, at high each note draws its own. A note's
//! place weighs in: weak 16ths and the step before a beat move most, the
//! downbeat least. Every draw follows the letter seed and the note's
//! written step, so a letter grooves alike wherever it plays. The drums
//! keep their own groove ([`drums`]): each voice pushed or laid back.

use crate::event::{flags, origin, BarEvents, EventKind};
use crate::generator::layout::DrumVoice;
use crate::rng;
use crate::{TICKS_PER_BAR, TICKS_PER_STEP};

// MUSICAL: tune by ear (user, 2026-10-23: the middle conservative and not
// often, the top drastic and often). The share of notes moved at full
// Groove and its curve (about a fifth at the middle); the largest move at
// full Groove and at the middle's end of the curve, and the smallest, in
// ticks (moves land on a 64th grid, so off the 16ths); the share of moves
// ahead of the note (pushes); the length nudge at full Groove (share).
const MOVE_SHARE: f32 = 0.6;
const MOVE_CURVE: f32 = 1.6;
const MOVE_MAX_TICKS: f32 = 48.0;
const MOVE_MIN_TICKS: f32 = 6.0;
const MOVE_GRID: i32 = 6;
const PUSH_SHARE: f32 = 0.65;
const NUDGE: f32 = 0.1;
/// Weight of a note's place: the downbeat, a beat, an off 8th, a weak 16th,
/// the 16th before a beat.
const PLACE_WEIGHT: [f32; 5] = [0.25, 0.6, 0.9, 1.0, 1.2];
/// Least distance a moved note keeps to its neighbours (ticks).
const KEEP_APART: i32 = 6;
const S_GROOVE: u32 = 0xF80;

fn place(tick: u16) -> f32 {
    let step = tick / TICKS_PER_STEP;
    PLACE_WEIGHT[match (tick, step % 4) {
        (0, _) => 0,
        (_, 0) => 1,
        (_, 2) => 2,
        (_, 3) => 4,
        _ => 3,
    }]
}

/// Groove a pitched lane's bar at `amount` 0..1 (the Groove knob),
/// `variation` 0..1, for the letter `seed`.
pub(crate) fn pitched(out: &mut BarEvents, amount: f32, variation: f32, seed: u32, lane: u8) {
    let g = amount.clamp(0.0, 1.0);
    if g <= 0.0 {
        return;
    }
    let share = MOVE_SHARE * g.powf(MOVE_CURVE);
    let reach = MOVE_MIN_TICKS + (MOVE_MAX_TICKS - MOVE_MIN_TICKS) * g * g;
    let letter = |k: u32| rng::unit_f32(rng::hash(seed, lane, 0, 0, S_GROOVE + k));
    let ev = out.as_mut_slice();
    for i in 0..ev.len() {
        if ev[i].kind != EventKind::NoteOn {
            continue;
        }
        let e = ev[i];
        // The note's own draws: its written step, its origin and group.
        let key = (e.step, origin::of(e.origin) | e.group << 4);
        let own = |k: u32| rng::unit_f32(rng::hash(seed, lane, key.0, key.1, S_GROOVE + 16 + k));
        // Low Variation: the letter's way for every note; high: the note's.
        let pick = |k: u32| if own(8 + k) < variation { own(k) } else { letter(k) };
        let prev = ev[..i].iter().rev().find(|x| x.kind == EventKind::NoteOn).map(|x| x.tick as i32);
        let next = ev[i + 1..].iter().find(|x| x.kind == EventKind::NoteOn).map_or(TICKS_PER_BAR as i32, |x| x.tick as i32);
        if own(0) < (share * place(e.tick)).min(1.0) {
            let size = MOVE_MIN_TICKS + (reach - MOVE_MIN_TICKS) * pick(1) * pick(1);
            let size = ((size as i32 + MOVE_GRID / 2) / MOVE_GRID * MOVE_GRID).max(MOVE_GRID);
            let early = pick(2) < PUSH_SHARE;
            let lo = prev.map_or(0, |p| p + KEEP_APART);
            let hi = next - KEEP_APART;
            let mut to = e.tick as i32 + if early { -size } else { size };
            if to < lo || to > hi {
                // The other way, else as far as it fits.
                to = e.tick as i32 + if early { size } else { -size };
            }
            let to = to.clamp(lo, hi.max(lo));
            if (0..TICKS_PER_BAR as i32).contains(&to) && to != e.tick as i32 {
                let end = e.tick as i32 + e.gate as i32;
                let x = &mut ev[i];
                x.tick = to as u16;
                // It ends where it ended (a push holds into its place).
                x.gate = (end - to).max(MOVE_GRID) as u16;
            }
        }
        // The length nudged a little, never over the next note (a tie keeps
        // its overlap).
        let tied = ev[i + 1..].iter().find(|x| x.kind == EventKind::NoteOn).is_some_and(|x| x.has(flags::TIE));
        let x = &mut ev[i];
        if !tied && x.gate > 0 {
            let f = 1.0 + NUDGE * g * (2.0 * own(4) - 1.0);
            let room = (next - x.tick as i32).max(1);
            x.gate = ((x.gate as f32 * f).round() as i32).clamp(1, room) as u16;
        }
    }
    // A note before a moved one ends in time (but a tie).
    for i in 0..ev.len() {
        if ev[i].kind != EventKind::NoteOn {
            continue;
        }
        let Some(n) = ev[i + 1..].iter().find(|x| x.kind == EventKind::NoteOn).copied() else { continue };
        if !n.has(flags::TIE) && ev[i].tick + ev[i].gate > n.tick {
            ev[i].gate = (n.tick - ev[i].tick).max(1);
        }
    }
    out.sort();
}

// MUSICAL: first draft, tune by ear (user: Groove, the drums' feel).
/// Ticks each voice plays off the grid at full Groove: the snare laid back,
/// the hats and shakers pushed, the kick on the grid.
const GROOVE_PUSH: [(DrumVoice, i32); 6] = [
    (DrumVoice::Snare, 4),
    (DrumVoice::Hats, -2),
    (DrumVoice::Shaker, -2),
    (DrumVoice::Ride, -1),
    (DrumVoice::Perc, 2),
    (DrumVoice::Top, 1),
];
/// Velocity scale of the cymbals' off 16ths and on-beat 8ths at full Groove.
const GROOVE_OFF_VEL: f32 = 0.55;
const GROOVE_ON_VEL: f32 = 1.15;
/// Velocity of snare ghosts (hits below [`GHOST_VEL`]) at full Groove.
const GROOVE_GHOST_VEL: f32 = 0.7;
const GHOST_VEL: u8 = 70;

/// The drums' Groove beyond the swing: each voice pushed or laid back, the
/// cymbals' accent shape (strong 8ths, soft off 16ths), softer ghosts.
/// `amount` 0 plays the bar as rendered.
pub(crate) fn drums(out: &mut BarEvents, amount: f32) {
    let g = amount.clamp(0.0, 1.0);
    if g <= 0.0 {
        return;
    }
    for e in out.as_mut_slice() {
        let Some(voice) = DrumVoice::of_target(e.target) else { continue };
        if let Some((_, push)) = GROOVE_PUSH.iter().find(|(v, _)| *v == voice) {
            let to = e.tick as i32 + (*push as f32 * g).round() as i32;
            e.tick = to.clamp(0, TICKS_PER_BAR as i32 - 1) as u16;
            e.gate = e.gate.min(TICKS_PER_BAR - e.tick);
        }
        let scale = match voice {
            DrumVoice::Hats | DrumVoice::Ride | DrumVoice::Shaker => {
                let off = e.step % 2 == 1;
                1.0 + (if off { GROOVE_OFF_VEL } else { GROOVE_ON_VEL } - 1.0) * g
            }
            DrumVoice::Snare if e.vel < GHOST_VEL => 1.0 + (GROOVE_GHOST_VEL - 1.0) * g,
            _ => 1.0,
        };
        if e.vel > 0 && scale != 1.0 {
            e.vel = (e.vel as f32 * scale).round().clamp(1.0, 127.0) as u8;
        }
    }
    out.sort();
}

