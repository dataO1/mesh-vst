//! Splice (design D42): one lane arranges itself around another.
//!
//! A lane follows a host: another mesh instance or a sidechain. The host
//! tells, per 16th of the song, how much it plays there ([`Occupancy`]).
//! The Splice amount turns that into a focus per step of the follower's
//! bar ([`focus`]): below the middle the follower leans into the host's
//! hits (stacking), above it into the host's silences (the gaps), the
//! middle leaves the pattern as it is. The focus moves the follower's
//! density, variation and spread per step and thins its hits outside the
//! focus, so the extremes play mostly, not only, stacked or in the gaps.
//!
//! A pitched host also shares its line (the key and length of the note
//! sounding on each step) and every mesh host its form position (D46). The
//! amount is then one relation axis: 0 layers the follower on the host
//! (same notes, octaves and fifths, parallel motion, the host's lengths),
//! the middle complements it (independent lines, no parallel octaves or
//! fifths, some length contrast), 1 counters it (contrary motion, holding
//! while the host moves fast, the opposite lengths). The rhythm stays as
//! the focus says; blocks change with the host's.

use crate::event::{BarEvents, EventKind};
use crate::generator::layout::DrumVoice;
use crate::role::Role;
use crate::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// What a lane plays in one song bar, per 16th: 0 nothing .. 255 a strong
/// onset (sustained notes count less); a pitched lane's line, and where in
/// its form the bar is.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Occupancy {
    /// Song bar (song tick / [`TICKS_PER_BAR`]); `u64::MAX` = none.
    pub song_bar: u64,
    pub steps: [u8; STEPS_PER_BAR],
    /// The key sounding on each step (0 = none; a pitched lane only).
    pub keys: [u8; STEPS_PER_BAR],
    /// The length in steps of the note sounding on each step (0 = none).
    pub lens: [u8; STEPS_PER_BAR],
    /// What the lane's form is built from (D48): the phrase seeds of A..D,
    /// the fill and the second-pass seed.
    pub form_seeds: [u32; 6],
    /// The lane's pattern bar under the song bar's start.
    pub form_bar: u32,
    /// Steps a note starts on, one bit each.
    pub onsets: u16,
    /// Ticks into that pattern bar at the song bar's start.
    pub form_offset: u16,
    /// The form's Variation and Shape bands (low and high nibble).
    pub form_bands: u8,
    /// Whether the form is a drum lane's.
    pub form_drums: u8,
    /// [`PITCHED`], [`FORMED`].
    pub flags: u8,
    /// The form's Contrast band (D49).
    pub form_contrast: u8,
    /// The form's Density and Groove bands (low and high nibble, D51).
    pub form_rhythm: u8,
    pub _pad: [u8; 3],
}

/// [`Occupancy::flags`]: the line (keys, lengths) is set.
pub const PITCHED: u8 = 1;
/// [`Occupancy::flags`]: the form position is set.
pub const FORMED: u8 = 2;

impl Default for Occupancy {
    fn default() -> Self {
        Self {
            song_bar: u64::MAX,
            steps: [0; STEPS_PER_BAR],
            keys: [0; STEPS_PER_BAR],
            lens: [0; STEPS_PER_BAR],
            form_seeds: [0; 6],
            form_bar: 0,
            onsets: 0,
            form_offset: 0,
            form_bands: 0,
            form_drums: 0,
            flags: 0,
            form_contrast: 0,
            form_rhythm: 0,
            _pad: [0; 3],
        }
    }
}

impl Occupancy {
    /// Share what the lane's form is built from.
    pub fn set_form(&mut self, spec: &crate::arrange::FormSpec) {
        let s = &spec.seeds;
        self.form_seeds = [s.phrases[0], s.phrases[1], s.phrases[2], s.phrases[3], s.fill, s.second];
        self.form_bands = spec.variation & 0xF | (spec.shape & 0xF) << 4;
        self.form_drums = spec.drums as u8;
        self.form_contrast = spec.contrast;
        self.form_rhythm = spec.density & 0xF | (spec.groove & 0xF) << 4;
        self.flags |= FORMED;
    }

    /// What the host's form is built from.
    pub fn form_spec(&self) -> crate::arrange::FormSpec {
        let f = &self.form_seeds;
        crate::arrange::FormSpec {
            seeds: crate::arrange::FormSeeds { phrases: [f[0], f[1], f[2], f[3]], fill: f[4], second: f[5] },
            variation: self.form_bands & 0xF,
            shape: self.form_bands >> 4,
            contrast: self.form_contrast,
            density: self.form_rhythm & 0xF,
            groove: self.form_rhythm >> 4,
            drums: self.form_drums != 0,
        }
    }
}


