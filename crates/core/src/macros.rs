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
    /// Rearranges the bar: beat chunks swap places (0 = as generated).
    pub permute: f32,
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
    /// Every knob at the seed's suggestion, Permute off.
    pub const CENTERED: Self =
        Self { density: 0.5, permute: 0.0, variation: 0.5, swing: 0.5, shape: 0.5, morph: 0.5, spread: 0.5, slide: 0.5 };

    fn as_array(&self) -> [f32; 8] {
        [self.density, self.permute, self.variation, self.swing, self.shape, self.morph, self.spread, self.slide]
    }

    fn from_array(a: [f32; 8]) -> Self {
        Self { density: a[0], permute: a[1], variation: a[2], swing: a[3], shape: a[4], morph: a[5], spread: a[6], slide: a[7] }
    }
}

/// Where each knob's middle sits for a seed, as a value 0..1.
pub type Centers = Knobs;

// MUSICAL: first draft, tune by ear. The range each knob's seeded centre
// falls in, per mode (from the Mesh starter style's defaults and their
// usual playing range).
const DRUM_CENTRES: [(f32, f32); 8] =
    [(0.35, 0.65), (0.5, 0.5), (0.25, 0.55), (0.0, 0.35), (0.35, 0.7), (0.1, 0.7), (0.5, 0.5), (0.5, 0.5)];
const BASS_CENTRES: [(f32, f32); 8] =
    [(0.35, 0.65), (0.5, 0.5), (0.3, 0.6), (0.0, 0.25), (0.25, 0.75), (0.1, 0.9), (0.05, 0.4), (0.3, 0.7)];
const S_CENTRE: u32 = 0xC40;

