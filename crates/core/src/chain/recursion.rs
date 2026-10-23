//! R4 Recursion (D57): some notes of the bar are replaced by a pattern of
//! their own, once (depth 1).
//!
//! - **Subdivision:** the note's span takes a pattern made again from the
//!   lane's skeleton with a seed of the note: a window of it (twice or four
//!   times the span, placed by the seed and the note, moved by Variation)
//!   squeezed into the span, on a grid of 32nds at most. Its notes form a
//!   group ([`origin::RECURSION`], [`LiveEvent::group`]), never subdivided
//!   again; the melody gives the group its own short line.
//! - **Doubling:** the note takes twice its length; the notes it now
//!   overlaps go, the pattern after it stays where it was.
//!
//! Groove sets how often (none at 0); subdivision is three times as likely
//! as doubling, held notes lean to subdivision, short ones to doubling. The
//! drums subdivide only, as rolls and flams. Every draw follows the letter
//! seed and the note's written step, so a letter recurses alike wherever
//! it plays.

use crate::event::{flags, origin, BarEvents, EventKind, LiveEvent};
use crate::generator::layout::DrumVoice;
use crate::rng;
use crate::{TICKS_PER_BAR, TICKS_PER_STEP};

// MUSICAL: tune by ear (user, 2026-10-23: about one note in ten at the
// middle of Groove, one in three at the top; subdivision more often than
// doubling). The share of notes at full Groove and its curve; the weights
// of subdivision and doubling, and how a held note (from [`HELD_TICKS`])
// or a short one leans them; the finest grid of a subdivision (a 32nd).
const RATE: f32 = 0.33;
const RATE_CURVE: f32 = 1.73;
const SUBDIVIDE_WEIGHT: f32 = 3.0;
const DOUBLE_WEIGHT: f32 = 1.0;
const HELD_TICKS: u16 = 2 * TICKS_PER_STEP;
const LEAN: (f32, f32) = (1.5, 0.5);
const GRID: u16 = TICKS_PER_STEP / 2;
/// Shortest span a note subdivides (two 32nds).
const SPAN_MIN: u16 = 2 * GRID;
/// Drums: the roll's hit spacings (a 32nd, a 16th triplet) and its longest
/// span, its velocity ramp, a flam's grace note ahead (ticks) and velocity,
/// and the share of flams among the drums' recursions.
const ROLL_SPACING: [u16; 2] = [GRID, TICKS_PER_BAR / 24];
const ROLL_SPAN: u16 = 2 * TICKS_PER_STEP;
const ROLL_VEL: (f32, f32) = (0.55, 0.9);
const FLAM_AHEAD: u16 = 6;
const FLAM_VEL: f32 = 0.5;
const FLAM_SHARE: f32 = 0.35;
/// The drums recurse this share of a pitched lane's rate (their hats are
/// many: a roll in every bar would crowd them).
const DRUM_RATE: f32 = 0.4;
const S_RECURSE: u32 = 0xF90;

/// The share of notes Groove `amount` recurses.
pub(crate) fn rate(amount: f32) -> f32 {
    RATE * amount.clamp(0.0, 1.0).powf(RATE_CURVE)
}

/// The seed of a note's own pattern.
fn note_seed(seed: u32, lane: u8, e: &LiveEvent) -> u32 {
    rng::hash(seed, lane, e.step, (e.note_id & 0xFF) as u8, S_RECURSE + 7) as u32
}

/// Recurse a pitched lane's bar `out` at Groove `amount` and `variation`
/// for the letter `seed`. `child(seed, buf)` renders the lane's skeleton
/// (R1 to R3) with another seed into `buf` (cleared first). `scratch` and
/// `child_buf` are work buffers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pitched(
    out: &mut BarEvents,
    amount: f32,
    variation: f32,
    seed: u32,
    lane: u8,
    child: &mut dyn FnMut(u32, &mut BarEvents),
    scratch: &mut BarEvents,
    child_buf: &mut BarEvents,
) {
    let r = rate(amount);
    if r <= 0.0 {
        return;
    }
    scratch.clear();
    let ev = out.as_slice();
    let mut group = 0u8;
    // Up to where a doubled note holds: the notes it overlaps go.
    let mut held_to = 0u16;
    for i in 0..ev.len() {
        let e = ev[i];
        if e.kind != EventKind::NoteOn {
            scratch.push(e);
            continue;
        }
        if e.tick < held_to {
            continue;
        }
        let o = origin::of(e.origin);
        let fresh = o != origin::RECURSION && e.origin & origin::DOUBLED == 0;
        let draw = |k: u32| rng::unit_f32(rng::hash(seed, lane, e.step, (e.note_id & 0xFF) as u8, S_RECURSE + k));
        let next = ev[i + 1..].iter().find(|x| x.kind == EventKind::NoteOn).map_or(TICKS_PER_BAR, |x| x.tick);
        if !fresh || draw(0) >= r {
            scratch.push(e);
            continue;
        }
        let held = e.gate >= HELD_TICKS;
        let sub = SUBDIVIDE_WEIGHT * if held { LEAN.0 } else { LEAN.1 };
        let dbl = DOUBLE_WEIGHT * if held { LEAN.1 } else { LEAN.0 };
        let span = e.gate.min(next - e.tick);
        if draw(1) * (sub + dbl) < sub && span >= SPAN_MIN {
            group = group.wrapping_add(1).max(1);
            child_buf.clear();
            child(note_seed(seed, lane, &e), child_buf);
            if subdivide(&e, span, group, variation, draw(2), draw(3), child_buf, scratch) {
                continue;
            }
            scratch.push(e);
        } else {
            let mut d = e;
            d.gate = (e.gate.saturating_mul(2)).min(TICKS_PER_BAR - e.tick);
            d.origin |= origin::DOUBLED;
            held_to = e.tick + d.gate;
            scratch.push(d);
        }
    }
    scratch.sort();
    std::mem::swap(out, scratch);
}