/// A pitched host's line under one of the follower's bars, per step of
/// that bar, and the relation the follower takes to it (D46).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostLine {
    /// Whether a pitched host's line is known for this bar.
    pub on: bool,
    /// The host's sounding key per step, in the follower's key frame (0 = none).
    pub keys: [u8; STEPS_PER_BAR],
    /// The length in steps of the host note sounding on each step.
    pub lens: [u8; STEPS_PER_BAR],
    /// Steps the host starts a note on.
    pub onsets: u16,
    /// −1 layer .. 0 complement .. 1 counter (the Splice amount, 2a − 1).
    pub relation: f32,
    /// Ticks of the follower's rendered bar per host step ([`TICKS_PER_STEP`]
    /// unless the follower renders for another speed); the lengths stay in
    /// host steps.
    pub step_ticks: f32,
}

impl Default for HostLine {
    fn default() -> Self {
        Self { on: false, keys: [0; STEPS_PER_BAR], lens: [0; STEPS_PER_BAR], onsets: 0, relation: 0.0, step_ticks: TICKS_PER_STEP as f32 }
    }
}

/// Fill the steps between the host's notes with the last note's key and
/// length (the bar's last note carries into its start).
pub fn carry(line: &mut HostLine) {
    let Some(last) = (0..STEPS_PER_BAR).rev().find(|&s| line.keys[s] != 0) else { return };
    let (mut key, mut len) = (line.keys[last], line.lens[last]);
    for s in 0..STEPS_PER_BAR {
        if line.keys[s] != 0 {
            (key, len) = (line.keys[s], line.lens[s]);
        } else {
            (line.keys[s], line.lens[s]) = (key, len);
        }
    }
}

/// The line of a pitched bar: the key and length (in steps) of the note
/// sounding on each step, and the onset steps. Later notes win.
pub fn line(events: &BarEvents) -> ([u8; STEPS_PER_BAR], [u8; STEPS_PER_BAR], u16) {
    let (mut keys, mut lens, mut onsets) = ([0u8; STEPS_PER_BAR], [0u8; STEPS_PER_BAR], 0u16);
    for e in events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn && e.key != 0 && e.vel > 0) {
        let first = (e.tick / TICKS_PER_STEP) as usize;
        if first >= STEPS_PER_BAR {
            continue;
        }
        let len = (e.gate as u32).div_ceil(TICKS_PER_STEP as u32).max(1);
        let end = (first + len as usize).min(STEPS_PER_BAR);
        for s in first..end {
            keys[s] = e.key;
            lens[s] = len.min(255) as u8;
        }
        onsets |= 1 << first;
    }
    (keys, lens, onsets)
}

// MUSICAL: first draft, tune by ear (user, 2026-10-20: counterpoint between
// two instances is the main point; stacked layers, the middle has some
// contrast, the gaps contrast maximally in pitch, length and rhythm).
/// Layer: a candidate a unison, octave or fifth from the host, and one
/// moving the way the host moves (parallel or similar motion).
const LAYER_PERFECT: f32 = 6.0;
const LAYER_SAME_WAY: f32 = 3.0;
/// Complement: parallel octaves or fifths (and unisons) with the host.
const MID_PARALLEL_PERFECT: f32 = 0.2;
/// Counter: contrary motion, holding while the host moves fast, similar
/// motion and parallel perfect intervals.
const COUNTER_CONTRARY: f32 = 6.0;
const COUNTER_HOLD: f32 = 2.5;
const COUNTER_SAME_WAY: f32 = 0.2;
const COUNTER_PARALLEL_PERFECT: f32 = 0.02;
/// The host moves fast: this many onsets since the follower's last note,
/// or a jump of this many semitones.
const FAST_ONSETS: u32 = 2;
const FAST_SEMIS: i32 = 5;

fn perfect(a: i32, b: i32) -> bool {
    matches!((a - b).rem_euclid(12), 0 | 7)
}