impl Centers {
    /// The seed's suggestion per knob. The pattern seed places the groove
    /// knobs (density, variation, swing, spread, slide), the sound seed the
    /// sound knobs (shape, morph). Permute has no seeded centre: the knob is
    /// the amount.
    pub fn of(role: Role, pattern_seed: u32, sound_seed: u32) -> Self {
        let table = if role == Role::Drums { &DRUM_CENTRES } else { &BASS_CENTRES };
        let sound = [false, false, false, false, true, true, false, false];
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

/// A lane's seeds, form and knob positions: everything that decides its
/// pattern and sound apart from the samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneSettings {
    pub knobs: Knobs,
    pub sound_seed: u32,
    pub pattern_seed: u32,
    /// 0 = one bar of A, 1..=10 = the default forms.
    pub form: usize,
    /// Speed exponent: −1 half, 0, 1 double.
    pub rate: i8,
    /// Mode in the scale family.
    pub mode: u8,
    /// Drums: the style (index into [`DRUM_STYLES`]); `None` lets the pattern
    /// seed pick one.
    pub style: Option<u8>,
    /// Bass: unison limits and mode (D38).
    pub unison: crate::sampler::Unison,
}

/// Configure `lane` for `s` and return the sampler params, from `base`.
pub fn configure(lane: &mut PatternLane, base: LaneSamplerParams, s: &LaneSettings) -> LaneSamplerParams {
    let role = lane.role();
    let resolved = Centers::of(role, s.pattern_seed, s.sound_seed).resolve(&s.knobs);
    let mut sp = base;
    lane.pattern_seed = s.pattern_seed;
    lane.sound_seed = s.sound_seed;
    lane.rate = s.rate;
    lane.mode = s.mode;
    lane.select_form(s.form);
    // Accent strength: the sound seed's, scaled by Variation.
    let seeded = rng::range_f32(rng::hash(s.sound_seed, 0, 0, 0, S_ACCENT), 0.3, 0.7);
    let accent = (seeded * (0.5 + resolved.variation)).clamp(0.0, 1.0);
    lane.permute = resolved.permute;
    apply(role, &resolved, accent, lane, &mut sp);
    if role == Role::Drums {
        lane.set_param("style", style_value(s.style.unwrap_or_else(|| seeded_style(s.pattern_seed))));
    } else {
        sp.unison = s.unison;
        seeded_voice(&mut sp, s.sound_seed);
    }
    sp
}

const S_VOICE: u32 = 0xC50;
const S_ACCENT: u32 = 0xC51;

// MUSICAL: first draft, tune by ear. Around the designed mudpie voice.
/// The sound seed's mudpie voice (design D20): filter, resonance, filter and
/// pitch envelopes and the LFO, drawn around the designed voice. The knobs
/// act on top.
fn seeded_voice(p: &mut LaneSamplerParams, sound_seed: u32) {
    let r = |k: u8, lo: f32, hi: f32| rng::range_f32(rng::hash(sound_seed, k, 0, 0, S_VOICE), lo, hi);
    // Cutoff 160 Hz .. 1.4 kHz around the designed 400 Hz.
    p.cutoff_hz *= 2f32.powf(r(0, -1.3, 1.8));
    p.resonance = (p.resonance + r(1, -0.25, 0.45)).clamp(0.05, 0.9);
    let m = &mut p.modulation;
    m.env.decay *= 2f32.powf(r(2, -1.6, 1.3));
    // A closed, dark voice .. a wide-open sweep.
    m.env.to_cutoff *= r(3, 0.2, 2.0);
    // A pitch blip on some voices, none on others.
    m.env.to_pitch = if r(4, 0.0, 1.0) < 0.4 { 0.0 } else { r(5, 60.0, 600.0) };
    // A slow drift .. a fast wobble, light .. deep.
    m.lfo.rate = 2f32.powf(r(6, -1.5, 3.0));
    m.lfo.to_cutoff = r(7, 0.0, 2800.0);
    p.decay *= 2f32.powf(r(8, -0.8, 0.8));
    p.release *= 2f32.powf(r(9, -0.7, 1.2));
}

/// The drum styles, in the generator's archetype order (design D39).
pub const DRUM_STYLES: [&str; 5] = crate::generator::drums::DEFAULT_ARCHETYPE_ORDER;
const S_STYLE: u32 = 0xC41;

/// The style a pattern seed picks when the user leaves it to the seed.
pub fn seeded_style(pattern_seed: u32) -> u8 {
    (rng::hash(pattern_seed, 0, 0, 0, S_STYLE) % DRUM_STYLES.len() as u64) as u8
}

/// The generator's `style` value that plays style `index` exactly, unblended.
pub fn style_value(index: u8) -> f32 {
    let last = (DRUM_STYLES.len() - 1) as f32;
    (index as f32).min(last) / last
}

/// Set the lane's generator parameters and the sampler's from resolved knob
/// values (see [`Centers::resolve`]).
/// `accent` 0..1: accent strength and dynamics (drums), accents, filter
/// sweep and stab punch (bass).
pub fn apply(role: Role, v: &Knobs, accent: f32, lane: &mut PatternLane, sampler: &mut LaneSamplerParams) {
    match role {
        Role::Drums => apply_drums(v, accent, lane, sampler),
        _ => apply_bass(v, accent, lane, sampler),
    }
}

// MUSICAL: first draft of the drum macros, tune by ear.
fn apply_drums(v: &Knobs, accent: f32, lane: &mut PatternLane, p: &mut LaneSamplerParams) {
    lane.set_param("density", v.density);
    lane.set_param("variation", v.variation);
    lane.set_param("overlay_ghost", lerp(0.1, 0.6, v.variation));
    lane.set_param("swing", v.swing);
    // Morph: straight .. chopped, inside the style the Style setting picks.
    lane.set_param("chop", v.morph);
    lane.set_param("mutate", (0.6 * v.variation + 0.4 * v.morph).min(1.0));
    lane.set_param("overlay_ride", 0.5);
    lane.set_param("overlay_shaker", 0.5);
    lane.set_param("overlay_top", 0.6);
    // Accent: wider dynamics and harder accents.
    p.velocity_curve = lerp(1.0, 2.2, accent);
    p.accent_gain = lerp(1.1, 2.0, accent);
    // Shape: tight and dry .. open and ringing.
    p.sustain = lerp(0.35, 1.0, v.shape);
    p.decay = lerp(0.12, 0.8, v.shape);
    p.release = lerp(0.02, 0.15, v.shape);
    // The choke amount follows Shape: tight cuts, open lets the hats ring (D27).
    p.choke_amount = lerp(1.0, 0.2, v.shape);
}

// MUSICAL: from Mesh live's bass lane (shape, morph, spread, slide), the
// other knobs first draft.
fn apply_bass(v: &Knobs, accent: f32, lane: &mut PatternLane, p: &mut LaneSamplerParams) {
    lane.set_param("density", v.density);
    lane.set_param("register", 1.0);
    lane.set_param("variation", v.variation);
    lane.set_param("complexity", lerp(0.3, 0.9, v.variation));
    lane.set_param("accent", lerp(0.2, 0.9, accent));
    lane.set_param("spread", v.spread);
    lane.set_param("slide", v.slide);
    // Shape: stabby, accented and moving .. sustained and calm.
    lane.set_param("length", lerp(0.4, 0.95, v.shape));
    lane.set_param("expression", lerp(1.0, 0.15, v.shape));
    lane.set_param("stabs", lerp(1.0, 0.0, v.shape));
    character(p, v.shape);
    // Accent also deepens the filter sweep and the stab punch.
    p.modulation.env.to_cutoff = lerp(2400.0, 6000.0, accent);
    p.punch *= lerp(0.6, 1.4, accent);
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

#[cfg(test)]
mod style_tests {
    use super::*;
    use crate::generator::DrumLayout;
    use crate::key::MusicalKey;
    use crate::pattern::{RenderedBar, Scratch};
    use crate::sampler::LaneSamplerParams;

    #[test]
    fn each_style_plays_its_own_rhythm_and_the_seed_picks_one() {
        let mut bars = Vec::new();
        for style in 0..DRUM_STYLES.len() as u8 {
            let mut lane = PatternLane::drums(DrumLayout::standard(), 5, 5);
            let s = LaneSettings { knobs: Knobs::CENTERED, sound_seed: 5, pattern_seed: 5, form: 0, rate: 0, mode: 0, style: Some(style), unison: Default::default() };
            configure(&mut lane, LaneSamplerParams::default(), &s);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            lane.render(0, MusicalKey::new(0, true), &mut out, &mut scratch);
            bars.push(out.events.as_slice().iter().map(|e| (e.tick, e.target)).collect::<Vec<_>>());
        }
        for i in 0..bars.len() {
            for j in i + 1..bars.len() {
                assert_ne!(bars[i], bars[j], "styles {i} and {j}");
            }
        }
        let picks: std::collections::BTreeSet<u8> = (0..200).map(seeded_style).collect();
        assert_eq!(picks.len(), DRUM_STYLES.len());
    }

    #[test]
    fn the_sound_seed_rolls_the_mudpie_voice() {
        let voice = |seed| {
            let mut lane = PatternLane::bass(5, seed);
            let s = LaneSettings { knobs: Knobs::CENTERED, sound_seed: seed, pattern_seed: 5, form: 1, rate: 0, mode: 0, style: None, unison: Default::default() };
            let p = configure(&mut lane, mudpie_params(), &s);
            (p.cutoff_hz, p.resonance, p.modulation.lfo.rate, p.modulation.env.decay)
        };
        let voices: Vec<_> = (0..20).map(voice).collect();
        assert_eq!(voice(3), voice(3), "seeded");
        for v in &voices {
            assert!((100.0..1600.0).contains(&v.0) && (0.05..=0.9).contains(&v.1), "{v:?}");
        }
        let cutoffs: Vec<f32> = voices.iter().map(|v| v.0).collect();
        let (lo, hi) = cutoffs.iter().fold((f32::MAX, 0.0f32), |(a, b), &c| (a.min(c), b.max(c)));
        assert!(hi / lo > 2.5, "voices differ: {lo}..{hi} Hz");
    }

    /// Eight bars of drums at style `style` with `knobs`, as (bar, tick, target).
    fn bars(style: u8, seed: u32, knobs: Knobs) -> Vec<(u64, u16, u8)> {
        let mut lane = PatternLane::drums(DrumLayout::standard(), seed, seed);
        let s = LaneSettings { knobs, sound_seed: seed, pattern_seed: seed, form: 1, rate: 0, mode: 0, style: Some(style), unison: Default::default() };
        configure(&mut lane, LaneSamplerParams::default(), &s);
        let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
        (0..8u64)
            .flat_map(|b| {
                lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                out.events.as_slice().iter().map(|e| (b, e.tick, e.target)).collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn swing_and_morph_audibly_change_every_style() {
        let c = Knobs::CENTERED;
        for seed in [7, 21] {
            for style in 0..DRUM_STYLES.len() as u8 {
                let changed = |a: &[(u64, u16, u8)], b: &[(u64, u16, u8)]| b.iter().filter(|e| !a.contains(e)).count() as f32 / b.len() as f32;
                let straight = bars(style, seed, Knobs { swing: 0.0, ..c });
                let swung = bars(style, seed, Knobs { swing: 1.0, ..c });
                // Every off 16th swings, breaks and ghosts included.
                assert!(changed(&straight, &swung) > 0.2, "seed {seed} style {style}: swing moves {}", changed(&straight, &swung));
                let calm = bars(style, seed, Knobs { morph: 0.0, ..c });
                let chopped = bars(style, seed, Knobs { morph: 1.0, ..c });
                assert!(changed(&calm, &chopped) > 0.15, "seed {seed} style {style}: morph changes {}", changed(&calm, &chopped));
            }
        }
    }
}

