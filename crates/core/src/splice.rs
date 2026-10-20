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

use crate::event::{BarEvents, EventKind};
use crate::generator::layout::DrumVoice;
use crate::role::Role;
use crate::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// What a lane plays in one song bar, per 16th: 0 nothing .. 255 a strong
/// onset (sustained notes count less).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Occupancy {
    /// Song bar (song tick / [`TICKS_PER_BAR`]); `u64::MAX` = none.
    pub song_bar: u64,
    pub steps: [u8; STEPS_PER_BAR],
}

impl Default for Occupancy {
    fn default() -> Self {
        Self { song_bar: u64::MAX, steps: [0; STEPS_PER_BAR] }
    }
}

// MUSICAL: first draft, tune by ear (user: the gaps setting has the highest
// priority, the extremes play mostly, not only, in the focus).
/// Weight of a step a note holds through (the onset step is 1).
const SUSTAIN_WEIGHT: f32 = 0.5;
/// Drum voice weights: the backbone counts most.
fn voice_weight(v: DrumVoice) -> f32 {
    match v {
        DrumVoice::Kick | DrumVoice::Snare => 1.0,
        DrumVoice::Break | DrumVoice::Top => 0.7,
        DrumVoice::Perc => 0.6,
        DrumVoice::Hats | DrumVoice::Ride | DrumVoice::Shaker => 0.35,
    }
}
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
    Occupancy { song_bar, steps: occ.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8) }
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