/// How much the follower's next note `cand` (key) on step `s` fits the
/// host's line, its previous note `prev` (step, key): a weight on the pick,
/// 1 without a host note there. Blends the layer, complement and counter
/// weights by the relation (each in full at its end of the axis).
pub fn motion_weight(host: &HostLine, s: usize, prev: Option<(usize, i32)>, cand: i32) -> f32 {
    let Some(&h_now) = host.keys.get(s).filter(|_| host.on) else { return 1.0 };
    if h_now == 0 {
        return 1.0;
    }
    let h_now = h_now as i32;
    let now_perfect = perfect(cand, h_now);
    // Motion against the host since the previous note (both known).
    let motion = prev.and_then(|(ps, pk)| {
        let h_prev = *host.keys.get(ps)? as i32;
        (h_prev > 0).then_some((cand - pk, h_now - h_prev, perfect(pk, h_prev), ps))
    });
    let (mut layer, mut mid, mut counter) = (1.0f32, 1.0f32, 1.0f32);
    if now_perfect {
        layer *= LAYER_PERFECT;
    }
    if let Some((df, dh, was_perfect, ps)) = motion {
        let same_way = df != 0 && dh != 0 && df.signum() == dh.signum();
        let contrary = df != 0 && dh != 0 && df.signum() != dh.signum();
        let parallel_perfect = now_perfect && was_perfect && same_way;
        let still_perfect = now_perfect && was_perfect && df == 0 && dh == 0;
        if same_way || (df == 0 && dh == 0) {
            layer *= LAYER_SAME_WAY;
        }
        if parallel_perfect || still_perfect {
            mid *= MID_PARALLEL_PERFECT;
            counter *= COUNTER_PARALLEL_PERFECT;
        }
        if contrary {
            counter *= COUNTER_CONTRARY;
        } else if same_way {
            counter *= COUNTER_SAME_WAY;
        }
        let between = if s > ps { (host.onsets >> (ps + 1)) & ((1u32 << (s - ps)) - 1) as u16 } else { 0 };
        let fast = between.count_ones() >= FAST_ONSETS || dh.abs() >= FAST_SEMIS;
        if fast && df == 0 {
            counter *= COUNTER_HOLD;
        }
    }
    let r = host.relation.clamp(-1.0, 1.0);
    let (a, c) = ((-r).max(0.0), r.max(0.0));
    let b = 1.0 - r.abs();
    (a * layer.ln() + b * mid.ln() + c * counter.ln()).exp()
}

// MUSICAL: first draft, tune by ear (user, 2026-10-20: lengths alike when
// stacked, some contrast in the middle, the opposite in the gaps).
/// How far the follower's share of short notes moves to the length target.
const LENGTH_PULL: f32 = 0.7;
/// Length contrast at the middle of the axis (0 alike .. 1 opposite).
const MID_CONTRAST: f32 = 0.6;
/// Host notes this many steps long count as short, and this many as long.
const HOST_SHORT_STEPS: f32 = 2.0;
const HOST_LONG_STEPS: f32 = 5.0;

/// The follower's share of short notes (stabs) on step `s` against the
/// host's note there: its own `share` pulled towards the host's lengths
/// (layer) or away from them (counter). `share` itself without a host note.
pub fn short_share(host: &HostLine, s: usize, share: f32) -> f32 {
    let Some(&len) = host.lens.get(s).filter(|_| host.on) else { return share };
    if len == 0 {
        return share;
    }
    let long = ((len as f32 - HOST_SHORT_STEPS) / (HOST_LONG_STEPS - HOST_SHORT_STEPS)).clamp(0.0, 1.0);
    let target_short = (1.0 - long) + (2.0 * long - 1.0) * contrast(host.relation);
    share + (target_short - share) * LENGTH_PULL
}

/// The length contrast at relation `r`: 0 alike .. [`MID_CONTRAST`] .. 1 opposite.
fn contrast(r: f32) -> f32 {
    let r = r.clamp(-1.0, 1.0);
    if r < 0.0 { MID_CONTRAST * (1.0 + r) } else { MID_CONTRAST + (1.0 - MID_CONTRAST) * r }
}

/// The follower's note on step `s` (its `gate`, `room` ticks to its next
/// note) against the host's note there: alike takes the host's length,
/// opposite answers a long host note with a stab and a short one with a
/// note held to the next; blended by the relation and pulled by
/// [`LENGTH_PULL`]. `gate` unchanged without a host note.
pub fn contrast_gate(host: &HostLine, s: usize, gate: u16, room: u16) -> u16 {
    let Some(&len) = host.lens.get(s).filter(|_| host.on) else { return gate };
    if len == 0 {
        return gate;
    }
    let room = room.max(1) as f32;
    let long = ((len as f32 - HOST_SHORT_STEPS) / (HOST_LONG_STEPS - HOST_SHORT_STEPS)).clamp(0.0, 1.0);
    let step = host.step_ticks.max(1.0);
    let alike = (len as f32 * step).min(room);
    let opposite = room + (step.min(room) - room) * long;
    let target = alike + (opposite - alike) * contrast(host.relation);
    (gate as f32 + (target - gate as f32) * LENGTH_PULL).round().clamp(1.0, room) as u16
}

