//! R5 Density fill (D56, D57): the rests a pitched lane's bar has left after
//! the recursion fill in clusters ([`crate::cluster`]). Density picks whole
//! gaps, each gets a run leading into the next note (growing) or trailing
//! the one before (fading), as hits or one held note. A fill repeats the
//! note before it (the bar's last before its first); the melody gives it
//! its pitch later. Every draw follows the letter seed and the gap's place.

use crate::cluster::{self, Cluster, MAX_CLUSTERS};
use crate::event::{flags, origin, BarEvents, EventKind};
use crate::rng;
use crate::TICKS_PER_BAR;

// MUSICAL: tune by ear (user, 2026-10-21: Density at full fills almost
// every rest; 2026-10-23: in clusters, not lone hits). Density from which
// the rests fill on the 8ths, from which on the finest grid the speed
// allows; how much softer a fill plays than the note it repeats, at the
// run's quiet end and at its loud end.
pub(crate) const FILL_8THS: f32 = 0.55;
pub(crate) const FILL_16THS: f32 = 0.8;
const FILL_VEL: (f32, f32) = (0.75, 0.92);
const S_FILL: u32 = 0xF55;

/// The share of the gaps Density fills (0 below [`FILL_8THS`], all at 1).
pub(crate) fn amount(density: f32) -> f32 {
    ((density - FILL_8THS) / (1.0 - FILL_8THS)).clamp(0.0, 1.0).sqrt()
}

/// Fill the rests of `out` at `density`, on the 8ths, or on `finest` (the
/// grid the speed allows) above [`FILL_16THS`]; a run holds one note at
/// chance `hold`. A note counts as sounding for `share` of its length (at
/// least a grid step): the Length phase after the fill may shorten it, and
/// the more Density the more of a held note's time the fill takes (the
/// fill then ends it). The bar stays sorted.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill(out: &mut BarEvents, density: f32, finest: u16, hold: f32, share: f32, seed: u32, lane: u8) {
    let a = amount(density);
    if a <= 0.0 {
        return;
    }
    let grid = if density > FILL_16THS { finest } else { finest.max(TICKS_PER_BAR / 8) }.max(1);
    let mut free = [false; 32];
    let points = ((TICKS_PER_BAR / grid) as usize).min(free.len());
    {
        let ev = out.as_slice();
        let on = || ev.iter().filter(|e| e.kind == EventKind::NoteOn && e.vel > 0);
        if on().next().is_none() {
            return;
        }
        // The more Density, the less of a held note counts.
        let share = share.clamp(0.0, 1.0) * (1.0 - a);
        let sounds = |g: u16| ((g as f32 * share) as u16).max(grid).min(g.max(1));
        for (k, f) in free.iter_mut().enumerate().take(points) {
            let t = k as u16 * grid;
            // A rest: nothing sounds there and nothing starts within the step.
            *f = !on().any(|e| (e.tick <= t && t < e.tick + sounds(e.gate)) || (t < e.tick && e.tick < t + grid));
        }
    }
    let draw = |p: usize, k: u32| rng::unit_f32(rng::hash(seed, lane, 0, p as u8, S_FILL + k));
    let mut runs = [Cluster::default(); MAX_CLUSTERS];
    let m = cluster::plan(&free[..points], a, hold, draw, &mut runs);
    for run in &runs[..m] {
        let t0 = run.start as u16 * grid;
        let hits = if run.hold { 1 } else { run.len };
        // A run up to the next note grows into it, one after a note fades.
        let lead_in = run.start + run.len >= points || !free[run.start + run.len];
        for h in 0..hits {
            let t = t0 + h as u16 * grid;
            let ev = out.as_slice();
            let on = || ev.iter().filter(|e| e.kind == EventKind::NoteOn && e.vel > 0);
            let Some(src) = on().rev().find(|e| e.tick < t).or_else(|| on().next_back()).copied() else { return };
            let next = on().map(|e| e.tick).filter(|&x| x > t).min().unwrap_or(TICKS_PER_BAR);
            let end = if run.hold { t0 + run.len as u16 * grid } else { t + grid };
            let pos = (h as f32 + 0.5) / hits as f32;
            let loud = if lead_in { pos } else { 1.0 - pos };
            let mut x = src;
            x.tick = t;
            x.step = (t / crate::TICKS_PER_STEP) as u8;
            x.gate = (end.min(next) - t).max(1);
            x.vel = ((src.vel as f32 * (FILL_VEL.0 + (FILL_VEL.1 - FILL_VEL.0) * loud)).round() as u8).max(1);
            x.flags &= !(flags::ACCENT | flags::TIE | flags::STAB | flags::FIGURE);
            // A held run is a sustain by design: the Length phase keeps it.
            if run.hold {
                x.flags |= flags::FIGURE;
            }
            x.glide = 0;
            x.loop_len = 0;
            x.origin = origin::FILL;
            x.group = 0;
            x.note_id = 300 + t / grid;
            out.push(x);
        }
    }
    out.sort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::LiveEvent;

    fn note(tick: u16, gate: u16) -> LiveEvent {
        LiveEvent { kind: EventKind::NoteOn, tick, gate, key: 40, vel: 100, step: (tick / 24) as u8, ..LiveEvent::default() }
    }

    fn bar(notes: &[(u16, u16)]) -> BarEvents {
        let mut b = BarEvents::default();
        for &(t, g) in notes {
            b.push(note(t, g));
        }
        b
    }

    #[test]
    fn below_its_start_nothing_fills_and_at_full_every_rest_does() {
        let mut b = bar(&[(0, 24), (192, 24)]);
        fill(&mut b, 0.5, 24, 0.0, 1.0, 3, 1);
        assert_eq!(b.len(), 2);
        // Even held notes give way at full Density.
        let mut b = bar(&[(0, 192), (192, 192)]);
        fill(&mut b, 1.0, 24, 0.0, 1.0, 3, 1);
        // Every 16th sounds.
        for k in 0..16u16 {
            let t = k * 24;
            assert!(b.as_slice().iter().any(|e| e.tick <= t && t < e.tick + e.gate), "16th {k} silent: {:?}", b.as_slice().iter().map(|e| (e.tick, e.gate)).collect::<Vec<_>>());
        }
        assert!(b.as_slice().iter().filter(|e| e.origin == origin::FILL).all(|e| !e.has(flags::ACCENT)));
    }

    #[test]
    fn fills_come_in_runs_and_repeat_alike() {
        for seed in 0..30 {
            let mut b = bar(&[(0, 24), (96, 24), (240, 24)]);
            fill(&mut b, 0.75, 24, 0.0, 1.0, seed, 1);
            let mut c = bar(&[(0, 24), (96, 24), (240, 24)]);
            fill(&mut c, 0.75, 24, 0.0, 1.0, seed, 1);
            assert_eq!(b.as_slice(), c.as_slice());
            // No fill stands alone: each has a fill or a note next to it.
            let ticks: Vec<u16> = b.as_slice().iter().map(|e| e.tick).collect();
            for e in b.as_slice().iter().filter(|e| e.origin == origin::FILL) {
                assert!(ticks.contains(&(e.tick + 48)) || ticks.contains(&e.tick.wrapping_sub(48)) || ticks.contains(&(e.tick + 24)) || ticks.contains(&e.tick.wrapping_sub(24)), "seed {seed}: lone fill at {}", e.tick);
            }
        }
    }
}