/// Replace `parent` (sounding `span` ticks) by the window of `pattern` its
/// draws place, squeezed into the span, as group `group`. False when the
/// window holds no note to start on.
#[allow(clippy::too_many_arguments)]
fn subdivide(parent: &LiveEvent, span: u16, group: u8, variation: f32, place: f32, moved: f32, pattern: &BarEvents, out: &mut BarEvents) -> bool {
    let bar = TICKS_PER_BAR as u32;
    // Twice the span, four times a span of a beat or more (a bar at most).
    let k: u32 = if span as u32 >= bar / 4 { 4 } else { 2 };
    let width = (span as u32 * k).min(bar);
    let room = bar - width;
    let beat = bar / 4;
    let want = ((place + variation.clamp(0.0, 1.0) * moved) * room as f32) as u32 % (room + 1).max(1);
    let want = want / beat * beat;
    let notes = || pattern.as_slice().iter().filter(|x| x.kind == EventKind::NoteOn && x.vel > 0);
    // The window starts on a note, so the group strikes where the parent did.
    let Some(start) = notes().map(|x| x.tick as u32).filter(|&t| t >= want && t < want + width).min() else { return false };
    let end = start + width;
    let mut last: Option<u16> = None;
    let mut n = 0u16;
    for c in notes().filter(|x| (x.tick as u32) >= start && (x.tick as u32) < end) {
        let off = ((c.tick as u32 - start) / k) as u16;
        let off = (off + GRID / 2) / GRID * GRID;
        if off >= span || last.is_some_and(|l| l == off) {
            continue;
        }
        let mut x = *c;
        x.tick = parent.tick + off;
        x.step = (x.tick / TICKS_PER_STEP) as u8;
        x.gate = ((c.gate as u32 / k) as u16).clamp(GRID / 2, span - off);
        x.vel = if n == 0 { parent.vel } else { c.vel };
        x.flags = (if n == 0 { parent.flags & flags::ACCENT } else { 0 }) | (c.flags & flags::STAB);
        x.glide = 0;
        x.loop_len = 0;
        x.origin = origin::RECURSION;
        x.group = group;
        x.note_id = 0x500 + group as u16 * 32 + n;
        x.target = parent.target;
        out.push(x);
        last = Some(off);
        n += 1;
    }
    n > 0
}

