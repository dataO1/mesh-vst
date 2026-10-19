//! The eight meta knobs (design D37) and what they move.
//!
//! A knob is a position 0..1 around the seed's own suggestion: 0.5 plays the
//! value the sound seed and pattern seed suggest ([`Centers`]), 0 and 1 reach
//! the ends of the range. The mappings follow Mesh live's lane controls.

use crate::pattern::PatternLane;
use crate::role::Role;
use crate::rng;
use crate::sampler::{CcMap, LaneSamplerParams, ModConfig, ModEnv, ModLfo};

/// Knob positions, 0..1, 0.5 = the seed's suggestion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    pub density: f32,
    pub intensity: f32,
    pub variation: f32,
    pub swing: f32,
    pub shape: f32,
    pub morph: f32,
    /// Bass only.
    pub spread: f32,
    /// Bass only.
    pub slide: f32,
}

impl Default for Knobs {
    fn default() -> Self {
        Self::CENTERED
    }
}

impl Knobs {
    /// Every knob at the seed's suggestion.
    pub const CENTERED: Self =
        Self { density: 0.5, intensity: 0.5, variation: 0.5, swing: 0.5, shape: 0.5, morph: 0.5, spread: 0.5, slide: 0.5 };

    fn as_array(&self) -> [f32; 8] {
        [self.density, self.intensity, self.variation, self.swing, self.shape, self.morph, self.spread, self.slide]
    }

    fn from_array(a: [f32; 8]) -> Self {
        Self { density: a[0], intensity: a[1], variation: a[2], swing: a[3], shape: a[4], morph: a[5], spread: a[6], slide: a[7] }
    }
}

/// Where each knob's middle sits for a seed, as a value 0..1.
pub type Centers = Knobs;

// MUSICAL: first draft, tune by ear. The range each knob's seeded centre
// falls in, per mode (from the Mesh starter style's defaults and their
// usual playing range).
const DRUM_CENTRES: [(f32, f32); 8] =
    [(0.35, 0.65), (0.4, 0.6), (0.25, 0.55), (0.0, 0.35), (0.35, 0.7), (0.1, 0.7), (0.5, 0.5), (0.5, 0.5)];
const BASS_CENTRES: [(f32, f32); 8] =
    [(0.35, 0.65), (0.4, 0.6), (0.3, 0.6), (0.0, 0.25), (0.25, 0.75), (0.1, 0.9), (0.05, 0.4), (0.3, 0.7)];
const S_CENTRE: u32 = 0xC40;

impl Centers {
    /// The seed's suggestion per knob. The pattern seed places the groove
    /// knobs (density, variation, swing, spread, slide), the sound seed the
    /// sound knobs (intensity, shape, morph).
    pub fn of(role: Role, pattern_seed: u32, sound_seed: u32) -> Self {
        let table = if role == Role::Drums { &DRUM_CENTRES } else { &BASS_CENTRES };
        let sound = [false, true, false, false, true, true, false, false];
        let mut out = [0.5; 8];
        for (i, ((lo, hi), o)) in table.iter().zip(out.iter_mut()).enumerate() {
            let seed = if sound[i] { sound_seed } else { pattern_seed };
            let u = rng::unit_f32(rng::hash(seed, 0, i as u8, 0, S_CENTRE));
            *o = lo + (hi - lo) * u;
        }
        Self::from_array(out)
    }

    /// The values the knobs give, around these centres.
    pub fn resolve(&self, knobs: &Knobs) -> Knobs {
        let c = self.as_array();
        let k = knobs.as_array();
        Knobs::from_array(std::array::from_fn(|i| around(k[i], c[i])))
    }
}

