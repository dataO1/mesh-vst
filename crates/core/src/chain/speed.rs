//! R3 Speed (D57): the Rate setting and a letter's speed step played on
//! the skeleton, and the speed limit (double speed keeps 16ths, half speed
//! 8ths).

use crate::event::{flags, BarEvents, EventKind, LiveEvent};
use crate::{STEPS_PER_BAR, TICKS_PER_BAR};

/// The finest grid (ticks) a bar plays at Rate `rate` and speed exponent
/// `exponent`: 8ths at half speed, else 16ths.
pub(crate) fn grid(rate: i8, exponent: i8) -> u16 {
    if rate <= -1 && exponent < 1 { TICKS_PER_BAR / 8 } else { TICKS_PER_BAR / 16 }
}

/// Play the bar at `2^exponent` speed (clamped to −3..=1). Below 1 each bar
/// plays the next part of the rendered bar stretched over the whole bar, or
/// the busiest part when the next one is far quieter (a bar of near silence
/// otherwise); a note held over the part start comes back on its first
/// tick. 1 plays the bar compressed into each half.
pub(crate) fn apply_speed(out: &mut BarEvents, exponent: i8, bar: u64, scratch: &mut BarEvents) {
    let half = TICKS_PER_BAR / 2;
    match exponent.clamp(-3, 1) {
        0 => return,
        e if e < 0 => {
            let k = 1u16 << (-e) as u16;
            let part = TICKS_PER_BAR / k;
            let from = busy_part(out.as_slice(), k, (bar % k as u64) as u16) * part;
            scratch.clear();
            for e in out.as_slice().iter().filter(|e| {
                e.kind == EventKind::NoteOn && e.gate > 0 && e.tick < from && e.tick + e.gate > from
            }) {
                let mut e = *e;
                e.gate = (e.tick + e.gate - from).min(part) * k;
                e.tick = 0;
                e.flags &= !(flags::TIE | flags::ACCENT | flags::REVERSE);
                // Picked up mid-note: its loop section (or reversed stretch) is behind it.
                e.loop_len = 0;
                e.note_id = e.note_id.wrapping_add(0x2000);
                scratch.push(e);
            }
            for e in out.as_slice().iter().filter(|e| e.tick >= from && e.tick < from + part) {
                let mut e = *e;
                e.tick = (e.tick - from) * k;
                e.gate = e.gate.saturating_mul(k).min(TICKS_PER_BAR - e.tick);
                scale_loop(&mut e, k as f32);
                scratch.push(e);
            }
        }
        _ => {
            scratch.clear();
            for offset in [0, half] {
                for e in out.as_slice() {
                    let mut e = *e;
                    e.tick = e.tick / 2 + offset;
                    e.gate = if e.gate > 0 { (e.gate / 2).max(1) } else { 0 };
                    scale_loop(&mut e, 0.5);
                    if offset > 0 {
                        // Distinct voices for the repeat (NoteOff matching).
                        e.note_id = e.note_id.wrapping_add(0x4000);
                    }
                    scratch.push(e);
                }
            }
            scratch.sort();
        }
    }
    std::mem::swap(out, scratch);
}

// MUSICAL: tune by ear (2026-10-20, the half speed centre: a seed's quiet
// half bar became a near silent bar).
/// A part this much quieter than the busiest one gives way to it.
const QUIET_PART: f32 = 0.35;
/// What a stab sounds for, whatever its gate (it decays).
const STAB_SOUNDS: u16 = crate::TICKS_PER_STEP;

/// Part `next` of the bar's `k` parts, or the busiest one when `next` plays
/// under [`QUIET_PART`] of it (sounding ticks times velocity).
fn busy_part(events: &[LiveEvent], k: u16, next: u16) -> u16 {
    let part = TICKS_PER_BAR / k;
    let score = |p: u16| -> f32 {
        let (lo, hi) = (p * part, (p + 1) * part);
        events
            .iter()
            .filter(|e| e.kind == EventKind::NoteOn && e.gate > 0)
            .map(|e| {
                let gate = if e.has(flags::STAB) { e.gate.min(STAB_SOUNDS) } else { e.gate };
                let covered = (e.tick + gate).min(hi).saturating_sub(e.tick.max(lo));
                covered as f32 * e.vel as f32
            })
            .sum()
    };
    let busiest = (0..k).map(|p| (p, score(p))).fold((next, score(next)), |a, b| if b.1 > a.1 { b } else { a });
    if score(next) < QUIET_PART * busiest.1 { busiest.0 } else { next }
}