// MUSICAL: first draft, tune by ear (user: the gaps setting has the highest
// priority, the extremes play mostly, not only, in the focus).
/// Weight of a step a note holds through (the onset step is 1).
const SUSTAIN_WEIGHT: f32 = 0.5;
/// Drum voice weights: the backbone counts most.
fn voice_weight(v: DrumVoice) -> f32 {
    match v {
        DrumVoice::Kick | DrumVoice::Snare => 1.0,
        DrumVoice::Break | DrumVoice::Top => 0.4,
        DrumVoice::Perc => 0.3,
        // (user: the hats count only a little.)
        DrumVoice::Hats | DrumVoice::Ride | DrumVoice::Shaker => 0.12,
    }
}
/// Gaps: a hit this strong (0..1) spills this much onto the steps beside it.
const STRONG: f32 = 0.8;
const GAP_SPILL: f32 = 0.6;
/// How far the focus moves a step at the extremes (focus 1 ± this).
pub const FOCUS_RANGE: f32 = 0.85;
/// Most of the hits outside the focus dropped at the extremes.
pub const DROP_MAX: f32 = 0.85;
/// Most of the hits outside the focus moved into it at the extremes.
pub const MOVE_MAX: f32 = 0.85;
/// Most a generator parameter moves at full focus (per unit of focus).
pub const PARAM_SHIFT: f32 = 0.45;

/// What a rendered bar plays per step of its own (pattern) bar, 0..1.
/// `voices`: the drum voices the bar has on.
pub fn occupancy(role: Role, events: &BarEvents, voices: u8) -> [f32; STEPS_PER_BAR] {
    let mut occ = [0.0f32; STEPS_PER_BAR];
    for e in events.as_slice() {
        if !matches!(e.kind, EventKind::Trigger | EventKind::NoteOn) || e.vel == 0 {
            continue;
        }
        let w = match role {
            Role::Drums => match DrumVoice::of_target(e.target) {
                Some(v) if voices & v.bit() != 0 => voice_weight(v),
                _ => continue,
            },
            _ => 1.0,
        } * (e.vel as f32 / 127.0).sqrt();
        let first = (e.tick / TICKS_PER_STEP) as usize;
        if first >= STEPS_PER_BAR {
            continue;
        }
        occ[first] = occ[first].max(w);
        // A held note fills the steps it holds through.
        let end = ((e.tick as u32 + e.gate as u32).min(TICKS_PER_BAR as u32) / TICKS_PER_STEP as u32) as usize;
        for o in occ.iter_mut().take(end.min(STEPS_PER_BAR)).skip(first + 1) {
            *o = o.max(w * SUSTAIN_WEIGHT);
        }
    }
    occ
}

/// Pack 0..1 values into an [`Occupancy`].
pub fn pack(song_bar: u64, occ: &[f32; STEPS_PER_BAR]) -> Occupancy {
    Occupancy { song_bar, steps: occ.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8), ..Occupancy::default() }
}

