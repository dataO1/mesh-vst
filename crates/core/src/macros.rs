//! The meta knobs (design D37) and what they move.
//!
//! A knob is a position 0..1 around the seed's own suggestion: 0.5 plays the
//! value the sound seed and pattern seed suggest ([`Centers`]), 0 and 1 reach
//! the ends of the range. The mappings follow Mesh live's lane controls.

use crate::event::LiveEvent;
use crate::pattern::{BarParams, PatternLane};
use crate::role::Role;
use crate::rng;
use crate::sampler::{CcMap, LaneSamplerParams, ModConfig, ModEnv, ModLfo};
use crate::tempo::note;

/// Knob positions, 0..1, 0.5 = the seed's suggestion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    pub density: f32,
    /// Rearranges the bar: beat chunks swap places (0 = as generated).
    pub permute: f32,
    pub variation: f32,
    /// The Groove knob: the drums' swing and feel, the bass's syncopation.
    pub swing: f32,
    pub shape: f32,
    pub morph: f32,
    /// Bass only.
    pub spread: f32,
    /// Bass only.
    pub slide: f32,
    /// Drums only: the kit's register, low voices .. high voices.
    pub tone: f32,
    /// Clip, saturation and bitcrush on the lane's output.
    pub heat: f32,
}

/// Number of knobs in [`Knobs::as_array`].
pub const KNOBS: usize = 10;

impl Default for Knobs {
    fn default() -> Self {
        Self::CENTERED
    }
}

impl Knobs {
    /// Every knob at the seed's suggestion, Permute off.
    pub const CENTERED: Self = Self {
        density: 0.5,
        permute: 0.0,
        variation: 0.5,
        swing: 0.5,
        shape: 0.5,
        morph: 0.5,
        spread: 0.5,
        slide: 0.5,
        tone: 0.5,
        heat: 0.5,
    };

    pub fn as_array(&self) -> [f32; KNOBS] {
        [self.density, self.permute, self.variation, self.swing, self.shape, self.morph, self.spread, self.slide, self.tone, self.heat]
    }

    fn from_array(a: [f32; KNOBS]) -> Self {
        Self {
            density: a[0],
            permute: a[1],
            variation: a[2],
            swing: a[3],
            shape: a[4],
            morph: a[5],
            spread: a[6],
            slide: a[7],
            tone: a[8],
            heat: a[9],
        }
    }
}

/// Where each knob's middle sits for a seed, as a value 0..1.
pub type Centers = Knobs;

// MUSICAL: first draft, tune by ear. The range each knob's seeded centre
// falls in, per mode (from the Mesh starter style's defaults and their
// usual playing range).
const DRUM_CENTRES: [(f32, f32); KNOBS] =
    [(0.35, 0.65), (0.5, 0.5), (0.25, 0.55), (0.0, 0.35), (0.35, 0.7), (0.1, 0.7), (0.5, 0.5), (0.5, 0.5), (0.3, 0.7), (0.5, 0.5)];
const BASS_CENTRES: [(f32, f32); KNOBS] =
    [(0.35, 0.65), (0.5, 0.5), (0.3, 0.6), (0.2, 0.6), (0.25, 0.75), (0.1, 0.9), (0.05, 0.4), (0.3, 0.7), (0.5, 0.5), (0.5, 0.5)];
const S_CENTRE: u32 = 0xC40;