/// A host's line (per played step) in the frame of a bar rendered for
/// `2^exponent` speed ([`apply_speed`]): at slow speeds the part bar `bar`
/// plays (and the other parts repeating it), at double speed the first
/// half; lengths stay in host steps ([`crate::splice::HostLine::step_ticks`]).
pub(crate) fn host_in_render(line: &crate::splice::HostLine, exponent: i8, bar: u64) -> crate::splice::HostLine {
    let e = exponent.clamp(-3, 1);
    if e == 0 || !line.on {
        return *line;
    }
    let k = if e < 0 { (1u32 << (-e) as u32) as f32 } else { 0.5 };
    let mut out = crate::splice::HostLine { on: true, relation: line.relation, step_ticks: line.step_ticks / k, ..Default::default() };
    for r in 0..STEPS_PER_BAR {
        // The played steps rendered step `r` covers.
        let (from, width, len) = if e < 0 {
            let k = 1usize << (-e) as usize;
            let part = STEPS_PER_BAR / k;
            let first = (bar % k as u64) as usize * part;
            (((r + STEPS_PER_BAR - first) % part) * k, k, line.lens[((r + STEPS_PER_BAR - first) % part) * k])
        } else {
            ((r / 2) % STEPS_PER_BAR, 1, line.lens[(r / 2) % STEPS_PER_BAR])
        };
        out.keys[r] = line.keys[from];
        out.lens[r] = len;
        if (from..(from + width).min(STEPS_PER_BAR)).any(|p| line.onsets & (1 << p) != 0) {
            out.onsets |= 1 << r;
        }
    }
    out
}

// (user, 2026-10-21: Density at full did not fill; pushes put every other
// 8th a few ticks early and it merged.) Share of the shortest gap a note
// keeps its own onset from.
const SPEED_SLACK: (u16, u16) = (3, 4);

/// The fastest notes a speed keeps (user, 2026-10-20): double speed stays
/// at 16ths and half speed at 8ths, instead of everything scaled (`rate`
/// the Rate setting, `exponent` with the letter's step). A note clearly
/// closer than that to the previous one goes ([`SPEED_SLACK`]: a pushed 8th
/// stays); the previous one holds over it.
pub(crate) fn limit_speed(out: &mut BarEvents, rate: i8, exponent: i8) {
    // The Rate setting decides; a block at double speed stays at 16ths too.
    let min = if rate >= 1 || exponent >= 1 {
        TICKS_PER_BAR / 16
    } else if rate <= -1 {
        TICKS_PER_BAR / 8
    } else {
        return;
    };
    let events = out.as_mut_slice();
    let mut kept = 0usize;
    let mut last: Option<usize> = None;
    for i in 0..events.len() {
        let e = events[i];
        if e.kind == EventKind::NoteOn {
            if let Some(l) = last {
                if e.tick < events[l].tick + min * SPEED_SLACK.0 / SPEED_SLACK.1 {
                    let p = &mut events[l];
                    p.gate = p.gate.max((e.tick + e.gate).saturating_sub(p.tick));
                    continue;
                }
            }
            last = Some(kept);
        }
        events[kept] = e;
        kept += 1;
    }
    out.truncate(kept);
}


/// A note's loop section at `k` times its speed's length (none when it no
/// longer fits its tick bytes).
pub(crate) fn scale_loop(e: &mut LiveEvent, k: f32) {
    let (at, len) = (e.loop_at as f32 * k, e.loop_len as f32 * k);
    if e.loop_len == 0 || at + len > u8::MAX as f32 || len < 1.0 {
        e.loop_len = 0;
    } else {
        (e.loop_at, e.loop_len) = (at as u8, len as u8);
    }
}