/// Recurse a drum bar: some hits of the snare, hats, percussion and tops
/// become rolls (32nds or 16th triplets up to the next hit of the voice) or
/// flams (a grace hit just ahead).
pub(crate) fn drums(out: &mut BarEvents, amount: f32, seed: u32, lane: u8, scratch: &mut BarEvents) {
    let r = DRUM_RATE * rate(amount);
    if r <= 0.0 {
        return;
    }
    scratch.clear();
    let ev = out.as_slice();
    for (i, e) in ev.iter().enumerate() {
        scratch.push(*e);
        if !matches!(e.kind, EventKind::Trigger | EventKind::NoteOn) || e.vel == 0 {
            continue;
        }
        let Some(voice) = DrumVoice::of_target(e.target) else { continue };
        if !matches!(voice, DrumVoice::Snare | DrumVoice::Hats | DrumVoice::Perc | DrumVoice::Top) {
            continue;
        }
        let draw = |k: u32| rng::unit_f32(rng::hash(seed, lane, e.step, e.target, S_RECURSE + 32 + k));
        if draw(0) >= r {
            continue;
        }
        if draw(1) < FLAM_SHARE {
            if e.tick >= FLAM_AHEAD {
                let mut g = *e;
                g.tick = e.tick - FLAM_AHEAD;
                g.vel = ((e.vel as f32 * FLAM_VEL) as u8).max(1);
                g.flags &= !flags::ACCENT;
                g.origin = origin::RECURSION;
                scratch.push(g);
            }
            continue;
        }
        let next = ev[i + 1..].iter().find(|x| x.target == e.target && matches!(x.kind, EventKind::Trigger | EventKind::NoteOn)).map_or(TICKS_PER_BAR, |x| x.tick);
        let span = (next - e.tick).min(ROLL_SPAN);
        let spacing = ROLL_SPACING[(draw(2) * ROLL_SPACING.len() as f32) as usize % ROLL_SPACING.len()];
        let hits = span / spacing;
        for k in 1..hits {
            let mut x = *e;
            x.tick = e.tick + k * spacing;
            let t = k as f32 / hits as f32;
            x.vel = ((e.vel as f32 * (ROLL_VEL.0 + (ROLL_VEL.1 - ROLL_VEL.0) * t)) as u8).max(1);
            x.flags &= !flags::ACCENT;
            x.origin = origin::RECURSION;
            x.note_id = e.note_id.wrapping_add(0x600 + k);
            scratch.push(x);
        }
    }
    scratch.sort();
    std::mem::swap(out, scratch);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(tick: u16, gate: u16, id: u16) -> LiveEvent {
        LiveEvent { kind: EventKind::NoteOn, tick, gate, key: 40, vel: 100, step: (tick / TICKS_PER_STEP) as u8, note_id: id, ..LiveEvent::default() }
    }

    fn bar(notes: &[(u16, u16)]) -> BarEvents {
        let mut b = BarEvents::default();
        for (i, &(t, g)) in notes.iter().enumerate() {
            b.push(note(t, g, i as u16 + 1));
        }
        b
    }

    /// A child skeleton: 16ths with a gap (a pattern of its own).
    fn child(seed: u32, out: &mut BarEvents) {
        out.clear();
        for s in 0..16u16 {
            if (seed >> (s % 7)) & 1 == 1 || s % 4 == 0 {
                out.push(note(s * TICKS_PER_STEP, TICKS_PER_STEP, s + 1));
            }
        }
    }

    fn run(groove: f32, seed: u32) -> BarEvents {
        let mut b = bar(&[(0, 96), (96, 48), (144, 48), (192, 192)]);
        let (mut s, mut c) = (BarEvents::default(), BarEvents::default());
        pitched(&mut b, groove, 0.5, seed, 1, &mut child, &mut s, &mut c);
        b
    }

    #[test]
    fn no_groove_no_recursion_and_it_repeats_exactly() {
        assert_eq!(run(0.0, 4).as_slice(), bar(&[(0, 96), (96, 48), (144, 48), (192, 192)]).as_slice());
        assert_eq!(run(1.0, 4).as_slice(), run(1.0, 4).as_slice());
    }

    #[test]
    fn subdivisions_stay_in_their_span_once_and_doublings_clear_what_they_cover() {
        let (mut groups, mut doubled) = (0, 0);
        for seed in 0..200 {
            let b = run(1.0, seed);
            let ev = b.as_slice();
            for w in ev.windows(2) {
                assert!(w[0].tick <= w[1].tick);
            }
            for e in ev {
                if origin::of(e.origin) == origin::RECURSION {
                    groups += 1;
                    assert!(e.group > 0 && e.tick % GRID == 0);
                }
                if e.origin & origin::DOUBLED != 0 {
                    doubled += 1;
                    // Nothing starts under a doubled note.
                    assert!(!ev.iter().any(|x| x.tick > e.tick && x.tick < e.tick + e.gate), "seed {seed}: {ev:?}");
                }
            }
            // Recursing again leaves the groups and the doubled notes alone.
            let mut again = b.clone();
            let (mut s, mut c) = (BarEvents::default(), BarEvents::default());
            pitched(&mut again, 1.0, 0.5, seed, 1, &mut child, &mut s, &mut c);
            for e in again.as_slice().iter().filter(|e| e.group > 0) {
                assert!(b.as_slice().iter().any(|x| x.tick == e.tick && x.group == e.group), "seed {seed}: a group was subdivided again");
            }
        }
        assert!(groups > 0 && doubled > 0 && groups > doubled, "{groups} group notes, {doubled} doubled");
    }

    #[test]
    fn more_groove_recurses_more() {
        let count = |g: f32| (0..200).map(|s| run(g, s).as_slice().iter().filter(|e| e.group > 0 || e.origin & origin::DOUBLED != 0).count()).sum::<usize>();
        assert!(count(0.5) < count(1.0) && count(0.5) > 0);
    }
}