impl Centers {
    /// The seed's suggestion per knob. The pattern seed places the groove
    /// knobs (density, variation, groove, spread, slide), the sound seed the
    /// sound knobs (shape, morph, tone, heat). Permute has no seeded centre:
    /// the knob is the amount.
    pub fn of(role: Role, pattern_seed: u32, sound_seed: u32) -> Self {
        let table = if role == Role::Drums { &DRUM_CENTRES } else { &BASS_CENTRES };
        let sound = [false, false, false, false, true, true, false, false, true, true];
        let mut out = [0.5; KNOBS];
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

/// The knob position that gives `value` around `centre`: [`around`] inverted.
pub fn knob_of(value: f32, centre: f32) -> f32 {
    let v = value.clamp(0.0, 1.0);
    let c = centre.clamp(0.0, 1.0);
    if v <= c {
        if c > 0.0 { 0.5 * v / c } else { 0.5 }
    } else if c < 1.0 {
        0.5 + 0.5 * (v - c) / (1.0 - c)
    } else {
        0.5
    }
}

/// Where the knobs sat for one hit: the values its bar and note played with
/// (the letter, filler and per-note moves on top of the knobs), as knob
/// positions. Knobs nothing moves keep their position.
pub fn hit_knobs(role: Role, centres: &Centers, knobs: &Knobs, names: &[&str], bar: &BarParams, ev: &LiveEvent) -> Knobs {
    let index = |name: &str| names.iter().position(|n| *n == name);
    let per_bar = |name: &str, own: f32, c: f32| index(name).map_or(own, |i| knob_of(bar.values[i], c));
    let at_step = crate::generator::blocks::NoteShape::at(ev.step);
    let per_note = |name: &str, own: f32, c: f32| index(name).map_or(own, |i| knob_of(bar.at(i, at_step), c));
    let c = centres;
    match role {
        Role::Drums => Knobs {
            density: per_bar("density", knobs.density, c.density),
            variation: per_bar("variation", knobs.variation, c.variation),
            swing: knob_of(bar.swing, c.swing),
            morph: per_bar("chop", knobs.morph, c.morph),
            permute: bar.permute,
            ..*knobs
        },
        // The bass generator reads density, spread and slide per note.
        _ => Knobs {
            density: per_note("density", knobs.density, c.density),
            variation: per_bar("variation", knobs.variation, c.variation),
            swing: per_bar("syncopation", knobs.swing, c.swing),
            spread: per_note("spread", knobs.spread, c.spread),
            // A slide shows the Slide its glide came from; other notes the
            // Slide their shape reads.
            slide: match index("slide") {
                Some(_) if ev.has(crate::event::flags::TIE) => knob_of(crate::generator::bass::slide_of_glide(ev.glide), c.slide),
                Some(i) => knob_of(bar.at(i, bar.shape_of(ev)), c.slide),
                None => knobs.slide,
            },
            morph: knob_of(around(knobs.morph, c.morph) + bar.morph + bar.morph_move(ev), c.morph),
            permute: bar.permute,
            // Shape: the note's Length (step and height), back on the knob.
            shape: match index("length") {
                Some(i) => {
                    let shape = crate::generator::blocks::NoteShape { gate: None, ..bar.shape_of(ev) };
                    let length = crate::generator::bass::held_length(bar.values[i], bar.at(i, shape));
                    knob_of(length_knob(length), c.shape)
                }
                None => knobs.shape,
            },
            ..*knobs
        },
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
    /// The form's seeds (D48); `None` plays one bar of A.
    pub form: Option<FormSettings>,
    /// Speed exponent: −1 half, 0, 1 double.
    pub rate: i8,
    /// Mode in the scale family.
    pub mode: u8,
    /// Drums: the style (index into [`DRUM_STYLES`]); `None` lets the pattern
    /// seed pick one.
    pub style: Option<u8>,
    /// Contrast 0..1: how far B moves from A, and C and D from them (D49).
    pub contrast: f32,
    /// Bass: unison limits and mode (D38).
    pub unison: crate::sampler::Unison,
    /// Bass: play from the clips (D50), else the generated line.
    pub clips: bool,
    /// The Squash knob, which the bass's loudness is calibrated through.
    pub squash: f32,
}

/// The seeds a generated form is built from (D48).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormSettings {
    pub phrase_seed: u32,
    pub fill_seed: u32,
    pub second_seed: u32,
    /// Phrases (A..D) pinned to the phrase seed they had: they stay over
    /// phrase rolls.
    pub pins: [Option<u32>; 4],
}

impl FormSettings {
    /// All three seeds `seed`, no pins.
    pub fn seeded(seed: u32) -> Self {
        Self { phrase_seed: seed, fill_seed: seed, second_seed: seed, pins: [None; 4] }
    }

    /// The phrase seed letter `l` plays: its pin, else the phrase seed's.
    pub fn phrase_of(&self, l: crate::form::Letter) -> u32 {
        self.pins[l.index() as usize].unwrap_or_else(|| crate::arrange::phrase_seed(self.phrase_seed, l))
    }

    /// The form spec for a lane at knob values `variation`, `shape` and
    /// `contrast`.
    pub fn spec(&self, drums: bool, variation: f32, shape: f32, contrast: f32) -> crate::arrange::FormSpec {
        use crate::arrange::{band, FormSeeds, FormSpec};
        let phrases = crate::form::Letter::ALL.map(|l| self.phrase_of(l));
        FormSpec {
            seeds: FormSeeds { phrases, fill: self.fill_seed, second: self.second_seed },
            variation: band(variation),
            shape: band(shape),
            contrast: band(contrast),
            drums,
        }
    }
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
    // The form, built for the Variation, Shape, Contrast, Density and
    // Groove bands (D48, D51).
    match s.form {
        Some(f) => lane.set_form_spec(f.spec(role == Role::Drums, resolved.variation, resolved.shape, s.contrast)),
        None => lane.clear_form(),
    }
    // Accent strength: the sound seed's, scaled by Variation.
    let seeded = rng::range_f32(rng::hash(s.sound_seed, 0, 0, 0, S_ACCENT), 0.3, 0.7);
    let accent = (seeded * (0.5 + resolved.variation)).clamp(0.0, 1.0);
    lane.permute = resolved.permute;
    lane.contrast = s.contrast.clamp(0.0, 1.0);
    apply(role, &resolved, accent, lane, &mut sp);
    if role == Role::Drums {
        lane.set_param("style", style_value(s.style.unwrap_or_else(|| seeded_style(s.pattern_seed))));
    } else {
        sp.unison = s.unison;
        lane.set_param("source", if s.clips { 1.0 } else { 0.0 });
        // Variation widens how far each note draws its detune and width.
        sp.unison.range = lerp(UNISON_RANGE.0, UNISON_RANGE.1, resolved.variation);
        seeded_voice(&mut sp, s.sound_seed);
        width_and_liquid(&mut sp, resolved.spread, resolved.slide);
    }
    sp
}

const S_VOICE: u32 = 0xC50;
// MUSICAL: first draft, tune by ear.
/// How far notes draw their unison detune and width at Variation 0 and 1.
const UNISON_RANGE: (f32, f32) = (0.1, 0.8);
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
    // A slow drift .. a fast wobble, light .. deep: half .. eight times the
    // designed rate, on the beat grid.
    m.lfo.rate = 2f32.powf(r(6, -1.5, 3.0).round());
    m.lfo.to_cutoff = r(7, 0.0, 2800.0);
    p.decay *= 2f32.powf(r(8, -0.8, 0.8));
    // The release only a little either way, and never long (user,
    // 2026-10-20: it reached 1 to 2 s).
    p.release = (p.release * 2f32.powf(r(9, -RELEASE_SPREAD, RELEASE_SPREAD))).min(RELEASE_MAX);
}

// MUSICAL: tune by ear (user, 2026-10-20). The seeded release's spread in
// octaves and the longest release (a quarter note, 2026-10-24).
const RELEASE_SPREAD: f32 = 0.3;
const RELEASE_MAX: f32 = note(1.0 / 4.0);

// MUSICAL: first draft, tune by ear (user: Spread and Slide change the
// sound, not only the line).
/// Spread is width: the filter LFO and envelope open up with it (the notes
/// spread out in the stereo field in `pattern`). Slide is liquid: the
/// filter glides between notes and held notes take a vibrato (more notes
/// tie in `shape_notes`). Both 0.5 = the voice as rolled.
fn width_and_liquid(p: &mut LaneSamplerParams, spread: f32, slide: f32) {
    let m = &mut p.modulation;
    m.lfo.to_cutoff *= lerp(0.3, 1.8, spread) / lerp(0.3, 1.8, 0.5);
    m.env.to_cutoff *= lerp(0.7, 1.4, spread) / lerp(0.7, 1.4, 0.5);
    p.cutoff_glide = lerp(note(1.0 / 256.0), note(1.0 / 8.0), slide);
    m.vib_depth = 30.0 * ((slide - 0.4) / 0.6).max(0.0).powf(1.5);
    m.vib_delay = note(1.0 / 8.0);
    m.vib_fade = note(1.0 / 4.0);
}

// MUSICAL: first draft, tune by ear (user: the ring mod comes in gradually,
// Morph changes its shape, at most 30 %).
/// Morph value where the drums' ring modulator starts, and the range its
/// seeded carrier falls in (Hz), moved up to an octave by Morph.
const RING_FROM: f32 = 0.2;
const RING_HZ: (f32, f32) = (300.0, 1100.0);
const S_RING: u32 = 0xC52;

/// The drums' ring modulator for a sound seed at resolved Morph `morph`:
/// carrier Hz, mix (up to [`crate::heat::RING_MIX_MAX`]) and carrier shape
/// (sine .. square).
pub fn ring(sound_seed: u32, morph: f32) -> (f32, f32, f32) {
    let m = ((morph - RING_FROM) / (1.0 - RING_FROM)).clamp(0.0, 1.0);
    let base = rng::range_f32(rng::hash(sound_seed, 0, 0, 0, S_RING), RING_HZ.0, RING_HZ.1);
    let mix = crate::heat::RING_MIX_MAX * m * m * (3.0 - 2.0 * m);
    (base * 2f32.powf(m), mix, morph.clamp(0.0, 1.0))
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
    // Groove: the lane's own times straight, dotted or in triplets.
    sampler.feel = crate::tempo::feel(v.swing);
}

// MUSICAL: first draft of the drum macros, tune by ear.
fn apply_drums(v: &Knobs, accent: f32, lane: &mut PatternLane, p: &mut LaneSamplerParams) {
    lane.set_param("density", v.density);
    lane.set_param("variation", v.variation);
    // Groove: the swing, each voice's push and pull, the accent shape
    // (`pattern::groove`) and more ghosts.
    lane.set_param("swing", v.swing);
    // Tone: the kit's register. Low: kicks, snares and their ghosts, the
    // mid percussion, few cymbals. High: hats, rides, shakers and the top
    // loop, sparse kicks and snares.
    let t = v.tone;
    lane.set_param("overlay_ghost", (lerp(0.1, 0.5, v.variation) + 0.3 * v.swing + 0.2 * (1.0 - t)).min(1.0));
    lane.set_param("tone", t);
    lane.set_param("overlay_hats", lerp(0.1, 0.9, t));
    lane.set_param("overlay_ride", lerp(0.05, 0.95, t));
    lane.set_param("overlay_shaker", lerp(0.1, 0.9, t));
    lane.set_param("overlay_top", lerp(0.15, 1.0, t));
    lane.set_param("overlay_perc", lerp(0.25, 0.45, t));
    // Morph: straight .. chopped, inside the style the Style setting picks.
    lane.set_param("chop", v.morph);
    lane.set_param("mutate", (0.6 * v.variation + 0.4 * v.morph).min(1.0));
    // Accent: wider dynamics and harder accents.
    p.velocity_curve = lerp(1.0, 2.2, accent);
    p.accent_gain = lerp(1.1, 2.0, accent);
    // Shape: tight and dry .. the hits whole. It shortens every voice but
    // lengthens none: the release tails above its middle do that, per voice
    // (`tail::VoiceTails`).
    p.sustain = lerp(0.35, 1.0, v.shape);
    p.decay = lerp(note(3.0 / 32.0), note(1.0 / 2.0), v.shape);
    p.release = lerp(note(1.0 / 64.0), note(3.0 / 32.0), v.shape);
    // The hits start at once, whatever Shape: the transient matters most
    // (user, 2026-10-20).
    p.attack = 0.0;
    // The choke amount follows Shape: tight cuts, open lets the hats ring
    // (D27). A retrigger's fade is the hits' audible release.
    p.choke_amount = lerp(1.0, DRUM_CHOKE_OPEN, v.shape);
}

// MUSICAL: tune by ear (user, 2026-10-20: full Shape too short). The choke
// amount at full Shape (0.05 = a fade of about 400 ms, it was 0.2, about
// 230 ms).
const DRUM_CHOKE_OPEN: f32 = 0.05;

// MUSICAL: tune by ear (user, 2026-10-20). Where the bass's Shape starts
// (its envelope, expression and stab share).
const SHAPE_FLOOR: f32 = 0.3;

// MUSICAL: tune by ear (user, 2026-10-21: at low Shape and high Density the
// hits still sounded held, not short or subdivided). The bass's note Length
// at Shape 0, its middle and full: short stabs .. as written .. held
// (user, 2026-10-26: 0 too short, where 40 % was is right: 0.43).
const SHAPE_LENGTH: (f32, f32, f32) = (0.43, 0.5, 1.0);

/// The bass's note Length at Shape knob `k`.
fn shape_length(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    if k < 0.5 { lerp(SHAPE_LENGTH.0, SHAPE_LENGTH.1, 2.0 * k) } else { lerp(SHAPE_LENGTH.1, SHAPE_LENGTH.2, 2.0 * k - 1.0) }
}

/// The Shape knob a note Length shows at (the inverse of [`shape_length`]).
pub(crate) fn length_knob(l: f32) -> f32 {
    if l < SHAPE_LENGTH.1 { 0.5 * (l - SHAPE_LENGTH.0) / (SHAPE_LENGTH.1 - SHAPE_LENGTH.0) } else { 0.5 + 0.5 * (l - SHAPE_LENGTH.1) / (SHAPE_LENGTH.2 - SHAPE_LENGTH.1) }
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
    // Swing: how far the line leaves the beat (syncopation).
    lane.set_param("syncopation", v.swing);
    lane.set_param("slide", v.slide);
    // Shape: stabby, accented and moving .. sustained and calm. Its bottom
    // starts where 30 % used to (user: longer stabs, fewer, some held notes).
    let shape = lerp(SHAPE_FLOOR, 1.0, v.shape);
    lane.set_param("length", shape_length(v.shape));
    p.pitch_env_time = crate::generator::bass::pitch_env_time(shape_length(v.shape));
    lane.set_param("expression", lerp(1.0, 0.15, shape));
    lane.set_param("stabs", lerp(1.0, 0.0, shape));
    character(p, shape);
    // The stabs' decay: twice the designed one at Shape 0, as designed at
    // the top (user, 2026-10-26: at the lowest Shape the notes faded away).
    p.decay *= lerp(STAB_DECAY_STRETCH, 1.0, v.shape);
    p.release = shape_release(p, v.shape);
    // Accent also deepens the filter sweep and the stab punch; its level
    // stays near the others (the dynamics live in the velocity window).
    p.accent_gain = BASS_ACCENT_GAIN;
    p.modulation.env.to_cutoff = lerp(2400.0, 6000.0, accent);
    p.punch *= lerp(0.6, 1.4, accent);
    p.held_punch = HELD_PUNCH * lerp(0.6, 1.4, accent);
    p.release_curve = RELEASE_CURVE;
    p.attack_curve = ATTACK_CURVE;
    p.morph = v.morph;
}

// MUSICAL: Mesh live's Character macro (user, 2026-10-20: the attack of
// long notes far louder than the rest; about −1 dB sustain, a longer
// decay, the release up to a quarter at full Shape).
/// The bass's accent gain (about +0.5 dB; user, 2026-10-21: the loudness
/// between notes differs far too much).
const BASS_ACCENT_GAIN: f32 = 1.06;
/// Sustain level of held notes (−1 dB).
const HELD_SUSTAIN: f32 = 0.89;
/// Release at full Shape (from the designed one at the bottom; user,
/// 2026-10-24: a quarter faded away too long).
const HELD_RELEASE: f32 = note(3.0 / 32.0);
/// The release's curve (`release_curve`; user, 2026-10-24: the tail
/// floated away): the level holds and falls fast at the end, −1 dB at half
/// the release, −6 dB at 80 %.
const RELEASE_CURVE: f32 = 3.0;
/// The attack's curve (`attack_curve`; user, 2026-10-24: more aggressive):
/// −6 dB at a fifth of the attack, −1.2 dB at half (the quarter sine was at
/// −10 and −3 dB).
const ATTACK_CURVE: f32 = 3.0;
/// Accent of held notes (user, 2026-10-24: a bit more punch): about 8 dB of
/// the stab drive through the note and a small kick at its start.
const HELD_PUNCH: f32 = 0.4;
/// How much longer the decay is at Shape 0 than at the top (stabs decay
/// through it, sustain 0).
const STAB_DECAY_STRETCH: f32 = 2.0;
/// Release at Shape 0 (user, 2026-10-21: at low Shape and full Density the
/// release rang over every 16th, the line sounded held).
const STAB_RELEASE: f32 = note(1.0 / 64.0);
/// Stab accent at the sustained end of the macro (full at the stabby end).
const STAB_PUNCH: f32 = 0.4;

/// The bass release at Shape knob `k`: [`STAB_RELEASE`] at 0, the designed
/// one at the middle, [`HELD_RELEASE`] at full.
fn shape_release(p: &LaneSamplerParams, k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    let designed = p.designed_release;
    if k < 0.5 { lerp(STAB_RELEASE.min(designed), designed, 2.0 * k) } else { lerp(designed, HELD_RELEASE.max(designed), 2.0 * k - 1.0) }
}

/// Mesh's Character macro: the share of stabs and held notes, the filter
/// bite and the stab accent.
fn character(p: &mut LaneSamplerParams, v: f32) {
    let (decay, sustain) = p.designed_env;
    let stab = (1.0 - 2.0 * v).max(0.0);
    p.decay = decay;
    p.sustain = sustain.max(HELD_SUSTAIN);
    p.release = p.designed_release + (HELD_RELEASE.max(p.designed_release) - p.designed_release) * v;
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
        attack: note(1.0 / 512.0),
        decay: note(3.0 / 4.0),
        sustain: 0.89,
        release: note(1.0 / 16.0),
        cutoff_glide: note(3.0 / 128.0),
        ..LaneSamplerParams::default()
    };
    let mut m = ModConfig {
        env: ModEnv { decay: note(1.0 / 4.0), sustain: 0.0, release: note(1.0 / 8.0), to_pitch: 200.0, to_cutoff: 4800.0, ..ModEnv::default() },
        // One cycle per quarter note.
        lfo: ModLfo { rate: 1.0 / note(1.0 / 4.0), to_cutoff: 600.0, ..ModLfo::default() },
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
            let s = LaneSettings { knobs: Knobs::CENTERED, sound_seed: 5, pattern_seed: 5, form: None, rate: 0, mode: 0, style: Some(style), contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
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
    fn full_shape_sustains_everything() {
        use crate::event::{flags, EventKind};
        let (mut held, mut notes) = (0, 0);
        for seed in 1..=40 {
            let mut lane = PatternLane::bass(seed, seed);
            // At Contrast 0: above it B mirrors the Shape into stabs (D62).
            let s = LaneSettings { knobs: Knobs { shape: 1.0, ..Knobs::CENTERED }, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.0, unison: Default::default(), clips: false, squash: 0.0 };
            configure(&mut lane, mudpie_params(), &s);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            for b in 0..8 {
                lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                let ev: Vec<_> = out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).copied().collect();
                assert!(ev.iter().all(|e| e.flags & flags::STAB == 0), "no stabs at full Shape");
                for w in ev.windows(2) {
                    notes += 1;
                    held += (w[0].gate + 4 >= w[1].tick - w[0].tick) as u32;
                }
            }
        }
        // (The form's operators shorten some on purpose: a question ends
        // each repeat of an asking phrase, mutations, double time.)
        assert!(held as f32 >= 0.7 * notes as f32, "{held} of {notes} held to the next note");
    }

    #[test]
    fn the_sound_seed_rolls_the_mudpie_voice() {
        let voice = |seed| {
            let mut lane = PatternLane::bass(5, seed);
            let s = LaneSettings { knobs: Knobs::CENTERED, sound_seed: seed, pattern_seed: 5, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
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

    #[test]
    fn knob_of_inverts_around() {
        for c in [0.0, 0.2, 0.5, 0.9] {
            for k in [0.0, 0.1, 0.4, 0.5, 0.7, 1.0] {
                assert!((knob_of(around(k, c), c) - k).abs() < 1e-4 || c == 0.0 && k <= 0.5, "c {c} k {k}");
            }
        }
    }

    #[test]
    fn hits_show_the_letter_and_note_moves_around_the_knobs() {
        use crate::event::EventKind;
        let knobs = Knobs { density: 0.6, variation: 0.4, spread: 0.5, slide: 0.5, ..Knobs::CENTERED };
        let (mut on_knob, mut moved, mut varied_bars) = (0, 0, 0);
        for seed in 1..=10 {
            let mut lane = PatternLane::bass(seed, seed);
            let s = LaneSettings { knobs, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
            configure(&mut lane, mudpie_params(), &s);
            let centres = Centers::of(Role::Bass, seed, seed);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            for b in 0..16 {
                lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                let notes: Vec<_> = out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).copied().collect();
                let hits: Vec<Knobs> = notes.iter().map(|e| hit_knobs(Role::Bass, &centres, &knobs, lane.param_names(), &out.params, e)).collect();
                // The pattern knobs do not move per note: one value a bar.
                for w in hits.windows(2) {
                    assert!((w[0].density - w[1].density).abs() < 1e-5 && (w[0].variation - w[1].variation).abs() < 1e-5, "{w:?}");
                }
                for h in &hits {
                    assert!(h.as_array().iter().all(|v| (0.0..=1.0).contains(v)), "{h:?}");
                    // Permute at 0 stays off; Shape moves much per note, Morph per block.
                    assert_eq!(h.permute, knobs.permute);
                    // Variation plays as set (user, 2026-10-21); Density
                    // moves with the letters.
                    assert!((h.variation - knobs.variation).abs() < 1e-3, "{h:?}");
                    if (h.density - knobs.density).abs() < 1e-3 {
                        on_knob += 1;
                    } else if (h.density - knobs.density).abs() > 0.02 {
                        moved += 1;
                    }
                }
                // The right-hand knobs do.
                varied_bars += hits.windows(2).any(|w| (w[0].spread - w[1].spread).abs() > 1e-3) as u32;
            }
        }
        assert!(on_knob > 0 && moved > 0, "A bars sit on the knob ({on_knob}), the other letters move ({moved})");
        assert!(varied_bars > 80, "{varied_bars} of 160 bars vary per note");
    }

    #[test]
    fn the_bass_shape_runs_from_stabs_through_as_written_to_held() {
        assert!((shape_length(0.0) - SHAPE_LENGTH.0).abs() < 1e-6 && (shape_length(0.5) - 0.5).abs() < 1e-6 && (shape_length(1.0) - 1.0).abs() < 1e-6);
        for k in [0.0, 0.2, 0.5, 0.8, 1.0] {
            assert!((length_knob(shape_length(k)) - k).abs() < 1e-5, "{k}");
        }
    }

    #[test]
    fn the_stabs_decay_twice_as_long_at_the_lowest_shape() {
        let decay = |shape: f32| {
            let mut lane = PatternLane::bass(4, 4);
            let mut p = mudpie_params();
            seeded_voice(&mut p, 4);
            apply_bass(&Knobs { shape, ..Knobs::CENTERED }, 0.5, &mut lane, &mut p);
            p.decay
        };
        assert!((decay(0.0) / decay(1.0) - STAB_DECAY_STRETCH).abs() < 1e-3, "{} vs {}", decay(0.0), decay(1.0));
    }

    #[test]
    fn shape_moves_little_per_note_at_low_variation_and_much_at_full() {
        use crate::event::EventKind;
        // At Contrast 0, so the letters' own Shapes (B mirrors it, D62) stay
        // out of it: what Variation moves per note.
        let widest = |variation: f32| {
            // In Shape's upper half: its lower half spans only 0.43 .. 0.5 of
            // the Length (D62), so a note's small move there reads as a big
            // one on the knob.
            let knobs = Knobs { variation, shape: 0.75, ..Knobs::CENTERED };
            let mut widest = 0.0f32;
            for seed in 1..=10 {
                let mut lane = PatternLane::bass(seed, seed);
                let s = LaneSettings { knobs, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.0, unison: Default::default(), clips: false, squash: 0.0 };
                configure(&mut lane, mudpie_params(), &s);
                let centres = Centers::of(Role::Bass, seed, seed);
                let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
                for b in 0..8 {
                    lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                    let shapes: Vec<f32> = out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| hit_knobs(Role::Bass, &centres, &knobs, lane.param_names(), &out.params, e).shape).collect();
                    let (lo, hi) = shapes.iter().fold((1.0f32, 0.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
                    widest = widest.max(hi - lo);
                }
            }
            widest
        };
        // User, 2026-10-21: at low Variation Shape still moved far too much.
        let (low, half, full) = (widest(0.0), widest(0.5), widest(1.0));
        assert!(low < 0.12, "Shape spans {low} of the knob within a bar at Variation 0");
        assert!(half < full * 0.6, "small up to about half Variation: {half} vs {full}");
        assert!(full > 0.3, "Shape spans {full} of the knob within a bar at Variation 1");
    }

    #[test]
    fn variation_widens_the_sound_knobs_moves_only() {
        use crate::event::EventKind;
        let spread = |variation: f32| {
            let knobs = Knobs { density: 0.5, variation, ..Knobs::CENTERED };
            let (mut density, mut spread, mut n) = (0.0, 0.0, 0);
            for seed in 1..=10 {
                let mut lane = PatternLane::bass(seed, seed);
                let s = LaneSettings { knobs, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
                configure(&mut lane, mudpie_params(), &s);
                let centres = Centers::of(Role::Bass, seed, seed);
                let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
                for b in 0..16 {
                    lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                    for e in out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn) {
                        let h = hit_knobs(Role::Bass, &centres, &knobs, lane.param_names(), &out.params, e);
                        density += (h.density - knobs.density).abs();
                        spread += (h.spread - knobs.spread).abs();
                        n += 1;
                    }
                }
            }
            (density / n as f32, spread / n as f32)
        };
        let (low, high) = (spread(0.0), spread(1.0));
        // (D61: 1.96 on the cycle forms' notes.)
        assert!(high.1 > 1.8 * low.1, "Spread moves {} at Variation 0, {} at 1", low.1, high.1);
        // The pattern knobs move as the form says, whatever Variation.
        assert!(high.0 < 1.5 * low.0 && low.0 < 1.5 * high.0, "Density moves {} at Variation 0, {} at 1", low.0, high.0);
    }

    /// Eight bars of drums at style `style` with `knobs`, as (bar, tick, target).
    fn bars(style: u8, seed: u32, knobs: Knobs) -> Vec<(u64, u16, u8)> {
        let mut lane = PatternLane::drums(DrumLayout::standard(), seed, seed);
        let s = LaneSettings { knobs, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: Some(style), contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
        configure(&mut lane, LaneSamplerParams::default(), &s);
        let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
        (0..8u64)
            .flat_map(|b| {
                lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                out.events.as_slice().iter().map(|e| (b, e.tick, e.target)).collect::<Vec<_>>()
            })
            .collect()
    }

    /// Hits per drum voice over eight bars of seeds 1..=10 with `knobs`.
    fn voice_hits(knobs: Knobs) -> [u32; 8] {
        use crate::generator::layout::DrumVoice;
        let mut n = [0u32; 8];
        for seed in 1..=10 {
            let mut lane = PatternLane::drums(DrumLayout::standard(), seed, seed);
            let s = LaneSettings { knobs, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
            configure(&mut lane, LaneSamplerParams::default(), &s);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            for b in 0..8 {
                lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                for e in out.events.as_slice().iter().filter(|e| e.vel > 0) {
                    if let Some(v) = DrumVoice::of_target(e.target).filter(|v| out.voices & v.bit() != 0) {
                        n[v as usize] += 1;
                    }
                }
            }
        }
        n
    }

    #[test]
    fn tone_moves_the_kit_from_kicks_and_snares_to_cymbals() {
        let share = |n: [u32; 8]| {
            let low = (n[0] + n[1]) as f32;
            let high = (n[2] + n[3] + n[4] + n[6]) as f32;
            low / (low + high)
        };
        let (low, high) = (share(voice_hits(Knobs { tone: 0.0, ..Knobs::CENTERED })), share(voice_hits(Knobs { tone: 1.0, ..Knobs::CENTERED })));
        assert!(low > high + 0.25, "kick and snare share {low} at Tone 0, {high} at 1");
    }

    #[test]
    fn groove_lays_the_snare_back_and_shapes_the_hats() {
        use crate::event::EventKind;
        use crate::generator::layout::DrumVoice;
        let render = |groove: f32| {
            let mut lane = PatternLane::drums(DrumLayout::standard(), 4, 4);
            let s = LaneSettings { knobs: Knobs { swing: groove, ..Knobs::CENTERED }, sound_seed: 4, pattern_seed: 4, form: None, rate: 0, mode: 0, style: Some(1), contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
            configure(&mut lane, LaneSamplerParams::default(), &s);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            lane.render(0, MusicalKey::new(5, true), &mut out, &mut scratch);
            out.events.as_slice().iter().filter(|e| e.kind == EventKind::Trigger && e.vel > 0).copied().collect::<Vec<_>>()
        };
        let (straight, grooved) = (render(0.0), render(1.0));
        let backbeat = |ev: &[LiveEvent]| ev.iter().find(|e| DrumVoice::of_target(e.target) == Some(DrumVoice::Snare) && e.step == 4).map(|e| e.tick);
        assert!(backbeat(&grooved) > backbeat(&straight), "the snare lays back");
        let hat_vel = |ev: &[LiveEvent], off: bool| {
            let v: Vec<f32> = ev.iter().filter(|e| DrumVoice::of_target(e.target) == Some(DrumVoice::Hats) && (e.step % 2 == 1) == off).map(|e| e.vel as f32).collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        assert!(hat_vel(&grooved, false) - hat_vel(&grooved, true) > hat_vel(&straight, false) - hat_vel(&straight, true), "on 8ths stand out of the off 16ths");
    }

    #[test]
    fn spread_widens_the_upper_notes_and_slide_makes_the_line_liquid() {
        use crate::event::{flags, EventKind};
        let bars = |knobs: Knobs| {
            let mut lane = PatternLane::bass(9, 9);
            let s = LaneSettings { knobs, sound_seed: 9, pattern_seed: 9, form: None, rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
            let p = configure(&mut lane, mudpie_params(), &s);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            let mut ev = Vec::new();
            for b in 0..16 {

                lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                ev.extend(out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).copied());
            }
            (p, ev)
        };
        let home = crate::generator::bass::home_root(MusicalKey::new(5, true));
        let (_, narrow) = bars(Knobs { spread: 0.0, ..Knobs::CENTERED });
        let (_, wide) = bars(Knobs { spread: 1.0, ..Knobs::CENTERED });
        let width = |ev: &[LiveEvent]| ev.iter().map(|e| e.pan.unsigned_abs() as f32).sum::<f32>() / ev.len() as f32;
        assert!(width(&wide) > 3.0 * width(&narrow), "{} vs {}", width(&wide), width(&narrow));
        assert!(wide.iter().filter(|e| (e.key as i32) <= home).all(|e| e.pan == 0), "the sub stays centred");
        assert!(wide.iter().any(|e| e.pan.abs() > 20), "upper notes spread out");
        let (dry, tight) = bars(Knobs { slide: 0.0, ..Knobs::CENTERED });
        let (wet, liquid) = bars(Knobs { slide: 1.0, ..Knobs::CENTERED });
        assert_eq!(dry.modulation.vib_depth, 0.0);
        assert!(wet.modulation.vib_depth > 10.0 && wet.cutoff_glide > 4.0 * dry.cutoff_glide);
        let ties = |ev: &[LiveEvent]| ev.iter().filter(|e| e.flags & flags::TIE != 0).count() as f32 / ev.len() as f32;
        assert!(ties(&liquid) > ties(&tight), "{} vs {}", ties(&liquid), ties(&tight));
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

    #[test]
    fn the_form_follows_its_seeds_pins_and_knob_bands() {
        use crate::form::Letter;
        let configured = |form: Option<FormSettings>, knobs: Knobs| {
            let mut lane = PatternLane::bass(4, 4);
            let s = LaneSettings { knobs, sound_seed: 4, pattern_seed: 4, form, rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
            configure(&mut lane, mudpie_params(), &s);
            lane
        };
        let c = Knobs::CENTERED;
        let base = configured(Some(FormSettings::seeded(3)), c);
        let spec = base.form_spec().expect("a generated form");
        assert_eq!(base.form().bars(), 64);
        // A small knob move stays in its band: the same form.
        let nudged = configured(Some(FormSettings::seeded(3)), Knobs { shape: c.shape + 0.02, variation: c.variation - 0.02, ..c });
        assert_eq!(nudged.form_spec(), Some(spec));
        // Shape and Variation at the ends build for other bands.
        let long = configured(Some(FormSettings::seeded(3)), Knobs { shape: 1.0, ..c });
        assert!(long.form_spec().unwrap().shape > spec.shape);
        // A pinned phrase keeps its length and offset through a phrase roll.
        let pinned = FormSettings { pins: [Some(FormSettings::seeded(3).phrase_of(Letter::A)), None, None, None], ..FormSettings::seeded(3) };
        let rolled = configured(Some(FormSettings { phrase_seed: 77, ..pinned }), c);
        let a_phrase = |l: &PatternLane| l.form().pattern().iter().find(|s| s.letter == Letter::A).map(|s| (s.phrase, s.offset));
        assert_eq!(rolled.form_spec().unwrap().seeds.phrases[0], spec.seeds.phrases[0]);
        assert_ne!(rolled.form_spec().unwrap().seeds.phrases[1], spec.seeds.phrases[1]);
        assert_eq!(a_phrase(&rolled), a_phrase(&base));
        // Contrast (D49) reaches the lane and the form's band.
        let mut lane = PatternLane::bass(4, 4);
        let s = LaneSettings { knobs: c, sound_seed: 4, pattern_seed: 4, form: Some(FormSettings::seeded(3)), rate: 0, mode: 0, style: None, contrast: 1.0, unison: Default::default(), clips: false, squash: 0.0 };
        configure(&mut lane, mudpie_params(), &s);
        assert_eq!(lane.contrast, 1.0);
        assert!(lane.form_spec().unwrap().contrast > spec.contrast);
        // No form settings: one bar of A.
        assert_eq!(configured(None, c).form().bars(), 1);
        // Building a new form while playing does not allocate.
        let mut lane = configured(Some(FormSettings::seeded(3)), c);
        let (_, allocs) = crate::test_support::count_allocs(|| {
            for seed in 10..30 {
                let s = LaneSettings { knobs: c, sound_seed: 4, pattern_seed: 4, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
                configure(&mut lane, mudpie_params(), &s);
            }
        });
        assert_eq!(allocs, 0);
    }

    /// Pitch agreement of consecutive 4-bar blocks of a bass render (music
    /// theory §6.3, as `melody.py` rep4): of the 16th steps both blocks
    /// start a note on, the share with the same key.
    fn rep4(lane: &PatternLane, bars: u64) -> f32 {
        use crate::event::EventKind;
        let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
        let mut blocks: Vec<std::collections::BTreeMap<u32, u8>> = Vec::new();
        for blk in 0..bars / 4 {
            let mut m = std::collections::BTreeMap::new();
            for b in 0..4u64 {
                lane.render(blk * 4 + b, MusicalKey::new(5, true), &mut out, &mut scratch);
                for e in out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn) {
                    m.insert(b as u32 * 16 + (e.tick / crate::TICKS_PER_STEP) as u32, e.key);
                }
            }
            blocks.push(m);
        }
        let agree = |a: &std::collections::BTreeMap<u32, u8>, b: &std::collections::BTreeMap<u32, u8>| {
            let both: Vec<u32> = a.keys().filter(|k| b.contains_key(k)).copied().collect();
            both.iter().filter(|k| a[k] == b[k]).count() as f32 / both.len().max(1) as f32
        };
        blocks.windows(2).map(|w| agree(&w[0], &w[1])).sum::<f32>() / (blocks.len() - 1) as f32
    }

    fn rep4_median(clips: bool) -> (f32, Vec<f32>) {
        let mut reps: Vec<f32> = (1..=30u32)
            .map(|seed| {
                let mut lane = PatternLane::bass(seed, seed);
                let s = LaneSettings { knobs: Knobs::CENTERED, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips, squash: 0.0 };
                configure(&mut lane, mudpie_params(), &s);
                rep4(&lane, 64)
            })
            .collect();
        reps.sort_by(f32::total_cmp);
        (reps[15], reps)
    }

    #[test]
    fn the_bass_source_setting_picks_clips_or_the_generator() {
        for clips in [true, false] {
            let mut lane = PatternLane::bass(3, 3);
            let s = LaneSettings { knobs: Knobs::CENTERED, sound_seed: 3, pattern_seed: 3, form: Some(FormSettings::seeded(3)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips, squash: 0.0 };
            configure(&mut lane, mudpie_params(), &s);
            let i = lane.param_names().iter().position(|n| *n == "source").unwrap();
            assert_eq!(lane.params[i], if clips { 1.0 } else { 0.0 });
        }
    }

    #[test]
    fn clip_forms_repeat_their_4_bar_blocks_like_the_references() {
        let (median, reps) = rep4_median(true);
        // As the generated forms: above the references' 0.63 (D61: the
        // cycles repeat, the slots vary them).
        assert!((0.33..=0.85).contains(&median), "median rep4 {median:.2} ({reps:?})");
    }

    #[test]
    fn generated_forms_repeat_their_4_bar_blocks_like_the_references() {
        let (_, mut reps) = rep4_median(false);
        reps.sort_by(f32::total_cmp);
        // The references repeat 0.33 .. 0.63 in pitch (music theory §6.3);
        // phrases that start on a note at an even beat (or the letter's
        // start) repeat a little more (user, 2026-10-21: less hectic), the
        // cycles more again (D61, user 2026-10-26: A and B repeat).
        assert!((0.33..=0.85).contains(&reps[15]), "median rep4 {:.2} ({reps:?})", reps[15]);
    }
}

#[cfg(test)]
mod density_tests {
    use super::*;
    use crate::event::EventKind;
    use crate::key::MusicalKey;
    use crate::pattern::{RenderedBar, Scratch};

    /// Share of bars where the line is silent for more than an 8th, over
    /// ten seeds' forms at Density 1 (a note counts as at least a 16th: a
    /// stab is a played note too).
    fn silent_bars(clips: bool, shape: f32) -> f32 {
        let bar = crate::TICKS_PER_BAR as usize;
        let knobs = Knobs { density: 1.0, shape, variation: 0.5, ..Knobs::CENTERED };
        let (mut silent, mut bars) = (0, 0);
        for seed in 1..=10u32 {
            let mut lane = PatternLane::bass(seed, seed);
            let s = LaneSettings { knobs, sound_seed: seed, pattern_seed: seed, form: Some(FormSettings::seeded(seed)), rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips, squash: 0.0 };
            configure(&mut lane, mudpie_params(), &s);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            for b in 0..64 {
                lane.render(b, MusicalKey::new(5, true), &mut out, &mut scratch);
                let mut sounding = vec![false; bar];
                for e in out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn && e.vel > 0) {
                    for k in e.tick as usize..(e.tick as usize + (e.gate as usize).max(bar / 16)).min(bar) {
                        sounding[k] = true;
                    }
                }
                // The longest silence, wrapping round the bar.
                let (mut gap, mut run) = (0, 0);
                for k in 0..2 * bar {
                    run = if sounding[k % bar] { 0 } else { run + 1 };
                    gap = gap.max(run.min(bar));
                }
                silent += (gap > bar / 6) as u32;
                bars += 1;
            }
        }
        silent as f32 / bars as f32
    }

    #[test]
    fn full_density_fills_almost_every_bar() {
        // User, 2026-10-21: at full Density the line still left rests
        // (merged pushed 8ths, the bar's start, the rhythm operators).
        for clips in [true, false] {
            for shape in [0.0, 0.5] {
                let s = silent_bars(clips, shape);
                assert!(s < 0.15, "clips {clips} shape {shape}: {s:.2} of the bars silent for over an 8th");
            }
        }
    }
}