/// Knob 0..1 to a value 0..1: 0.5 gives `centre`, the ends 0 and 1.
pub fn around(knob: f32, centre: f32) -> f32 {
    let k = knob.clamp(0.0, 1.0);
    let c = centre.clamp(0.0, 1.0);
    if k <= 0.5 {
        c * k * 2.0
    } else {
        c + (1.0 - c) * (k - 0.5) * 2.0
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Set the lane's generator parameters and the sampler's from resolved knob
/// values (see [`Centers::resolve`]).
pub fn apply(role: Role, v: &Knobs, lane: &mut PatternLane, sampler: &mut LaneSamplerParams) {
    match role {
        Role::Drums => apply_drums(v, lane, sampler),
        _ => apply_bass(v, lane, sampler),
    }
}

// MUSICAL: first draft of the drum macros, tune by ear.
fn apply_drums(v: &Knobs, lane: &mut PatternLane, p: &mut LaneSamplerParams) {
    lane.set_param("density", v.density);
    lane.set_param("variation", v.variation);
    lane.set_param("mutate", 0.6 * v.variation);
    lane.set_param("overlay_ghost", lerp(0.1, 0.6, v.variation));
    lane.set_param("swing", v.swing);
    lane.set_param("style", v.morph);
    lane.set_param("chop", lerp(0.15, 0.8, v.morph));
    lane.set_param("overlay_ride", 0.5);
    lane.set_param("overlay_shaker", 0.5);
    lane.set_param("overlay_top", 0.6);
    // Intensity: wider dynamics and harder accents.
    p.velocity_curve = lerp(1.0, 2.2, v.intensity);
    p.accent_gain = lerp(1.1, 2.0, v.intensity);
    // Shape: tight and dry .. open and ringing.
    p.sustain = lerp(0.35, 1.0, v.shape);
    p.decay = lerp(0.12, 0.8, v.shape);
    p.release = lerp(0.02, 0.15, v.shape);
}

// MUSICAL: from Mesh live's bass lane (shape, morph, spread, slide), the
// other knobs first draft.
fn apply_bass(v: &Knobs, lane: &mut PatternLane, p: &mut LaneSamplerParams) {
    lane.set_param("density", v.density);
    lane.set_param("register", 1.0);
    lane.set_param("variation", v.variation);
    lane.set_param("complexity", lerp(0.3, 0.9, v.variation));
    lane.set_param("accent", lerp(0.2, 0.9, v.intensity));
    lane.set_param("spread", v.spread);
    lane.set_param("slide", v.slide);
    // Shape: stabby, accented and moving .. sustained and calm.
    lane.set_param("length", lerp(0.4, 0.95, v.shape));
    lane.set_param("expression", lerp(1.0, 0.15, v.shape));
    lane.set_param("stabs", lerp(1.0, 0.0, v.shape));
    character(p, v.shape);
    // Intensity also deepens the filter sweep and the stab punch.
    p.modulation.env.to_cutoff = lerp(2400.0, 6000.0, v.intensity);
    p.punch *= lerp(0.6, 1.4, v.intensity);
    p.morph = v.morph;
}

// MUSICAL: Mesh live's Character macro.
/// Sustain level of held notes.
const HELD_SUSTAIN: f32 = 0.85;
/// Stab accent at the sustained end of the macro (full at the stabby end).
const STAB_PUNCH: f32 = 0.4;

/// Mesh's Character macro: the share of stabs and held notes, the filter
/// bite and the stab accent.
fn character(p: &mut LaneSamplerParams, v: f32) {
    let (decay, sustain) = p.designed_env;
    let stab = (1.0 - 2.0 * v).max(0.0);
    p.decay = decay;
    p.sustain = sustain.max(HELD_SUSTAIN);
    p.release = p.designed_release;
    p.expression = if v < 0.5 { 1.15 + 0.5 * stab } else { 1.15 - (2.0 * v - 1.0) };
    p.tone = 0.5 + 0.5 * v;
    p.resonance = (p.designed_res + 0.2 * stab).min(0.95);
    p.punch = STAB_PUNCH + (1.0 - STAB_PUNCH) * stab;
}

/// The mudpie voice as Mesh's mudpie sound packs designed it: a 400 Hz
/// lowpass opened by a fast mod envelope and a slow LFO, the Tone knob on
/// CC74 to the cutoff.
pub fn mudpie_params() -> LaneSamplerParams {
    let mut p = LaneSamplerParams {
        cutoff_hz: 400.0,
        resonance: 0.35,
        attack: 0.002,
        decay: 0.6,
        sustain: 0.85,
        release: 0.12,
        cutoff_glide: 0.03,
        ..LaneSamplerParams::default()
    };
    let mut m = ModConfig {
        env: ModEnv { decay: 0.4, sustain: 0.0, release: 0.2, to_pitch: 200.0, to_cutoff: 4800.0, ..ModEnv::default() },
        lfo: ModLfo { rate: 3.0, to_cutoff: 600.0, ..ModLfo::default() },
        vib_rate: 5.5,
        ..ModConfig::default()
    };
    m.cutoff_cc[0] = CcMap::new(74, 3600.0);
    p.modulation = m;
    p.designed_env = (p.decay, p.sustain);
    p.designed_release = p.release;
    p.designed_res = p.resonance;
    p
}

/// The drum kit's sampler params before the knobs.
pub fn drum_params() -> LaneSamplerParams {
    let mut p = LaneSamplerParams::default();
    p.designed_env = (p.decay, p.sustain);
    p.designed_release = p.release;
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_is_the_seeds_value_and_the_ends_reach_the_range() {
        for c in [0.0, 0.2, 0.5, 0.9, 1.0] {
            assert_eq!(around(0.5, c), c);
            assert_eq!(around(0.0, c), 0.0);
            assert_eq!(around(1.0, c), 1.0);
            assert!(around(0.25, c) <= c && around(0.75, c) >= c);
        }
    }

    #[test]
    fn centres_are_seeded_and_stay_in_range() {
        let a = Centers::of(Role::Drums, 1, 1);
        assert_eq!(a, Centers::of(Role::Drums, 1, 1));
        assert_ne!(a, Centers::of(Role::Drums, 2, 1));
        // The sound seed moves the sound knobs only.
        let b = Centers::of(Role::Drums, 1, 2);
        assert_eq!(a.density, b.density);
        assert_ne!(a.shape, b.shape);
        for s in 0..100 {
            let c = Centers::of(Role::Bass, s, s);
            for (v, (lo, hi)) in c.as_array().iter().zip(BASS_CENTRES) {
                assert!((lo..=hi).contains(v));
            }
        }
    }
}