/// The focus per step for Splice `amount` (0 stacked .. 0.5 off .. 1 gaps)
/// against the host's `occ` (0..1 per step): 1 = as generated, above 1 the
/// pattern leans in, below it thins out. Uniform when the host plays
/// everywhere alike.
pub fn focus(amount: f32, occ: &[f32; STEPS_PER_BAR]) -> [f32; STEPS_PER_BAR] {
    let s = 2.0 * amount.clamp(0.0, 1.0) - 1.0;
    if s == 0.0 {
        return [1.0; STEPS_PER_BAR];
    }
    // Gaps: a strong hit (kick, snare) keeps the steps beside it clear too,
    // so the gaps at the key moments are wide.
    let mut occ = *occ;
    if s > 0.0 {
        let src = occ;
        for (i, o) in occ.iter_mut().enumerate() {
            let near = [i.wrapping_sub(1), i + 1].iter().filter_map(|&j| src.get(j)).copied().filter(|v| *v >= STRONG).fold(0.0f32, f32::max);
            *o = o.max(GAP_SPILL * near);
        }
    }
    let target = occ.map(|o| if s < 0.0 { o } else { 1.0 - o });
    let mean = target.iter().sum::<f32>() / STEPS_PER_BAR as f32;
    let spread = target.iter().map(|t| (t - mean).abs()).fold(0.0f32, f32::max);
    if spread < 1e-3 {
        return [1.0; STEPS_PER_BAR];
    }
    target.map(|t| (1.0 + s.abs() * FOCUS_RANGE * (t - mean) / spread).clamp(1.0 - FOCUS_RANGE, 1.0 + FOCUS_RANGE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::blocks::{note, trigger};

    #[test]
    fn occupancy_weights_onsets_and_holds() {
        let mut b = BarEvents::default();
        b.push(note(0, 40, 127, 4 * TICKS_PER_STEP, 1));
        let occ = occupancy(Role::Bass, &b, 0);
        assert_eq!(occ[0], 1.0);
        assert!((occ[2] - SUSTAIN_WEIGHT).abs() < 1e-6 && occ[4] == 0.0);
        let mut d = BarEvents::default();
        d.push(trigger(4 * TICKS_PER_STEP, crate::generator::layout::BACKBONE_BASE, 127));
        let occ = occupancy(Role::Drums, &d, 0xFF);
        assert!(occ[4] > 0.9);
    }

    #[test]
    fn the_relation_weighs_motion_and_lengths() {
        // The host steps up from 40 to 42; the follower was on 40 (unison).
        let mut host = HostLine { on: true, ..Default::default() };
        host.keys[0] = 40;
        host.keys[4] = 42;
        host.lens[4] = 6;
        host.onsets = 1 | 1 << 4;
        let w = |r: f32, cand: i32| motion_weight(&HostLine { relation: r, ..host }, 4, Some((0, 40)), cand);
        // Up with the host to a unison again (parallel perfect), down against it.
        let (parallel, contrary) = (42, 38);
        assert!(w(-1.0, parallel) > w(-1.0, contrary), "layer moves with the host");
        assert!(w(1.0, contrary) > 50.0 * w(1.0, parallel), "counter: contrary, never parallel");
        assert!(w(0.0, parallel) < w(0.0, contrary), "complement avoids parallel unisons");
        // No host note: no say.
        assert_eq!(motion_weight(&HostLine { relation: 1.0, ..host }, 8, Some((0, 40)), 45), 1.0);
        // A long host note: layered notes go long (fewer stabs), countered short.
        let share = |r: f32| short_share(&HostLine { relation: r, ..host }, 4, 0.5);
        assert!(share(-1.0) < 0.3 && share(1.0) > 0.7 && share(0.0) > 0.5, "{} {} {}", share(-1.0), share(0.0), share(1.0));
        // A long host note: the layered note grows towards its length, the
        // countered one shrinks towards a 16th.
        let gate = |r: f32| contrast_gate(&HostLine { relation: r, ..host }, 4, 3 * TICKS_PER_STEP, 400);
        assert!(gate(-1.0) > 4 * TICKS_PER_STEP && gate(1.0) < 2 * TICKS_PER_STEP, "{} {}", gate(-1.0), gate(1.0));
    }

    #[test]
    fn a_line_holds_its_keys_and_lengths() {
        let mut b = BarEvents::default();
        b.push(note(0, 40, 127, 3 * TICKS_PER_STEP, 1));
        b.push(note(8 * TICKS_PER_STEP, 47, 127, TICKS_PER_STEP, 9));
        let (keys, lens, onsets) = line(&b);
        assert_eq!((keys[0], keys[2], keys[3], keys[8]), (40, 40, 0, 47));
        assert_eq!((lens[1], lens[8]), (3, 1));
        assert_eq!(onsets, 1 | 1 << 8);
    }

    #[test]
    fn focus_is_neutral_in_the_middle_and_leans_at_the_ends() {
        let mut occ = [0.0; STEPS_PER_BAR];
        for s in [0, 4, 8, 12] {
            occ[s] = 1.0;
        }
        assert_eq!(focus(0.5, &occ), [1.0; STEPS_PER_BAR]);
        let stacked = focus(0.0, &occ);
        let gaps = focus(1.0, &occ);
        assert!(stacked[4] > 1.5 && stacked[2] < 1.0);
        assert!(gaps[4] < 0.5 && gaps[2] > 1.0);
        // A host that plays every step alike leaves the pattern alone.
        assert_eq!(focus(1.0, &[0.6; STEPS_PER_BAR]), [1.0; STEPS_PER_BAR]);
        // Halfway is half the lean.
        let half = focus(0.75, &occ);
        assert!((1.0 - half[4]) < (1.0 - gaps[4]));
    }
}
