//! Neuro (design D53): the bass lane's resampling-style processing chain,
//! one amount from nothing (0) to a lot (1).
//!
//! The lane's output splits at [`SUB_HZ`]: the sub passes clean, the band
//! above runs through a chain of stages ([`ORDER`]) and is matched back to
//! its own loudness by the amount's make-up ([`crate::makeup`], D64), so the amount changes
//! the character, not the level. Each stage is one [`Stage`]; the chain is
//! built from [`ORDER`], so stages can be added, removed and reordered in
//! one place.
//!
//! **Motion** comes from the sound only, no LFO. Each note gets an ADSR
//! whose shape the seed picks ([`Adsr`]) and whose times are shares of the
//! note's own length (known at its onset), so it always plays out within
//! the note: stabs move fast, held notes slowly. Every filter band moves on
//! its own ([`Mover`]) with that ADSR, the note's draw (Variation) and
//! Morph; paired bands move against each other. An envelope follower on the
//! input sets how hard everything works: the louder parts of a note get more
//! processing, the quieter less. Contrast parts the letters: at 0 they move
//! alike, at 1 B moves the other way at another speed, with another ADSR,
//! its bands placed elsewhere ([`Voice`]).
//!
//! Every stage knows the note playing ([`Ctl`]): the emphasis and the comb
//! sit on the note's harmonics. The sound seed picks how much of each stage
//! the amount brings in (two of [`RACKS`], blended).
//!
//! Deterministic: everything follows the notes and the input. RT-safe:
//! buffers are allocated in [`Neuro::new`].

use std::f32::consts::{PI, TAU};

use crate::event::{flags, EventKind, LiveEvent};
use crate::dsp::{flush, mix, safe, Biquad, Butter4, Delay, Gap, Oversampler, Svf};
use crate::mbdist::{BandConfig, Curve, MbConfig, MbDist, Pass};
use crate::rng;
use crate::squash::{Lr4, Squash};
use crate::types::StereoSample;

/// Below this the sub passes untouched.
pub const SUB_HZ: f32 = 130.0;
/// Control rate: the stages update their coefficients every this many
/// frames (8: a fast sweep moves in steps of a sixth of a millisecond, too
/// fine to hear as stepping).
const CHUNK: usize = 8;
/// The stage amount at full Neuro for a rack weight of 1 (clamped to 1).
const AMOUNT_REACH: f32 = 1.4;
/// The amount's smoothing time.
const AMOUNT_SECS: f32 = 0.05;
/// The pitch tracking's glide without a slide.
const TRACK_SECS: f32 = 0.004;
/// Longest comb delay: down to this note frequency.
const COMB_LOWEST_HZ: f32 = 25.0;
/// The chorus's longest delay.
const CHORUS_MAX_SECS: f32 = 0.03;
/// Ticks per beat of the event gates.
const TICKS_PER_BEAT: f64 = crate::TICKS_PER_BAR as f64 / 4.0;

/// How things move for one control chunk.
#[derive(Debug, Clone, Copy)]
pub struct Motion {
    /// The note's ADSR, 0..1.
    pub adsr: f32,
    /// Seconds since the note started, and its length.
    pub age: f32,
    pub length: f32,
    /// The input's envelope against its own average, −1 (quiet) .. 1 (loud).
    pub level: f32,
    /// The note's own draw, −1..1 (scaled by Variation).
    pub note: f32,
    /// Morph: octaves the bands sit off their centre.
    pub morph: f32,
}

/// What every stage reads for one control chunk.
#[derive(Debug, Clone, Copy)]
pub struct Ctl {
    pub sr: f32,
    /// The note's frequency (glided).
    pub f0: f32,
    pub motion: Motion,
    /// Spread 0..1: the chorus width.
    pub spread: f32,
    /// The playing letter's voicing.
    pub voice: Voice,
    /// What is playing: its register and brightness.
    pub source: Source,
}

/// What the chain hears: how bass-like or lead-like the source is (its
/// sub's share), how much of it lies above 1 kHz (what turns harsh when
/// driven), and how far the fixed ranges move up with the register playing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Source {
    /// Octaves every fixed range moves up.
    pub shift: f32,
    /// 0 a source with little above 1 kHz .. 1 one with much.
    pub harsh: f32,
    /// 0 bass-like (a strong sub) .. 1 lead-like (none).
    pub lead: f32,
}

// MUSICAL: tune by ear (user, 2026-10-22: a lead in mudpie mode sits above
// the bass ranges; the energy above 1 kHz turns harsh when driven; moving
// the ranges up with a bright lead made it harsher). The register: a note
// above this plays higher ranges by this share of each octave, a bass-like
// source up to [`REGISTER_MAX_OCT`], a lead-like one up to
// [`LEAD_MAX_OCT`] (the moving ranges stay in the mids, the air does not
// move). A scanned file's centroid above 130 Hz, moved to the note, counts
// from [`CENTROID_REF_HZ`]. The sub's share (dB of all) from bass-like to
// lead-like; the share above 1 kHz (of the band above the sub) from mild
// to harsh; the followers' time while playing.
const REGISTER_REF_HZ: f32 = 65.0;
const REGISTER_SHARE: f32 = 0.8;
const REGISTER_MAX_OCT: f32 = 3.0;
const LEAD_MAX_OCT: f32 = 1.0;
const CENTROID_REF_HZ: f32 = 700.0;
const SUB_DB: (f32, f32) = (-3.0, -15.0);
const HARSH_RANGE: (f32, f32) = (0.08, 0.45);
const BRIGHT_SPLIT_HZ: f32 = 1000.0;
const BRIGHT_SECS: f32 = 1.5;
/// The followers' energy (mean square) from which what they heard counts
/// over a scan (about −60 dBFS).
const HEARD_ENERGY: f32 = 1e-6;
/// The drives: what a harsh source takes off them; a lead-like source's
/// share of the heavy symmetric drive and its extra on the subtle ones (O4).
const HARSH_DRIVE_CUT: f32 = 0.5;
const LEAD_HEAVY: f32 = 0.5;
const LEAD_SUBTLE: f32 = 0.5;
/// The top before the drive (O2): a high shelf at [`TAME_HZ`], up by
/// [`TAME_LIFT`] on a dark source (user, 2026-10-27: the drive reacted to the
/// lows only, the top came out with holes) down to
/// [`TAME_DB`] at a harsh source (a chord or a lead, so it does not fizz), half
/// of it given back after the deemphasis.
const TAME_HZ: f32 = 1200.0;
const TAME_DB: f32 = -12.0;
const TAME_LIFT: f32 = 8.0;
const TAME_BACK: f32 = 0.5;
// MUSICAL: tune by ear (2026-10-22: a chord as a mudpie came out mostly
// fizz, a lead got harsher at full Neuro). The balance guard (O3): the share
// of the processed band above [`GUARD_HZ`] may grow this many times over
// the input's (a bass-like source, a lead-like one) and is never held below
// [`GUARD_FLOOR`]; the share above [`GUARD_TOP_HZ`] stays under its cap
// (bass-like, lead-like); the followers' time; the most it takes off.
const GUARD_HZ: f32 = 1000.0;
const GUARD_GROWTH: (f32, f32) = (6.0, 2.0);
const GUARD_FLOOR: f32 = 0.08;
const GUARD_TOP_HZ: f32 = 3000.0;
const GUARD_TOP_CAP: (f32, f32) = (0.25, 0.15);
/// Above [`GUARD_TOP_HZ`] the share may also grow at most this many times
/// over the input's, never held under this share.
const GUARD_TOP_GROWTH: f32 = 10.0;
const GUARD_TOP_FLOOR: f32 = 0.06;
const GUARD_SECS: f32 = 0.05;
const GUARD_MIN_GAIN: f32 = 0.1;
const AIR_SHIFT_SHARE: f32 = 0.5;
const AIR_TOP_HZ: f32 = 14_000.0;

fn ramp(x: f32, (a, b): (f32, f32)) -> f32 {
    ((x - a) / (b - a)).clamp(0.0, 1.0)
}

impl Source {
    fn new(register: f32, sub_share: f32, over_1k: f32) -> Self {
        let lead = ramp(10.0 * sub_share.max(1e-6).log10(), SUB_DB);
        let max = REGISTER_MAX_OCT + (LEAD_MAX_OCT - REGISTER_MAX_OCT) * lead;
        Self { shift: (REGISTER_SHARE * register.max(0.0)).min(max), harsh: ramp(over_1k, HARSH_RANGE), lead }
    }

    /// The source of a scanned file (its `timbre`, at `root`) playing at
    /// `f0` Hz: its centroid moves with the note. `heard`: the sub's share
    /// and the share above 1 kHz as they sound now (the file played away
    /// from its root has another sub), else the scan's.
    pub fn scanned(t: &crate::library::Timbre, root: u8, f0: f32, heard: Option<(f32, f32)>) -> Self {
        let root_hz = 440.0 * ((root as f32 - 69.0) / 12.0).exp2();
        let centroid = t.centroid * f0.max(1.0) / root_hz;
        let (sub, over_1k) = heard.unwrap_or((t.sub, t.over_1k));
        Self::new((centroid / CENTROID_REF_HZ).log2(), sub, over_1k)
    }

    /// The source heard while playing (a file the library did not scan):
    /// the note at `f0` Hz, the sub's share of all, the share above 1 kHz
    /// of the band above the sub.
    pub fn of(f0: f32, sub_share: f32, over_1k: f32) -> Self {
        Self::new((f0.max(1.0) / REGISTER_REF_HZ).log2(), sub_share, over_1k)
    }

    /// The factor on every moving range.
    pub fn scale(&self) -> f32 {
        self.shift.exp2()
    }

    /// The factor on the drives' gain.
    pub fn drive(&self) -> f32 {
        1.0 - HARSH_DRIVE_CUT * self.harsh
    }

    /// The factor on a stage's amount by the source (O4: a lead gets less of
    /// the heavy drive, more of the subtle ones).
    pub fn stage(&self, kind: StageKind) -> f32 {
        match kind {
            StageKind::Drive => 1.0 - LEAD_HEAVY * self.lead,
            StageKind::Asym | StageKind::Shaper => 1.0 + LEAD_SUBTLE * self.lead,
            // The follower's lowpass is for bass sources only.
            StageKind::Filter => 1.0 - self.lead,
            _ => 1.0,
        }
    }
}

/// An ADSR's shape: attack, decay and release as shares of the note's
/// length, the sustain level and how far it slopes over the held part
/// (signed: the sustain keeps moving, not a flat hold).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adsr {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub slope: f32,
}

// MUSICAL: tune by ear (user, 2026-10-22: some seeds move quickly and
// release quickly, some move slowly with a fast attack and a very slow
// release, always within the note; 2026-10-23: the movement was not heard,
// a flat sustain held the bands still for most of a note). The seed's ADSR
// shapes, as shares of the note: snap, swell and long release, slow,
// pluck; each sustain slopes (the seed picks its sign).
// User, 2026-10-23: the attack sweeps too, audibly long by the shape (a
// long attack about half the sweep or more).
pub const ADSR_SHAPES: [Adsr; 4] = [
    Adsr { attack: 0.15, decay: 0.3, sustain: 0.35, release: 0.2, slope: 0.5 },
    Adsr { attack: 0.5, decay: 0.25, sustain: 0.7, release: 0.25, slope: -0.4 },
    Adsr { attack: 0.6, decay: 0.2, sustain: 0.6, release: 0.2, slope: 0.3 },
    Adsr { attack: 0.08, decay: 0.45, sustain: 0.2, release: 0.3, slope: 0.6 },
];

impl Adsr {
    fn lerp(self, o: Adsr, t: f32) -> Adsr {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Adsr { attack: l(self.attack, o.attack), decay: l(self.decay, o.decay), sustain: l(self.sustain, o.sustain), release: l(self.release, o.release), slope: l(self.slope, o.slope) }
    }

    /// The value `t` seconds into a note of `length` seconds, its attack
    /// and decay played `speed` times as fast (the release ends with the
    /// note).
    pub fn at(&self, t: f32, length: f32, speed: f32) -> f32 {
        let len = length.max(1e-3);
        let release = (self.release * len).min(0.9 * len);
        let attack = (self.attack * len / speed).clamp(ADSR_MIN_ATTACK, (len - release).max(ADSR_MIN_ATTACK));
        let decay = (self.decay * len / speed).max(1e-3);
        let from = len - release;
        let held = |t: f32| {
            if t < attack {
                t / attack
            } else {
                // The sustain slopes from its level over the held part.
                let s = (self.sustain + self.slope * ((t - attack) / (from - attack).max(1e-4)).clamp(0.0, 1.0)).clamp(0.0, 1.0);
                (s + (1.0 - s) * (-3.0 * (t - attack) / decay).exp()).clamp(0.0, 1.0)
            }
        };
        if t >= len {
            0.0
        } else if t < from {
            held(t)
        } else {
            let x = (t - from) / release.max(1e-4);
            held(from) * (1.0 - x) * (1.0 - x)
        }
    }
}

/// A letter's voicing of the chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voice {
    /// Seed of the movers (the sound seed's: the letters' bands start alike
    /// and Contrast moves them apart).
    pub seed: u32,
    /// How far this letter's bands move the other way (0 as A .. 1 turned)
    /// and are placed off A's (octaves, signed).
    pub flip: f32,
    pub shift: f32,
    /// The letter's ADSR and the speed of its attack and decay.
    pub adsr: Adsr,
    pub speed: f32,
    /// The shortest sweep (beats): notes starting inside one do not start
    /// it again, so stabs move through one sweep together.
    pub sweep: f32,
    /// The follower lowpass's closed cutoff (Hz, in [`FILTER_HZ`]).
    pub filter_low: f32,
    /// The emphasis's cut, in harmonics of the note.
    pub eq_ratio: f32,
    /// The comb's harmonic and its sign (−1 hollow, 1 metallic).
    pub comb_harmonic: f32,
    pub comb_sign: f32,
    /// The symmetric drive's fold share (0 tanh .. 1 folded).
    pub fold: f32,
    /// How much the input's envelope sets the processing (0 none .. 1 in full).
    pub follow: f32,
    /// How each moving stage's bands move against each other.
    pub relations: [Relation; MOVING_STAGES],
}

/// One stage of the chain.
pub trait Stage: Send {
    /// Process `buf` (one control chunk) at stage amount `amount` (0..1).
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32);
    fn reset(&mut self);
}

/// The stages, in chain order (the weights in [`RACKS`] follow it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    /// A resonant 24 dB/octave lowpass on the source, opened by the input's
    /// envelope follower (800 Hz .. 8 kHz), bass sources only.
    Filter,
    /// Pre-emphasis: broad boosts either side of a narrower, deeper cut,
    /// around the note's harmonics, each band moving on its own.
    Emphasis,
    /// A high shelf before the drive: up on a dark source, down on a harsh
    /// one, so the drive bites on the mids and the top alike.
    Tame,
    /// The heavy multiband drive (tape, a character drive into a slight
    /// tanh, a slight tube).
    Drive,
    /// The emphasis reversed after the drive (the boosts cut back, the cut
    /// partly restored), its bands moving their own ways.
    Deemphasis,
    /// Half of the tame given back after the deemphasis.
    Untame,
    /// Two gaps moving against each other: a wide one (24 dB/octave) in
    /// 250 .. 500 Hz, a narrower one (18 dB/octave) in 1 .. 5 kHz.
    Notch,
    /// A subtle asymmetric saturation (a diode-like curve): even harmonics.
    Asym,
    /// A damped feedback comb on a harmonic of the note.
    Comb,
    /// Peaks and nulls in 2 .. 8 kHz that grow over a held note.
    Air,
    /// A four-stage phaser around the note's upper harmonics, swept by
    /// the note (left and right against each other).
    Phaser,
    /// A slight Chebyshev waveshaper: the last colour.
    Shaper,
    /// The post filter: tames the fizz the stages made.
    Post,
    /// A multiband compressor (the Squash at 60 %, mostly mixed in), on top
    /// of the lane's own Squash.
    Multiband,
    /// A wide stereo chorus after the chain, at most [`CHORUS_WET`] wet
    /// (user, 2026-10-23: before the drive the detuned copies distorted
    /// against each other, the most off-harmonic energy of any stage); its
    /// delays move with the note.
    Chorus,
}

// MUSICAL: tune by ear (user, 2026-10-22: broad boosts and slimmer, deeper
// cuts into the drive, the reversed EQ after it, moving differently; mid
// notches moving against each other; three kinds of distortion sprinkled
// in; a slight multiband compressor; 2026-10-23: the chorus last, wide).
pub const ORDER: [StageKind; 15] = [
    StageKind::Filter,
    StageKind::Emphasis,
    StageKind::Tame,
    StageKind::Drive,
    StageKind::Deemphasis,
    StageKind::Untame,
    StageKind::Notch,
    StageKind::Asym,
    StageKind::Comb,
    StageKind::Air,
    StageKind::Phaser,
    StageKind::Shaper,
    StageKind::Post,
    StageKind::Multiband,
    StageKind::Chorus,
];

// MUSICAL: first draft. Racks: how much of each stage (in [`ORDER`]) the
// amount brings in. The sound seed blends two.
pub const RACKS: [(&str, [f32; 15]); 4] = [
    ("talker", [1.0, 1.0, 1.0, 0.8, 1.0, 1.0, 1.0, 0.5, 0.2, 0.8, 0.3, 0.6, 0.6, 1.0, 0.3]),
    ("metal", [1.0, 0.7, 1.0, 0.8, 0.7, 1.0, 0.6, 0.6, 1.0, 0.6, 0.2, 0.8, 0.7, 1.0, 0.2]),
    ("tear", [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.8, 1.0, 0.4, 1.0, 0.7, 1.0, 0.8, 1.0, 0.4]),
    ("reese", [1.0, 0.6, 1.0, 0.6, 0.6, 1.0, 0.7, 0.4, 0.3, 0.5, 1.0, 0.5, 0.5, 1.0, 1.0]),
];

// MUSICAL: first draft, tune by ear. Stage depths at full stage amount.
/// The emphasis: a broad boost below, a narrower deeper cut at the centre,
/// a broad boost above (dB, Q, ratio to the centre), for a bass-like
/// source and a lead-like one (user, 2026-10-23: more boost and cut, by the
/// source; a lead's drive turns harsh sooner), and where the cut may sit
/// (Hz, moved with the register).
// User, 2026-10-23: a strong nasal 5 kHz; the bands cluster in the middle,
// under 1 kHz for a bass ([`EMPHASIS_TOP_HZ`], bass-like and lead-like,
// moved with the register by at most an octave).
const EMPHASIS: [(f32, f32, f32); 3] = [(12.0, 0.6, 0.55), (-20.0, 1.6, 1.0), (6.0, 0.9, 1.6)];
const EMPHASIS_LEAD: [(f32, f32, f32); 3] = [(7.0, 0.7, 0.6), (-14.0, 2.0, 1.0), (5.0, 0.8, 1.7)];
const EMPHASIS_CUT_HZ: (f32, f32) = (250.0, 600.0);
const EMPHASIS_TOP_HZ: (f32, f32) = (1000.0, 2500.0);
/// The follower's lowpass (user, 2026-10-23: on the bass source at the
/// start, opening fully, its lowest point 800 Hz .. 8 kHz by the seed,
/// 24 dB/octave, a little resonance): the range of the closed cutoff (Hz),
/// the two sections' Q (Butterworth's lower one, the upper raised), the
/// cutoff it opens to (fully open, and at no amount).
const FILTER_HZ: (f32, f32) = (800.0, 8000.0);
const FILTER_Q: (f32, f32) = (0.54, 1.9);
const FILTER_OPEN_HZ: f32 = 20_000.0;
/// The ADSR's share of the opening (the envelope follower the rest).
const FILTER_ADSR: f32 = 0.75;
/// The deemphasis's share of each band, reversed (the boosts cut back,
/// the cut restored by this much, broader). Under half: most of the
/// emphasis and its movement stays heard (user, 2026-10-23: not heard).
const DEEMPHASIS: [f32; 3] = [0.45, 0.2, 0.75];
const DEEMPHASIS_CUT_Q: f32 = 0.9;

/// A notch: a true gap between two crossovers around its centre.
struct NotchSpec {
    /// The range its centre moves in (Hz).
    range: (f32, f32),
    /// The gap's width (octaves between its edges) and its slopes
    /// (24 dB/octave, else 18).
    width: f32,
    steep: bool,
    /// A bell at the centre that deepens the gap (Q, dB at full).
    bell: (f32, f32),
    /// The most of the band the gap takes out (0..1).
    most: f32,
    /// Boosts on both shoulders just outside the edges (dB at full, Q),
    /// so the region around the gap stays full.
    shoulders: (f32, f32),
}

/// The notches (user, 2026-10-23: the low one wide, 24 dB/octave, a true
/// gap; the upper one 18 dB/octave, narrower). The 24 dB/octave gap of 1.5
/// octaves is −14 dB at its middle, its bell takes it to about −25; the
/// 18 dB/octave one is −23 dB on its own.
// User, 2026-10-23: the low cut was too hard; less of it, its shoulders
// boosted. At full the low gap is about −12 dB in its middle. 2026-10-27:
// the holes in the top too big; the upper gap takes 60 % of its band, its
// shoulders lifted 3 dB, the air's dip halved.
const NOTCHES: [NotchSpec; 2] = [
    NotchSpec { range: (250.0, 500.0), width: 1.5, steep: true, bell: (1.0, -4.0), most: 0.75, shoulders: (4.0, 1.0) },
    NotchSpec { range: (1000.0, 5000.0), width: 1.25, steep: false, bell: (1.0, 0.0), most: 0.6, shoulders: (3.0, 1.0) },
];
/// The air bands (Hz, Q, dB), each moving within ±[`AIR_OCTAVES`], kept in
/// [`AIR_RANGE_HZ`]; the share of a note they take to grow in full.
const AIR: [(f32, f32, f32); 3] = [(2500.0, 1.0, 3.0), (4500.0, 3.0, -4.0), (7000.0, 1.2, 2.0)];
const AIR_OCTAVES: f32 = 0.6;
const AIR_RANGE_HZ: (f32, f32) = (2000.0, 8000.0);
const AIR_GROW: f32 = 0.6;
/// The heavy multiband drive (user, 2026-10-23: very subtle tape up to
/// about 150 Hz; 150 Hz .. 1 kHz in two stages, a character drive that
/// grows highs from the mids, then a slight symmetric one that brings them
/// out; only a slight asymmetric drive on the highs; never all wet): its
/// crossovers (Hz, moved with the register) and its bands at full. The
/// character drive's dB moves with the voice's fold share by up to
/// [`DRIVE_FOLD_DB`]. The subtle asymmetric drive's gain, its negative
/// side's softness and its wet at full; the shaper's third and fifth
/// harmonic shares.
const DRIVE_XOVER: (f32, f32) = (150.0, 1000.0);
const DRIVE_BANDS: [BandConfig; 3] = [
    BandConfig { passes: [Pass { curve: Curve::Tape, drive_db: 4.0, wet: 0.35 }, Pass::OFF], contain: 0.0, oversample: 1 },
    BandConfig {
        passes: [Pass { curve: Curve::Drive, drive_db: 24.0, wet: 0.7 }, Pass { curve: Curve::Tanh, drive_db: 8.0, wet: 0.4 }],
        contain: 3.0,
        oversample: 2,
    },
    BandConfig { passes: [Pass { curve: Curve::Tube, drive_db: 8.0, wet: 0.35 }, Pass { curve: Curve::Tanh, drive_db: 4.0, wet: 0.25 }], contain: 1.0, oversample: 2 },
];
const DRIVE_FOLD_DB: f32 = 4.0;
const ASYM_DB: f32 = 9.0;
const ASYM_SOFT: f32 = 0.6;
const ASYM_WET: f32 = 0.45;
const SHAPER: (f32, f32) = (0.2, 0.08);
/// The comb: its feedback, the damping in its loop (Hz), its mix.
const COMB_FEEDBACK: f32 = 0.55;
const COMB_DAMP_HZ: f32 = 2500.0;
const COMB_MIX: f32 = 0.5;
/// The phaser's centre (harmonics), feedback; the chorus's delays and how
/// far the note moves them (s).
const PHASER_HARMONIC: f32 = 6.0;
const PHASER_FEEDBACK: f32 = 0.3;
const CHORUS_DELAYS: (f32, f32) = (0.009, 0.013);
const CHORUS_DEPTH: f32 = 0.004;
/// The chorus's wet share at full stage amount.
const CHORUS_WET: f32 = 0.3;
/// The post lowpass: harmonics of the note, kept between these.
const POST_HARMONIC: f32 = 120.0;
const POST_RANGE_HZ: (f32, f32) = (5000.0, 16_000.0);
/// The multiband's mix at full stage amount (the Squash at its mild end).
const MULTIBAND_MIX: f32 = 0.85;
/// The Squash amount the multiband stage runs at (user, 2026-10-27: harder
/// than the lane'"'"'s own, on top of it).
const MULTIBAND_SQUASH: f32 = 0.6;
/// The shortest attack; how much the seed's ADSR strays from its shape.
const ADSR_MIN_ATTACK: f32 = 0.002;
const ADSR_JITTER: f32 = 0.3;
/// The input envelope's follower (fast, slow) in seconds; the seeded range
/// of how much it sets the processing; the processing factor's range.
const LEVEL_SECS: (f32, f32) = (0.006, 0.3);
const FOLLOW: (f32, f32) = (0.5, 1.0);
const FOLLOW_RANGE: (f32, f32) = (0.25, 1.75);
/// The movement's speed at full amount (user: only a little faster).
const AMOUNT_SPEED: f32 = 2.0;
/// Variation's scale of the depths and of the speed at 0 and 1.
const VARIATION_DEPTH: (f32, f32) = (0.6, 1.5);
const VARIATION_SPEED: (f32, f32) = (0.8, 1.3);
/// Contrast (user, 2026-10-22: at 0 every letter alike, at 1 on purpose
/// very different): how far B's bands sit off A's (octaves) and its
/// attack and decay speed against A's (one of these, seeded); C's and D's
/// own moves as shares of B's.
const B_SHIFT: f32 = 1.0;
/// The sweeps' shortest spans (beats, user 2026-10-23: one move per hit of
/// a stab was too short to hear); B takes the other one at high Contrast.
const SWEEP_BEATS: [f32; 2] = [1.0, 2.0];
const B_SPEEDS: [f32; 2] = [0.4, 2.5];
const C_SHARE: f32 = 0.4;
const D_EXTRA: f32 = 0.5;

const S_RACK: u32 = 0xE50;
const S_VOICE: u32 = 0xE51;
const S_NOTE: u32 = 0xE52;
const S_MOVER: u32 = 0xE54;

// --- Motion ----------------------------------------------------------------

/// How a stage's bands move against each other over a note (seeded per
/// stage, and per letter by Contrast).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// All the same way.
    Together,
    /// Away from each other: the lower ones down, the upper ones up.
    Apart,
    /// A peak and a null towards each other and over each other (bands
    /// that cannot cross move apart).
    Cross,
}

pub const RELATIONS: [Relation; 3] = [Relation::Together, Relation::Apart, Relation::Cross];

/// The stages with movers (their index in [`Voice::relations`]).
pub const MOVING_STAGES: usize = 5;
const ST_EMPHASIS: u8 = 0;
const ST_DEEMPHASIS: u8 = 1;
const ST_NOTCH: u8 = 2;
const ST_AIR: u8 = 3;
const ST_PHASER: u8 = 4;

// MUSICAL: tune by ear (user, 2026-10-22: some seeds move the points the
// same way, some apart, some over each other, peaks over troughs; boosts
// not piled up). A crossing pair travels this share of the gap between
// them each way (over 1: they pass each other at the ADSR's ends); boosts
// stay at least this far apart (octaves); how far Contrast's change of a
// relation reaches C (a share of B's).
const CROSS_SHARE: f32 = 0.65;
const BOOST_GAP_OCT: f32 = 1.0;
const C_RELATION: f32 = 0.4;

/// How one filter band moves: octaves off its base by the ADSR, the note's
/// draw and Morph, its gain by the input's envelope.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Mover {
    /// Its resting place off the band's base (octaves, seeded).
    pub base_oct: f32,
    /// Octaves the ADSR moves it from the note's peak to its rest (signed).
    pub adsr_oct: f32,
    /// Octaves the note's draw moves it.
    pub note_oct: f32,
    /// How much the input's envelope scales its gain.
    pub level: f32,
}

// MUSICAL: first draft. A mover's ranges: its resting offset, its ADSR
// depth (octaves), the note's and the envelope's share.
const MOVE_BASE_OCT: f32 = 0.4;
const MOVE_ADSR_OCT: (f32, f32) = (0.7, 1.6);
const MOVE_NOTE_OCT: f32 = 0.7;
const MOVE_LEVEL: (f32, f32) = (0.3, 0.8);

impl Mover {
    /// Band `band` of stage `stage` for seed `seed`.
    pub fn seeded(seed: u32, stage: u8, band: u8) -> Self {
        let u = |k: u8| rng::unit_f32(rng::hash(seed, stage, band, k, S_MOVER));
        let lerp = |(a, b): (f32, f32), t: f32| a + (b - a) * t;
        Self {
            base_oct: MOVE_BASE_OCT * (2.0 * u(7) - 1.0),
            adsr_oct: if u(0) < 0.5 { -1.0 } else { 1.0 } * lerp(MOVE_ADSR_OCT, u(1)),
            note_oct: MOVE_NOTE_OCT * (2.0 * u(5) - 1.0),
            level: lerp(MOVE_LEVEL, u(6)),
        }
    }

    /// Octaves off the band's base for this motion and voice: the voice
    /// turns the ADSR's direction by its flip and places it by its shift.
    #[inline]
    pub fn octaves(&self, m: &Motion, v: &Voice) -> f32 {
        let adsr = self.adsr_oct * (1.0 - 2.0 * v.flip);
        self.base_oct + v.shift + adsr * (m.adsr - 0.5) * 2.0 + self.note_oct * m.note + m.morph
    }

    /// The gain factor for this motion (louder input: more).
    #[inline]
    pub fn gain(&self, m: &Motion) -> f32 {
        (1.0 + self.level * m.level).max(0.0)
    }
}

/// The movers of a stage's bands, built again when the seed or the
/// relation changes.
#[derive(Debug, Clone, Copy)]
struct Movers<const N: usize> {
    key: Option<(u32, Relation)>,
    m: [Mover; N],
}

impl<const N: usize> Default for Movers<N> {
    fn default() -> Self {
        Self { key: None, m: [Mover::default(); N] }
    }
}

impl<const N: usize> Movers<N> {
    /// The movers for `seed`, stage `stage`, its bands resting at `bases`
    /// (octaves, ascending) with gains of the signs `gains`, moving by
    /// `relation`. `can_cross`: whether the bands may pass each other.
    fn get(&mut self, seed: u32, stage: u8, relation: Relation, bases: &[f32; N], gains: &[f32; N], can_cross: bool) -> &[Mover; N] {
        if self.key != Some((seed, relation)) {
            self.m = relate(seed, stage, relation, bases, gains, can_cross);
            self.key = Some((seed, relation));
        }
        &self.m
    }
}

/// A stage's movers under `relation` (see [`Movers::get`]).
pub fn relate<const N: usize>(seed: u32, stage: u8, relation: Relation, bases: &[f32; N], gains: &[f32; N], can_cross: bool) -> [Mover; N] {
    let mut m: [Mover; N] = std::array::from_fn(|b| Mover::seeded(seed, stage, b as u8));
    let dir = if m[0].adsr_oct < 0.0 { -1.0 } else { 1.0 };
    let mean = bases.iter().sum::<f32>() / N.max(1) as f32;
    // Away from the middle; bands at one place by their order.
    let side = |b: usize| match bases[b] - mean {
        d if d > 1e-3 => 1.0,
        d if d < -1e-3 => -1.0,
        _ => if b % 2 == 0 { -1.0 } else { 1.0 },
    };
    let pair = (0..N.saturating_sub(1)).filter(|&i| gains[i] * gains[i + 1] < 0.0).nth(rng::below(rng::hash(seed, stage, 0, 0, S_MOVER ^ 1), N.max(2) as u32 - 1) as usize % N.max(1));
    let relation = if relation == Relation::Cross && (!can_cross || pair.is_none()) { Relation::Apart } else { relation };
    match relation {
        Relation::Together => {
            for x in m.iter_mut() {
                x.adsr_oct = dir * x.adsr_oct.abs();
            }
        }
        Relation::Apart => {
            for (b, x) in m.iter_mut().enumerate() {
                x.adsr_oct = side(b) * x.adsr_oct.abs();
            }
        }
        Relation::Cross => {
            let i = pair.unwrap_or(0);
            for x in m.iter_mut() {
                x.adsr_oct = dir * x.adsr_oct.abs();
            }
            // The pair rests where it is and moves over each other: at the
            // ADSR's ends each has passed the other's resting place.
            let gap = (bases[i + 1] - bases[i]).abs().max(0.5);
            let depth = CROSS_SHARE * gap;
            (m[i].adsr_oct, m[i + 1].adsr_oct) = (dir * depth, -dir * depth);
            (m[i].base_oct, m[i + 1].base_oct) = (0.0, 0.0);
        }
    }
    m
}

/// Keep the boosts (positive `gains`) of `hz` (ascending by band) at least
/// [`BOOST_GAP_OCT`] apart: an upper boost too near the one below moves up.
fn spread_boosts<const N: usize>(hz: &mut [f32; N], gains: &[f32; N]) {
    let mut last: Option<f32> = None;
    for b in 0..N {
        if gains[b] <= 0.0 {
            continue;
        }
        if let Some(l) = last {
            hz[b] = hz[b].max(l * BOOST_GAP_OCT.exp2());
        }
        last = Some(hz[b]);
    }
}

// --- Stages ----------------------------------------------------------------

/// Run `bands` over `buf` in series.
fn run_bands(bands: &mut [Biquad], buf: &mut [StereoSample]) {
    for s in buf.iter_mut() {
        let mut x = [s.left, s.right];
        for b in bands.iter_mut() {
            x = b.run(x);
        }
        (s.left, s.right) = (x[0], x[1]);
    }
}

/// The emphasis around the note's harmonics; `de` the reversed one (its
/// bands move their own ways).
struct Emphasis {
    bands: [Biquad; 3],
    movers: Movers<3>,
    de: bool,
}

impl Emphasis {
    /// The cut's resting frequency for these controls.
    pub(crate) fn base(c: &Ctl) -> f32 {
        let k = c.source.scale();
        (c.f0 * c.voice.eq_ratio).clamp(EMPHASIS_CUT_HZ.0 * k, EMPHASIS_CUT_HZ.1 * k)
    }
}

impl Stage for Emphasis {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let base = Self::base(c);
        let lead = c.source.lead;
        let band = |k: usize| {
            let l = |a: f32, b: f32| a + (b - a) * lead;
            let (db, q, ratio) = (l(EMPHASIS[k].0, EMPHASIS_LEAD[k].0), l(EMPHASIS[k].1, EMPHASIS_LEAD[k].1), l(EMPHASIS[k].2, EMPHASIS_LEAD[k].2));
            if self.de { (-db * DEEMPHASIS[k], if db < 0.0 { DEEMPHASIS_CUT_Q } else { q }, ratio) } else { (db, q, ratio) }
        };
        let gains: [f32; 3] = std::array::from_fn(|k| band(k).0);
        let bases: [f32; 3] = std::array::from_fn(|k| band(k).2.log2());
        let stage = if self.de { ST_DEEMPHASIS } else { ST_EMPHASIS };
        let movers = *self.movers.get(c.voice.seed, stage, c.voice.relations[stage as usize], &bases, &gains, true);
        let top = (EMPHASIS_TOP_HZ.0 + (EMPHASIS_TOP_HZ.1 - EMPHASIS_TOP_HZ.0) * lead) * c.source.scale().min(2.0);
        let mut hz: [f32; 3] = std::array::from_fn(|k| (base * band(k).2 * movers[k].octaves(&c.motion, &c.voice).exp2()).clamp(SUB_HZ, top));
        spread_boosts(&mut hz, &gains);
        for h in hz.iter_mut() {
            *h = h.min(top);
        }
        for (k, b) in self.bands.iter_mut().enumerate() {
            let (db, q, _) = band(k);
            b.peaking(hz[k].min(0.4 * c.sr), q, db * amount * movers[k].gain(&c.motion), c.sr);
        }
        run_bands(&mut self.bands, buf);
    }

    fn reset(&mut self) {
        self.bands.iter_mut().for_each(Biquad::reset);
    }
}

/// The follower's lowpass: the sweep's ADSR opens and closes it (a curve
/// over every sweep, its shape the seed's), the input's envelope against
/// its own average adds to it (user, 2026-10-23: on the envelope follower
/// alone it stood still on a held mudpie, muffled).
struct Filter {
    s: [Svf; 2],
}

impl Filter {
    /// The cutoff for the sweep's ADSR `adsr` (0..1) and the envelope
    /// `level` (−1 .. 1) at stage amount `amount`, closed at `low` (Hz).
    pub(crate) fn cutoff(adsr: f32, level: f32, amount: f32, low: f32) -> f32 {
        let open = (FILTER_ADSR * adsr + (1.0 - FILTER_ADSR) * (0.5 + 0.5 * level)).clamp(0.0, 1.0);
        // Less amount: the closed point rises towards the top.
        let low = FILTER_OPEN_HZ * (low / FILTER_OPEN_HZ).powf(amount.clamp(0.0, 1.0));
        low * (FILTER_OPEN_HZ / low).powf(open)
    }
}

impl Stage for Filter {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let hz = Self::cutoff(c.motion.adsr, c.motion.level, amount, c.voice.filter_low);
        self.s[0].set(hz, 1.0 / FILTER_Q.0, c.sr);
        self.s[1].set(hz, 1.0 / FILTER_Q.1, c.sr);
        for x in buf.iter_mut() {
            let y = self.s[0].run([x.left, x.right])[0];
            let y = self.s[1].run(y)[0];
            (x.left, x.right) = (y[0], y[1]);
        }
    }

    fn reset(&mut self) {
        self.s.iter_mut().for_each(Svf::reset);
    }
}

/// The top shelf before the drive, and half of it given back after (`back`).
struct Tame {
    shelf: Biquad,
    back: bool,
}

impl Tame {
    /// The shelf's gain (dB) for this source at stage amount `amount`.
    pub(crate) fn db(source: &Source, amount: f32, back: bool) -> f32 {
        let db = (TAME_LIFT + (TAME_DB - TAME_LIFT) * source.harsh) * amount;
        if back { -TAME_BACK * db } else { db }
    }
}

impl Stage for Tame {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        self.shelf.high_shelf(TAME_HZ * c.source.scale(), Self::db(&c.source, amount, self.back), c.sr);
        for s in buf.iter_mut() {
            let y = self.shelf.run([s.left, s.right]);
            (s.left, s.right) = (y[0], y[1]);
        }
    }

    fn reset(&mut self) {
        self.shelf.reset();
    }
}

/// The heavy drive, multiband (D54): subtle tape low, a character drive
/// then a slight tanh on the mids, a slight tube on the highs, its
/// crossovers moving with the register.
struct Drive {
    mb: MbDist,
}

impl Drive {
    /// The multiband's setting for these controls.
    pub(crate) fn config(c: &Ctl) -> MbConfig {
        let k = c.source.scale();
        let soft = 1.0 - 0.5 * c.source.harsh;
        let mut bands = DRIVE_BANDS;
        bands[1].passes[0].drive_db += DRIVE_FOLD_DB * (2.0 * c.voice.fold - 1.0);
        bands[2].passes[0].drive_db *= soft;
        bands[2].passes[0].wet *= soft;
        MbConfig { xover: (DRIVE_XOVER.0 * k, DRIVE_XOVER.1 * k), bands }
    }
}

impl Stage for Drive {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let cfg = Self::config(c);
        self.mb.process(buf, &cfg, amount * c.source.drive(), [0.0; 3]);
    }

    fn reset(&mut self) {
        self.mb.reset();
    }
}

/// A subtle asymmetric saturation: the positive side clips harder than the
/// negative (even harmonics), its offset blocked.
struct Asym {
    dc: [f32; 2],
    os: Oversampler,
}

impl Stage for Asym {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        self.os.ready(c.sr);
        let k = 10f32.powf(ASYM_DB * amount * c.source.drive() / 20.0);
        let shape = |x: f32| {
            let v = x * k;
            let y = (if v >= 0.0 { v.tanh() } else { ASYM_SOFT * (v / ASYM_SOFT).tanh() }) / k.sqrt();
            x + (y - x) * amount * ASYM_WET
        };
        let a = 1.0 - (-TAU * 20.0 / c.sr).exp();
        for s in buf.iter_mut() {
            let mut y = self.os.run([s.left, s.right], |u| [shape(u[0]), shape(u[1])]);
            for ch in 0..2 {
                self.dc[ch] = flush(self.dc[ch] + a * (y[ch] - self.dc[ch]));
                y[ch] -= self.dc[ch];
            }
            [s.left, s.right] = y;
        }
    }

    fn reset(&mut self) {
        self.dc = [0.0; 2];
        self.os.reset();
    }
}

/// A slight Chebyshev waveshaper: third and fifth harmonics on a soft-
/// clipped copy, added beside the signal.
struct Shaper {
    os: Oversampler,
}

impl Stage for Shaper {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        self.os.ready(c.sr);
        let (a3, a5) = (SHAPER.0 * amount, SHAPER.1 * amount);
        let shape = |x: f32| {
            let v = x.tanh();
            let v2 = v * v;
            // T3 = 4v³ − 3v, T5 = 16v⁵ − 20v³ + 5v.
            x + a3 * v * (4.0 * v2 - 3.0) + a5 * v * (16.0 * v2 * v2 - 20.0 * v2 + 5.0)
        };
        for s in buf.iter_mut() {
            [s.left, s.right] = self.os.run([s.left, s.right], |u| [shape(u[0]), shape(u[1])]);
        }
    }

    fn reset(&mut self) {
        self.os.reset();
    }
}

/// Two gaps moving against each other in their ranges.
struct Notch {
    gaps: [Gap; 2],
    bells: [Biquad; 2],
    shoulders: [[Biquad; 2]; 2],
    movers: Movers<2>,
}

impl Notch {
    /// Notch `k`'s centre for these controls and its mover: from the
    /// middle of its range (in octaves), kept in it.
    pub(crate) fn centre(c: &Ctl, k: usize, m: &Mover) -> f32 {
        let (lo, hi) = NOTCHES[k].range;
        let (lo, hi) = (lo * c.source.scale(), hi * c.source.scale());
        let span = (hi / lo).log2();
        let at = (0.5 + m.octaves(&c.motion, &c.voice) / span).clamp(0.0, 1.0);
        lo * (hi / lo).powf(at)
    }
}

impl Stage for Notch {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let bases = NOTCHES.map(|n| (n.range.0 * n.range.1).sqrt().log2());
        let movers = *self.movers.get(c.voice.seed, ST_NOTCH, c.voice.relations[ST_NOTCH as usize], &bases, &[-1.0, -1.0], false);
        let mut keep = [1.0f32; 2];
        for (k, n) in NOTCHES.iter().enumerate() {
            let hz = Self::centre(c, k, &movers[k]);
            let half = (0.5 * n.width).exp2();
            self.gaps[k].set(hz / half, hz * half, n.steep, c.sr);
            let depth = (amount * movers[k].gain(&c.motion)).clamp(0.0, 1.0);
            keep[k] = 1.0 - n.most * depth;
            self.bells[k].peaking(hz, n.bell.0, n.bell.1 * depth, c.sr);
            let out = (0.5 * n.width + 0.3).exp2();
            self.shoulders[k][0].peaking((hz / out).max(SUB_HZ), n.shoulders.1, n.shoulders.0 * depth, c.sr);
            self.shoulders[k][1].peaking(hz * out, n.shoulders.1, n.shoulders.0 * depth, c.sr);
        }
        for s in buf.iter_mut() {
            let mut x = [s.left, s.right];
            for k in 0..2 {
                x = self.gaps[k].run(x, keep[k]);
                if NOTCHES[k].bell.1 != 0.0 {
                    x = self.bells[k].run(x);
                }
                if NOTCHES[k].shoulders.0 != 0.0 {
                    x = self.shoulders[k][0].run(x);
                    x = self.shoulders[k][1].run(x);
                }
            }
            (s.left, s.right) = (x[0], x[1]);
        }
    }

    fn reset(&mut self) {
        self.gaps.iter_mut().for_each(Gap::reset);
        self.bells.iter_mut().for_each(Biquad::reset);
        self.shoulders.iter_mut().flatten().for_each(Biquad::reset);
    }
}

/// Peaks and nulls up top that grow over a held note: none at its onset,
/// in full after the grow time (shorter the more amount).
struct Air {
    bands: [Biquad; 3],
    movers: Movers<3>,
}

impl Air {
    /// How far grown the bands are at `age` seconds into a note of
    /// `length`: none at its onset, in full by [`AIR_GROW`] of it (sooner
    /// the more amount).
    pub(crate) fn growth(age: f32, length: f32, amount: f32) -> f32 {
        let t = AIR_GROW * length.max(1e-3) / (1.0 + (AMOUNT_SPEED - 1.0) * amount);
        (age / t).clamp(0.0, 1.0)
    }
}

impl Stage for Air {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let grow = Self::growth(c.motion.age, c.motion.length, amount);
        let gains = AIR.map(|a| a.2);
        let bases = AIR.map(|a| a.0.log2());
        let movers = *self.movers.get(c.voice.seed, ST_AIR, c.voice.relations[ST_AIR as usize], &bases, &gains, true);
        let mut hz: [f32; 3] = std::array::from_fn(|k| {
            let oct = movers[k].octaves(&Motion { morph: 0.0, ..c.motion }, &c.voice).clamp(-AIR_OCTAVES, AIR_OCTAVES);
            let up = (AIR_SHIFT_SHARE * c.source.shift * (1.0 - c.source.lead)).exp2();
            (AIR[k].0 * up * oct.exp2()).clamp(AIR_RANGE_HZ.0 * up, (AIR_RANGE_HZ.1 * up).min(AIR_TOP_HZ))
        });
        spread_boosts(&mut hz, &gains);
        for (k, b) in self.bands.iter_mut().enumerate() {
            let (_, q, db) = AIR[k];
            b.peaking(hz[k].min(0.4 * c.sr), q, db * amount * grow * movers[k].gain(&c.motion), c.sr);
        }
        run_bands(&mut self.bands, buf);
    }

    fn reset(&mut self) {
        self.bands.iter_mut().for_each(Biquad::reset);
    }
}

/// A feedback comb on a harmonic of the note, damped in its loop, mixed in
/// beside the dry signal.
struct Comb {
    line: Delay,
    damp: [f32; 2],
}

impl Comb {
    /// The comb's delay in samples for these controls.
    pub(crate) fn delay(c: &Ctl) -> f32 {
        c.sr / (c.f0 * c.voice.comb_harmonic).max(COMB_LOWEST_HZ)
    }
}

impl Stage for Comb {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let d = Self::delay(c);
        let fb = c.voice.comb_sign * COMB_FEEDBACK * amount;
        let k = 1.0 - (-TAU * COMB_DAMP_HZ / c.sr).exp();
        let m = COMB_MIX * amount;
        for s in buf.iter_mut() {
            let x = [s.left, s.right];
            let mut y = [0.0; 2];
            for ch in 0..2 {
                // The loop's lowpass: the repeats darken, no whistle.
                self.damp[ch] = flush(self.damp[ch] + k * (self.line.read(d, ch) - self.damp[ch]));
                y[ch] = safe(x[ch] + fb * self.damp[ch]);
            }
            self.line.push(y);
            [s.left, s.right] = mix(x, [y[0] * (1.0 - 0.5 * fb.abs()), y[1] * (1.0 - 0.5 * fb.abs())], m);
        }
    }

    fn reset(&mut self) {
        self.line.reset();
        self.damp = [0.0; 2];
    }
}

/// Four first-order allpasses with feedback, swept by the note (the left
/// and the right channel against each other).
struct Phaser {
    z: [[f32; 2]; 4],
    last: [f32; 2],
    movers: Movers<2>,
}

impl Stage for Phaser {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let movers = *self.movers.get(c.voice.seed, ST_PHASER, c.voice.relations[ST_PHASER as usize], &[0.0; 2], &[1.0, 1.0], false);
        let coef = |m: &Mover| {
            let hz = (c.f0 * PHASER_HARMONIC * m.octaves(&c.motion, &c.voice).exp2()).clamp(100.0, 0.4 * c.sr);
            let t = (PI * hz / c.sr).tan();
            (t - 1.0) / (t + 1.0)
        };
        let k = [coef(&movers[0]), coef(&movers[1])];
        let fb = PHASER_FEEDBACK * amount;
        for s in buf.iter_mut() {
            let x = [s.left, s.right];
            let mut y = [0.0; 2];
            for ch in 0..2 {
                let mut v = x[ch] + fb * self.last[ch];
                for st in self.z.iter_mut() {
                    let out = k[ch] * v + st[ch];
                    st[ch] = flush(v - k[ch] * out);
                    v = out;
                }
                self.last[ch] = safe(v);
                y[ch] = 0.5 * (x[ch] + v);
            }
            [s.left, s.right] = mix(x, y, amount);
        }
    }

    fn reset(&mut self) {
        self.z = [[0.0; 2]; 4];
        self.last = [0.0; 2];
    }
}

/// A two-tap stereo chorus.
struct Chorus {
    line: Delay,
}

impl Stage for Chorus {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        // The delays move with the note (a pitch bend on the copies as the
        // ADSR moves), the two sides the other ways; more amount, more.
        let swing = (c.motion.adsr - 0.5) * 2.0 * (1.0 - 2.0 * c.voice.flip) + 0.5 * c.motion.note;
        let dl = (CHORUS_DELAYS.0 + CHORUS_DEPTH * amount * swing) * c.sr;
        let dr = (CHORUS_DELAYS.1 - CHORUS_DEPTH * amount * swing) * c.sr;
        // Wide: each side its own copy, mixed in at most [`CHORUS_WET`].
        let wet = CHORUS_WET * amount;
        for s in buf.iter_mut() {
            let x = [s.left, s.right];
            self.line.push(x);
            let (l, r) = (self.line.read(dl, 0), self.line.read(dr, 1));
            s.left = x[0] + wet * (l - x[0]);
            s.right = x[1] + wet * (r - x[1]);
        }
    }

    fn reset(&mut self) {
        self.line.reset();
    }
}

/// The post lowpass on the note's upper harmonics.
struct Post {
    lp: Biquad,
}

impl Stage for Post {
    fn process(&mut self, buf: &mut [StereoSample], c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let hz = (c.f0 * POST_HARMONIC).clamp(POST_RANGE_HZ.0, POST_RANGE_HZ.1);
        // Less amount: the cutoff opens towards the top.
        let hz = hz * (POST_RANGE_HZ.1 / hz).powf(1.0 - amount);
        self.lp.lowpass(hz, 0.75, c.sr);
        for s in buf.iter_mut() {
            let y = self.lp.run([s.left, s.right]);
            (s.left, s.right) = (y[0], y[1]);
        }
    }

    fn reset(&mut self) {
        self.lp.reset();
    }
}

/// The Squash at its mild end, mixed in by the amount.
struct Multiband {
    squash: Squash,
    dry: Vec<StereoSample>,
    stretch: f32,
}

impl Stage for Multiband {
    fn process(&mut self, buf: &mut [StereoSample], _c: &Ctl, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let n = buf.len().min(self.dry.len());
        self.dry[..n].copy_from_slice(&buf[..n]);
        self.squash.process(&mut buf[..n], MULTIBAND_SQUASH, false, self.stretch);
        let m = MULTIBAND_MIX * amount;
        for (s, d) in buf[..n].iter_mut().zip(&self.dry[..n]) {
            s.left = d.left + (s.left - d.left) * m;
            s.right = d.right + (s.right - d.right) * m;
        }
    }

    fn reset(&mut self) {}
}

fn build(kind: StageKind, sr: f32) -> Box<dyn Stage> {
    match kind {
        StageKind::Filter => Box::new(Filter { s: [Svf::default(); 2] }),
        StageKind::Emphasis => Box::new(Emphasis { bands: [Biquad::default(); 3], movers: Movers::default(), de: false }),
        StageKind::Deemphasis => Box::new(Emphasis { bands: [Biquad::default(); 3], movers: Movers::default(), de: true }),
        StageKind::Tame => Box::new(Tame { shelf: Biquad::default(), back: false }),
        StageKind::Untame => Box::new(Tame { shelf: Biquad::default(), back: true }),
        StageKind::Drive => Box::new(Drive { mb: MbDist::new(sr) }),
        StageKind::Asym => Box::new(Asym { dc: [0.0; 2], os: Oversampler::default() }),
        StageKind::Shaper => Box::new(Shaper { os: Oversampler::default() }),
        StageKind::Notch => Box::new(Notch { gaps: [Gap::default(); 2], bells: [Biquad::default(); 2], shoulders: [[Biquad::default(); 2]; 2], movers: Movers::default() }),
        StageKind::Air => Box::new(Air { bands: [Biquad::default(); 3], movers: Movers::default() }),
        StageKind::Comb => Box::new(Comb { line: Delay::new((sr / COMB_LOWEST_HZ) as usize + 4), damp: [0.0; 2] }),
        StageKind::Phaser => Box::new(Phaser { z: [[0.0; 2]; 4], last: [0.0; 2], movers: Movers::default() }),
        StageKind::Chorus => Box::new(Chorus { line: Delay::new((sr * CHORUS_MAX_SECS) as usize + 4) }),
        StageKind::Post => Box::new(Post { lp: Biquad::default() }),
        StageKind::Multiband => Box::new(Multiband { squash: Squash::new(sr), dry: vec![StereoSample::silence(); CHUNK], stretch: 1.0 }),
    }
}

// --- Voicing ---------------------------------------------------------------

/// Letter `letter`'s voicing (0..=3 for A..D) for `sound_seed` at
/// Contrast `contrast` (D49's shape): A the seed's own; B moved away by
/// Contrast, at 1 its bands turned and placed an octave off, its ADSR the
/// most different shape and its attack and decay at another speed; C a
/// mild move off A; D B's, further in a direction of its own. At Contrast
/// 0 every letter plays A's.
pub fn voice(sound_seed: u32, letter: u8, contrast: f32) -> Voice {
    let h = |l: u8, k: u8| rng::hash(sound_seed, l, k, 0, S_VOICE);
    let u = |l: u8, k: u8| rng::unit_f32(h(l, k));
    let pick = |l: u8, k: u8, n: u32| rng::below(h(l, k), n) as usize;
    let sign = |l: u8, k: u8| if u(l, k) < 0.5 { -1.0 } else { 1.0 };
    let c = contrast.clamp(0.0, 1.0);
    let shape = pick(0, 1, ADSR_SHAPES.len() as u32);
    let jitter = |k: u8| 1.0 + ADSR_JITTER * (2.0 * u(0, k) - 1.0);
    let base = ADSR_SHAPES[shape];
    let own = Adsr {
        attack: base.attack * jitter(2),
        decay: base.decay * jitter(3),
        sustain: (base.sustain * jitter(4)).min(0.95),
        release: base.release * jitter(5),
        slope: base.slope * jitter(11) * sign(0, 12),
    };
    let a = Voice {
        seed: h(0, 9) as u32,
        flip: 0.0,
        shift: 0.0,
        adsr: own,
        speed: 1.0,
        sweep: SWEEP_BEATS[pick(0, 13, SWEEP_BEATS.len() as u32)],
        filter_low: FILTER_HZ.0 * (FILTER_HZ.1 / FILTER_HZ.0).powf(u(0, 14)),
        eq_ratio: [6.0, 8.0, 12.0, 16.0][pick(0, 0, 4)],
        comb_harmonic: [2.0, 3.0, 4.0, 6.0][pick(0, 6, 4)],
        comb_sign: if u(0, 7) < 0.6 { 1.0 } else { -1.0 },
        fold: 0.15 + 0.6 * u(0, 8),
        follow: FOLLOW.0 + (FOLLOW.1 - FOLLOW.0) * u(0, 10),
        relations: std::array::from_fn(|k| RELATIONS[pick(0, 20 + k as u8, RELATIONS.len() as u32)]),
    };
    // Another letter's relations: each stage's changes to another one when
    // its draw falls under `t` (so all of them at 1).
    let related = |l: u8, t: f32| -> [Relation; MOVING_STAGES] {
        std::array::from_fn(|k| {
            let own = a.relations[k];
            if u(l, 20 + k as u8) < t {
                let i = RELATIONS.iter().position(|r| *r == own).unwrap_or(0);
                RELATIONS[(i + 1 + pick(l, 30 + k as u8, RELATIONS.len() as u32 - 1)) % RELATIONS.len()]
            } else {
                own
            }
        })
    };
    // B: the opposite shape (two along), turned, shifted, at another speed.
    let b_adsr = ADSR_SHAPES[(shape + 2) % ADSR_SHAPES.len()];
    let b_speed = B_SPEEDS[pick(1, 1, B_SPEEDS.len() as u32)];
    let b = |t: f32| Voice {
        flip: t,
        shift: t * B_SHIFT * sign(1, 2),
        adsr: own.lerp(b_adsr, t),
        speed: 1.0 + (b_speed - 1.0) * t,
        filter_low: if t > 0.5 { FILTER_HZ.0 * (FILTER_HZ.1 / FILTER_HZ.0).powf(u(1, 14)) } else { a.filter_low },
        sweep: if t > 0.5 { SWEEP_BEATS[(SWEEP_BEATS.iter().position(|b| *b == a.sweep).unwrap_or(0) + 1) % SWEEP_BEATS.len()] } else { a.sweep },
        eq_ratio: if t > 0.5 { [6.0, 8.0, 12.0, 16.0][pick(1, 0, 4)] } else { a.eq_ratio },
        comb_sign: if t > 0.5 { -a.comb_sign } else { a.comb_sign },
        relations: related(1, t),
        ..a
    };
    match letter {
        1 => b(c),
        2 => Voice {
            shift: C_SHARE * c * B_SHIFT * sign(2, 2),
            adsr: own.lerp(b_adsr, C_SHARE * c),
            speed: 1.0 + (b_speed - 1.0) * C_SHARE * c,
            relations: related(2, C_RELATION * c),
            ..a
        },
        3 => {
            let v = b(c);
            Voice { shift: v.shift + D_EXTRA * c * B_SHIFT * sign(3, 2), speed: v.speed * (1.0 + D_EXTRA * c), ..v }
        }
        _ => a,
    }
}

/// The stage weights for `sound_seed`: two racks blended.
pub fn rack(sound_seed: u32) -> [f32; ORDER.len()] {
    let (a, b, t) = rack_pick(sound_seed);
    std::array::from_fn(|i| RACKS[a].1[i] + (RACKS[b].1[i] - RACKS[a].1[i]) * t)
}

/// The two [`RACKS`] the sound seed blends and how far towards the second.
pub fn rack_pick(sound_seed: u32) -> (usize, usize, f32) {
    let h = |k: u8| rng::hash(sound_seed, 0, k, 0, S_RACK);
    let a = rng::below(h(0), RACKS.len() as u32) as usize;
    let b = (a + 1 + rng::below(h(1), RACKS.len() as u32 - 1) as usize) % RACKS.len();
    (a, b, 0.5 * rng::unit_f32(h(2)))
}

/// `n` sound seeds that play rack `rack` nearly alone (the make-up's
/// measurement, [`crate::makeup`]).
pub(crate) fn seeds_of_rack(rack: usize, n: usize) -> Vec<u32> {
    (0u32..).filter(|&s| rack_pick(s).0 == rack && rack_pick(s).2 < 0.1).take(n).collect()
}

// --- The chain -------------------------------------------------------------

/// The knobs the chain reads, per block.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeuroKnobs {
    /// The Neuro slider, 0..1.
    pub amount: f32,
    pub shape: f32,
    pub morph: f32,
    pub variation: f32,
    pub spread: f32,
    pub contrast: f32,
    pub sound_seed: u32,
    /// The letter playing (0..=3).
    pub letter: u8,
    /// The playing file's scanned spectrum and its root key, when the
    /// library scanned it (else the chain listens for it).
    pub timbre: Option<(crate::library::Timbre, u8)>,
}

/// The note the chain follows.
#[derive(Debug, Clone, Copy, Default)]
struct NoteState {
    /// Frames since its onset, and its length in frames (0: none yet).
    age: f64,
    gate: f64,
    /// The ADSR's peak (an accent opens it fully) and the note's draw.
    peak: f32,
    draw: f32,
}

/// One band of the balance guard: the processed band's share above
/// `hz` held under a limit by a high shelf there (measured through a
/// fourth-order highpass, so the mids below do not count).
#[derive(Debug, Clone, Copy)]
struct Guard {
    hz: f32,
    inp: Butter4,
    out: Butter4,
    shelf: Biquad,
    /// Energies (slow): the input above and in all, the output above and in all.
    e: [f32; 4],
    gain: f32,
}

impl Guard {
    fn new(hz: f32, sr: f32) -> Self {
        let hp = || {
            let mut b = Butter4::default();
            b.highpass(hz, sr);
            b
        };
        Self { hz, inp: hp(), out: hp(), shelf: Biquad::default(), e: [0.0; 4], gain: 1.0 }
    }

    /// Hold `wet`'s share above the band under `limit(input share)`
    /// (`dry` the input), following at `a`.
    fn run(&mut self, dry: &[StereoSample], wet: &mut [StereoSample], a: f32, sr: f32, limit: impl Fn(f32) -> f32) {
        let n = wet.len();
        let mut e = [0.0f32; 4];
        for (d, w) in dry.iter().zip(wet.iter()) {
            let hd = self.inp.run([d.left, d.right]);
            let hw = self.out.run([w.left, w.right]);
            e[0] += hd[0] * hd[0] + hd[1] * hd[1];
            e[1] += d.left * d.left + d.right * d.right;
            e[2] += hw[0] * hw[0] + hw[1] * hw[1];
            e[3] += w.left * w.left + w.right * w.right;
        }
        for (s, v) in self.e.iter_mut().zip(e) {
            *s += (v / n.max(1) as f32 - *s) * a;
        }
        let share = |hi: f32, all: f32| if all > 1e-12 { (hi / all).min(1.0) } else { 0.0 };
        let (input, output) = (share(self.e[0], self.e[1]), share(self.e[2], self.e[3]));
        let allowed = limit(input).clamp(1e-3, 1.0);
        // The highs' gain that brings their share to the allowed one (the
        // measured share is the band's before this shelf).
        let target = if output > allowed {
            let g2 = allowed * (1.0 - output) / (output * (1.0 - allowed)).max(1e-9);
            g2.sqrt().max(GUARD_MIN_GAIN)
        } else {
            1.0
        };
        self.gain += (target - self.gain) * a;
        if self.gain >= 0.999 {
            self.shelf.reset();
            return;
        }
        self.shelf.high_shelf(self.hz, 20.0 * self.gain.log10(), sr);
        for w in wet.iter_mut() {
            let y = self.shelf.run([w.left, w.right]);
            (w.left, w.right) = (y[0], y[1]);
        }
    }
}

/// The bass lane's Neuro stage.
pub struct Neuro {
    sr: f32,
    split: Lr4,
    /// The processed band's highpass at the split (the drives' difference
    /// tones stay out of the sub), the same split's low side on the dry
    /// band and its allpass on the sub, so the three sum flat when nothing
    /// is processed (the sub and the band met in antiphase at the split:
    /// −79 dB at 130 Hz, −8 dB at 110 and 150 Hz).
    clean: Lr4,
    dry_clean: Lr4,
    sub_ap: Lr4,
    /// The band's crossovers near the sub (the drive's low one, the low
    /// gap's edges) as allpasses on the sub, so the band's phase there is
    /// the sub's; the frequencies they are set at.
    align: [Lr4; 3],
    align_hz: [f32; 3],
    stages: Vec<(StageKind, Box<dyn Stage>)>,
    /// Whether the make-up ([`crate::makeup::neuro`]) is off (its table's
    /// measurement).
    pub(crate) raw: bool,
    upper: Vec<StereoSample>,
    dry_upper: Vec<StereoSample>,
    amount: f32,
    amount_a: f32,
    /// The pitch tracking: the target and the glided frequency, the glide's
    /// coefficient.
    target: f32,
    f0: f32,
    glide_a: f32,
    note: NoteState,
    /// The input envelope: fast and slow followers and their coefficients.
    fast: f32,
    slow: f32,
    level_a: (f32, f32),
    /// The source heard while playing: a highpass's state, the energies
    /// above it, of the band and of the sub (slow), their coefficient, the
    /// highpass's.
    bright_hp: f32,
    bright_hi: f32,
    bright_all: f32,
    bright_sub: f32,
    bright_a: f32,
    bright_k: f32,
    /// The balance guard: the growth above 1 kHz, the cap above 3 kHz.
    guards: [Guard; 2],
    guard_a: f32,
    /// The source of the chunk playing.
    source: Source,
    /// The rack the seed set up, and for which seed.
    weights: [f32; ORDER.len()],
    seed: Option<u32>,
    active: bool,
}

impl Neuro {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let mut split = Lr4::default();
        split.set(SUB_HZ, sr);
        let follow = |secs: f32| 1.0 - (-(CHUNK as f32) / (secs * sr)).exp();
        Self {
            sr,
            split,
            clean: split,
            dry_clean: split,
            sub_ap: split,
            align: [Lr4::default(); 3],
            align_hz: [0.0; 3],
            stages: ORDER.iter().map(|&k| (k, build(k, sr))).collect(),
            raw: false,
            upper: vec![StereoSample::silence(); CHUNK],
            dry_upper: vec![StereoSample::silence(); CHUNK],
            amount: 0.0,
            amount_a: follow(AMOUNT_SECS),
            target: 55.0,
            f0: 55.0,
            glide_a: 1.0,
            note: NoteState::default(),
            fast: 0.0,
            slow: 0.0,
            level_a: (follow(LEVEL_SECS.0), follow(LEVEL_SECS.1)),
            bright_hp: 0.0,
            bright_hi: 0.0,
            bright_all: 0.0,
            bright_sub: 0.0,
            bright_a: follow(BRIGHT_SECS),
            bright_k: (-TAU * BRIGHT_SPLIT_HZ / sr).exp(),
            guards: [Guard::new(GUARD_HZ, sr), Guard::new(GUARD_TOP_HZ, sr)],
            guard_a: follow(GUARD_SECS),
            source: Source::default(),
            weights: [0.0; ORDER.len()],
            seed: None,
            active: false,
        }
    }

    /// Process `buf` (one block of the lane) in place: `fired` the notes the
    /// block started (frame offset, event) and `beats_per_frame` the tempo
    /// (their gates are in ticks). RT-safe.
    pub fn process(&mut self, buf: &mut [StereoSample], fired: &[(u32, LiveEvent)], knobs: &NeuroKnobs, beats_per_frame: f64) {
        if self.seed != Some(knobs.sound_seed) {
            self.weights = rack(knobs.sound_seed);
            self.seed = Some(knobs.sound_seed);
        }
        let voice = voice(knobs.sound_seed, knobs.letter, knobs.contrast);
        let variation = knobs.variation.clamp(0.0, 1.0);
        let depth = VARIATION_DEPTH.0 + (VARIATION_DEPTH.1 - VARIATION_DEPTH.0) * variation;
        let var_speed = VARIATION_SPEED.0 + (VARIATION_SPEED.1 - VARIATION_SPEED.0) * variation;
        let mut events = fired.iter().filter(|(_, e)| e.kind == EventKind::NoteOn && e.key > 0 && e.vel > 0).peekable();
        let mut at = 0usize;
        while at < buf.len() {
            // A chunk ends at the next note, so each note starts its own.
            let mut end = (at + CHUNK).min(buf.len());
            while let Some((off, e)) = events.peek().copied() {
                let off = *off as usize;
                if off > at {
                    end = end.min(off);
                    break;
                }
                self.note_on(e, knobs, beats_per_frame);
                events.next();
            }
            self.amount += (knobs.amount.clamp(0.0, 1.0) - self.amount) * self.amount_a;
            if knobs.amount <= 0.0 && self.amount < 1e-4 {
                self.amount = 0.0;
                if self.active {
                    self.reset();
                }
            }
            let n = end - at;
            self.f0 = self.target + (self.f0 - self.target) * (1.0 - self.glide_a).powf(n as f32);
            if self.amount > 0.0 {
                self.active = true;
                // The movement's speed: a little faster with the amount,
                // Variation's and the letter's own.
                let speed = (1.0 + (AMOUNT_SPEED - 1.0) * self.amount) * var_speed * voice.speed;
                let (age, length) = ((self.note.age / self.sr as f64) as f32, (self.note.gate / self.sr as f64) as f32);
                let adsr = if self.note.gate > 0.0 { voice.adsr.at(age, length, speed) * self.note.peak } else { 0.0 };
                let motion = Motion { adsr, age, length, level: 0.0, note: self.note.draw * variation, morph: (knobs.morph - 0.5) * 2.0 };
                let high = if self.bright_all > 1e-9 { self.bright_hi / self.bright_all } else { 0.0 };
                let sub = if self.bright_all + self.bright_sub > 1e-9 { self.bright_sub / (self.bright_all + self.bright_sub) } else { 0.0 };
                // The shares as they sound, once the followers have heard enough.
                let heard = (self.bright_all + self.bright_sub > HEARD_ENERGY).then_some((sub, high));
                let source = match &knobs.timbre {
                    Some((t, root)) => Source::scanned(t, *root, self.f0, heard),
                    None => Source::of(self.f0, sub, high),
                };
                self.source = source;
                let c = Ctl { sr: self.sr, f0: self.f0, motion, spread: knobs.spread, voice, source };
                self.chunk(&mut buf[at..end], &c, depth);
            }
            self.note.age += n as f64;
            at = end;
        }
    }

    fn note_on(&mut self, e: &LiveEvent, knobs: &NeuroKnobs, beats_per_frame: f64) {
        self.target = 440.0 * ((e.key as f32 - 69.0) / 12.0).exp2();
        let tie = e.flags & flags::TIE != 0;
        let glide = if tie { crate::sampler::glide_secs(e.glide).max(TRACK_SECS) } else { TRACK_SECS };
        self.glide_a = 1.0 - (-1.0 / (glide * self.sr)).exp();
        let gate = (e.gate as f64 / TICKS_PER_BEAT / beats_per_frame.max(1e-12)).max(1.0);
        if tie && self.note.gate > 0.0 {
            // A slide carries the note on: it now ends with the slid-to one.
            self.note.gate = self.note.gate.max(self.note.age + gate);
            return;
        }
        if self.note.gate > 0.0 && self.note.age < self.note.gate {
            // Inside a sweep: it plays on through this note.
            return;
        }
        let sweep = voice(knobs.sound_seed, knobs.letter, knobs.contrast).sweep as f64 / beats_per_frame.max(1e-12);
        let gate = gate.max(sweep);
        let h = rng::hash(knobs.sound_seed, knobs.letter, e.step, e.key, S_NOTE);
        self.note = NoteState { age: 0.0, gate, peak: if e.flags & flags::ACCENT != 0 { 1.0 } else { 0.85 }, draw: 2.0 * rng::unit_f32(h) - 1.0 };
    }

    /// One chunk: split, the input envelope, the stages on the upper band,
    /// make-up, sum. `depth` scales every move (Variation).
    fn chunk(&mut self, buf: &mut [StereoSample], c: &Ctl, depth: f32) {
        let n = buf.len();
        let mut sub = [[0.0f32; 2]; CHUNK];
        let mut peak = 0.0f32;
        let (mut e_hi, mut e_all, mut e_sub) = (0.0f32, 0.0f32, 0.0f32);
        for (i, s) in buf.iter().enumerate() {
            let (lo, hi) = self.split.split([s.left, s.right]);
            sub[i] = lo;
            self.upper[i] = StereoSample { left: hi[0], right: hi[1] };
            self.dry_upper[i] = self.upper[i];
            peak = peak.max(hi[0].abs()).max(hi[1].abs());
            // The brightness: a one-pole highpass on the mono band.
            let m = 0.5 * (hi[0] + hi[1]);
            self.bright_hp = flush(self.bright_k * self.bright_hp + (1.0 - self.bright_k) * m);
            let h = m - self.bright_hp;
            e_hi += h * h;
            e_all += m * m;
            let ms = 0.5 * (lo[0] + lo[1]);
            e_sub += ms * ms;
        }
        self.bright_hi += (e_hi / n.max(1) as f32 - self.bright_hi) * self.bright_a;
        self.bright_all += (e_all / n.max(1) as f32 - self.bright_all) * self.bright_a;
        self.bright_sub += (e_sub / n.max(1) as f32 - self.bright_sub) * self.bright_a;
        // The input's envelope against its own average.
        self.fast += (peak - self.fast) * self.level_a.0;
        self.slow += (peak - self.slow) * self.level_a.1;
        let ratio = if self.slow > 1e-5 { self.fast / self.slow } else { 1.0 };
        let mut c = *c;
        c.motion.level = (ratio - 1.0).clamp(-1.0, 1.0);
        // The crossovers the band passes near the sub: the drive's low one
        // (moving with the register) and the low gap's edges (moving with
        // the note).
        let movers = relate(c.voice.seed, ST_NOTCH, c.voice.relations[ST_NOTCH as usize], &NOTCHES.map(|n| (n.range.0 * n.range.1).sqrt().log2()), &[-1.0, -1.0], false);
        let centre = Notch::centre(&c, 0, &movers[0]);
        let half = (0.5 * NOTCHES[0].width).exp2();
        let hz = [Drive::config(&c).xover.0, centre / half, (centre * half).max(centre / half * 1.05)];
        for k in 0..3 {
            if hz[k] != self.align_hz[k] {
                self.align[k].set(hz[k], self.sr);
                self.align_hz[k] = hz[k];
            }
        }
        // The louder parts of a note get more processing, the quieter less.
        let follow = 1.0 + (ratio.clamp(FOLLOW_RANGE.0, FOLLOW_RANGE.1) - 1.0) * c.voice.follow;
        // Variation scales the moves: the ADSR's swing around its middle,
        // the draw and Morph stay.
        c.motion.adsr = 0.5 + (c.motion.adsr - 0.5) * depth;
        // The amount comes in early (half way is already a clear chain) and
        // the rack's weights reach their stage in full before the top.
        let curve = self.amount.powf(0.8) * AMOUNT_REACH;
        for (k, (_, stage)) in self.stages.iter_mut().enumerate() {
            let kind = ORDER[k];
            stage.process(&mut self.upper[..n], &c, (curve * self.weights[k] * follow * c.source.stage(kind)).clamp(0.0, 1.0));
        }
        for w in self.upper[..n].iter_mut() {
            let (_, hi) = self.clean.split([w.left, w.right]);
            (w.left, w.right) = (hi[0], hi[1]);
        }
        self.guard(n);
        // The band's level from the (smoothed) amount and the racks (D64).
        let g = if self.raw { 1.0 } else { crate::makeup::neuro(self.seed.unwrap_or(0), self.amount) };
        for i in 0..n {
            let (d, w) = (self.dry_upper[i], self.upper[i]);
            // What the clean highpass took off the band, as it was dry, and
            // the sub in the same phase.
            let (low, _) = self.dry_clean.split([d.left, d.right]);
            let (a, b) = self.sub_ap.split(sub[i]);
            let mut under = [a[0] + b[0] + low[0], a[1] + b[1] + low[1]];
            for x in self.align.iter_mut() {
                let (l, h) = x.split(under);
                under = [l[0] + h[0], l[1] + h[1]];
            }
            buf[i].left = under[0] + w.left * g;
            buf[i].right = under[1] + w.right * g;
        }
    }

    /// The balance guard on the chunk's processed band (O3): the share above
    /// 1 kHz may grow [`GUARD_GROWTH`] times over the input's (less for a
    /// lead-like source, never held under [`GUARD_FLOOR`]), the share above
    /// 3 kHz stays under [`GUARD_TOP_CAP`].
    fn guard(&mut self, n: usize) {
        let lead = self.source.lead;
        let growth = GUARD_GROWTH.0 + (GUARD_GROWTH.1 - GUARD_GROWTH.0) * lead;
        let cap = GUARD_TOP_CAP.0 + (GUARD_TOP_CAP.1 - GUARD_TOP_CAP.0) * lead;
        let (a, sr) = (self.guard_a, self.sr);
        let [g1k, g3k] = &mut self.guards;
        g1k.run(&self.dry_upper[..n], &mut self.upper[..n], a, sr, |input| (input * growth).max(GUARD_FLOOR));
        // Its own growth under the cap; never less than the input had there.
        g3k.run(&self.dry_upper[..n], &mut self.upper[..n], a, sr, |input| (input * GUARD_TOP_GROWTH).max(GUARD_TOP_FLOOR).min(cap).max(input));
    }

    fn reset(&mut self) {
        for (_, s) in self.stages.iter_mut() {
            s.reset();
        }
        self.active = false;
    }

    /// The stages in chain order.
    pub fn order(&self) -> impl Iterator<Item = StageKind> + '_ {
        self.stages.iter().map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saw(hz: f32, secs: f32, sr: f32) -> Vec<StereoSample> {
        (0..(secs * sr) as usize)
            .map(|i| {
                let v = 0.5 * (2.0 * (i as f32 * hz / sr).fract() - 1.0);
                StereoSample { left: v, right: v }
            })
            .collect()
    }

    fn rms(b: &[StereoSample]) -> f32 {
        (b.iter().map(|s| 0.5 * (s.left * s.left + s.right * s.right)).sum::<f32>() / b.len() as f32).sqrt()
    }

    fn note(key: u8) -> LiveEvent {
        LiveEvent { kind: EventKind::NoteOn, key, vel: 100, gate: 96, ..LiveEvent::default() }
    }

    fn run(amount: f32, seed: u32, input: &[StereoSample]) -> Vec<StereoSample> {
        let sr = 48_000.0;
        let mut n = Neuro::new(sr);
        let knobs = NeuroKnobs { amount, shape: 0.3, morph: 0.5, variation: 0.5, spread: 0.5, contrast: 0.5, sound_seed: seed, letter: 0, timbre: None };
        let mut out = input.to_vec();
        for (k, block) in out.chunks_mut(512).enumerate() {
            let fired = if k % 8 == 0 { vec![(0u32, note(41))] } else { Vec::new() };
            n.process(block, &fired, &knobs, 175.0 / 60.0 / sr as f64);
        }
        out
    }

    fn ctl(seed: u32, adsr: f32) -> Ctl {
        let motion = Motion { adsr, age: 0.0, length: 0.2, level: 0.0, note: 0.0, morph: 0.0 };
        Ctl { sr: 48_000.0, f0: 55.0, motion, spread: 0.5, voice: voice(seed, 0, 0.5), source: Source::default() }
    }

    #[test]
    fn zero_passes_untouched() {
        let x = saw(87.3, 0.5, 48_000.0);
        assert_eq!(run(0.0, 3, &x), x);
    }

    #[test]
    fn full_amount_keeps_the_level_and_stays_finite() {
        let x = saw(87.3, 2.0, 48_000.0);
        for seed in 0..12 {
            let y = run(1.0, seed, &x);
            assert!(y.iter().all(|s| s.left.is_finite() && s.right.is_finite()), "seed {seed}");
            let (a, b) = (rms(&x[48_000..]), rms(&y[48_000..]));
            let db = 20.0 * (b / a).log10();
            // Unweighted: the make-up is K-weighted, and the gaps take
            // out the low mids it weighs less (about −2 .. −3 dB here).
            assert!(db.abs() < 4.0, "seed {seed}: {db:.1} dB");
            assert_ne!(y, x);
        }
    }

    #[test]
    fn it_repeats_exactly() {
        let x = saw(87.3, 0.5, 48_000.0);
        assert_eq!(run(0.8, 5, &x), run(0.8, 5, &x));
    }

    #[test]
    fn the_sub_passes_clean() {
        let sr = 48_000.0;
        let x: Vec<StereoSample> = (0..48_000).map(|i| { let v = 0.5 * (TAU * 45.0 * i as f32 / sr).sin(); StereoSample { left: v, right: v } }).collect();
        let y = run(1.0, 2, &x);
        let db = 20.0 * (rms(&y[24_000..]) / rms(&x[24_000..])).log10();
        assert!(db.abs() < 1.0, "{db:.2} dB");
    }

    #[test]
    fn the_split_sums_flat_around_the_sub() {
        // The sub and the processed band met in antiphase at the split
        // (−79 dB at 130 Hz): at a touch of Neuro and in full every sine
        // passes.
        let sr = 48_000.0;
        for hz in [60.0f32, 90.0, 110.0, 130.0, 150.0, 180.0, 250.0] {
            let x: Vec<StereoSample> = (0..48_000).map(|i| { let v = 0.3 * (TAU * hz * i as f32 / sr).sin(); StereoSample { left: v, right: v } }).collect();
            // (The sub takes the band's crossovers near it as allpasses; what
            // is left of the band's phase there makes up to about 2 dB. In
            // full only around the split: above it a sine is the band's, at
            // the level the make-up gives the probes, D64.)
            let amounts: &[f32] = if hz <= 180.0 { &[0.001, 1.0] } else { &[0.001] };
            for &amount in amounts {
                let y = run(amount, 2, &x);
                let db = 20.0 * (rms(&y[24_000..]) / rms(&x[24_000..])).log10();
                assert!(db.abs() < 2.0, "{hz} Hz at {amount}: {db:.2} dB");
            }
        }
    }

    #[test]
    fn the_comb_follows_the_note() {
        let mut c = ctl(1, 0.5);
        let low = Comb::delay(&c);
        c.f0 = 110.0;
        assert!((Comb::delay(&c) * 2.0 - low).abs() < 1e-3);
    }

    #[test]
    fn the_notches_stay_in_their_ranges() {
        for seed in 0..50 {
            let v = voice(seed, 0, 0.5);
            let movers = relate(v.seed, ST_NOTCH, v.relations[ST_NOTCH as usize], &[8.0, 11.0], &[-18.0, -20.0], false);
            for k in 0..2 {
                let (lo, hi) = NOTCHES[k].range;
                for adsr in [0.0, 0.5, 1.0] {
                    let hz = Notch::centre(&ctl(seed, adsr), k, &movers[k]);
                    assert!((lo - 1e-3..=hi + 1e-3).contains(&hz), "{hz}");
                }
            }
        }
    }

    #[test]
    fn relations_move_the_bands_together_apart_or_over_each_other() {
        let bases = [-1.0, 0.0, 1.26];
        let gains = [8.0, -18.0, 7.0];
        let v = voice(1, 0, 0.0);
        let at = |m: &Mover, adsr: f32, base: f32| base + m.octaves(&Motion { adsr, age: 0.0, length: 0.2, level: 0.0, note: 0.0, morph: 0.0 }, &v);
        let (mut crossed, mut together, mut apart) = (0, 0, 0);
        for seed in 0..60 {
            for rel in RELATIONS {
                let m = relate(seed, 0, rel, &bases, &gains, true);
                let pos = |adsr: f32| -> [f32; 3] { std::array::from_fn(|k| at(&m[k], adsr, bases[k])) };
                let (hi, lo) = (pos(1.0), pos(0.0));
                let moves: [f32; 3] = std::array::from_fn(|k| hi[k] - lo[k]);
                match rel {
                    Relation::Together => together += moves.iter().all(|d| d.signum() == moves[0].signum()) as u32,
                    Relation::Apart => apart += (moves[0] < 0.0 && moves[2] > 0.0) as u32,
                    Relation::Cross => {
                        // Some neighbouring peak and null swap their order.
                        crossed += (0..2).any(|i| (hi[i] - hi[i + 1]).signum() != (lo[i] - lo[i + 1]).signum()) as u32;
                    }
                }
            }
        }
        assert_eq!((together, apart, crossed), (60, 60, 60));
        // Notches cannot cross: they move apart instead.
        let m = relate(3, ST_NOTCH, Relation::Cross, &[8.0, 11.0], &[-18.0, -20.0], false);
        assert!(m[0].adsr_oct < 0.0 && m[1].adsr_oct > 0.0);
    }

    #[test]
    fn a_higher_brighter_source_moves_the_ranges_up_and_eases_the_drive() {
        let bass = Source::of(44.0, 0.9, 0.05);
        assert_eq!((bass.shift, bass.harsh, bass.lead), (0.0, 0.0, 0.0), "a bass plays the ranges as set");
        // A bass-like source played high moves the ranges far.
        assert!(Source::of(440.0, 0.9, 0.05).shift > 2.0);
        let lead = Source::of(220.0, 0.005, 0.5);
        assert!(lead.lead == 1.0 && lead.shift > 0.9 && lead.shift <= LEAD_MAX_OCT && lead.drive() < 0.7, "{lead:?}");
        let v = voice(1, 0, 0.0);
        let movers = relate(v.seed, ST_NOTCH, Relation::Together, &[8.0, 11.0], &[-18.0, -20.0], false);
        let at = |source: Source| Notch::centre(&Ctl { source, ..ctl(1, 0.5) }, 0, &movers[0]);
        assert!((at(lead) / at(bass)).log2() > 0.9);
    }

    #[test]
    fn a_scanned_lead_plays_higher_ranges_than_a_bass() {
        let t = |centroid: f32, sub: f32, over_1k: f32| crate::library::Timbre { centroid, bright: 0.0, flat: 0.2, sub, over_1k, version: crate::library::TIMBRE_VERSION };
        let (bass, lead) = (t(600.0, 0.95, 0.05), t(2400.0, 0.01, 0.5));
        let f0 = |key: f32| 440.0 * ((key - 69.0) / 12.0).exp2();
        assert_eq!(Source::scanned(&bass, 41, f0(41.0), None).shift, 0.0);
        let l = Source::scanned(&lead, 41, f0(41.0), None);
        assert!(l.shift > 0.9 && l.shift <= LEAD_MAX_OCT && l.lead == 1.0 && l.harsh == 1.0, "{l:?}");
        // O4: a lead gets less of the heavy drive, more of the subtle ones.
        assert!(l.stage(StageKind::Drive) < 1.0 && l.stage(StageKind::Asym) > 1.0);
        // O2: a harsh source's top goes down before the drive, half back after.
        let (pre, back) = (Tame::db(&l, 1.0, false), Tame::db(&l, 1.0, true));
        assert!(pre <= TAME_DB + 1e-3 && (back + 0.5 * pre).abs() < 1e-3);
        // The same file an octave up plays its ranges higher.
        let (lo, hi) = (Source::scanned(&bass, 41, f0(41.0), None), Source::scanned(&bass, 41, f0(65.0), None));
        assert!(hi.shift > lo.shift + 0.5, "{lo:?} {hi:?}");
    }

    #[test]
    fn the_oversampler_passes_the_band_and_keeps_the_level() {
        let sr = 48_000.0;
        let mut os = Oversampler::default();
        os.ready(sr);
        let x: Vec<f32> = (0..9600).map(|i| 0.5 * (TAU * 1000.0 * i as f32 / sr).sin()).collect();
        let y: Vec<f32> = x.iter().map(|v| os.run([*v, *v], |u| u)[0]).collect();
        let r = |v: &[f32]| (v.iter().map(|a| a * a).sum::<f32>() / v.len() as f32).sqrt();
        let db = 20.0 * (r(&y[4800..]) / r(&x[4800..])).log10();
        assert!(db.abs() < 0.2, "{db:.2} dB");
    }

    #[test]
    fn oversampling_cuts_the_aliases_of_a_hard_drive() {
        // A high note driven hard: the energy below its fundamental (where
        // only aliases land) is far less with the oversampler.
        let sr = 48_000.0;
        let f = 3_100.0;
        let x: Vec<f32> = (0..16_384).map(|i| 0.8 * (TAU * f * i as f32 / sr).sin()).collect();
        let hard = |v: f32| (v * 30.0).tanh();
        let plain: Vec<f32> = x.iter().map(|v| hard(*v)).collect();
        let mut os = Oversampler::default();
        os.ready(sr);
        let over: Vec<f32> = x.iter().map(|v| os.run([*v, *v], |u| [hard(u[0]), hard(u[1])])[0]).collect();
        // Energy below the fundamental: a crude DFT over 50 .. 2500 Hz.
        let below = |v: &[f32]| {
            let v = &v[4096..];
            (5..250).map(|k| {
                let hz = k as f32 * 10.0;
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (i, s) in v.iter().enumerate() {
                    let ph = TAU * hz * i as f32 / sr;
                    re += s * ph.cos();
                    im += s * ph.sin();
                }
                re * re + im * im
            }).sum::<f32>()
        };
        let (p, o) = (below(&plain), below(&over));
        assert!(o < 0.25 * p, "aliases {o:.3e} vs {p:.3e}");
    }

    #[test]
    fn boosts_keep_their_distance() {
        let mut hz = [1000.0, 1100.0, 1200.0];
        spread_boosts(&mut hz, &[6.0, -12.0, 5.0]);
        assert!((hz[2] / hz[0]).log2() >= BOOST_GAP_OCT - 1e-4 && hz[1] == 1100.0, "{hz:?}");
    }

    #[test]
    fn contrast_changes_the_relations() {
        let changed = |l: u8, c: f32| (0..200).map(|s| (0..MOVING_STAGES).filter(|&k| voice(s, l, c).relations[k] != voice(s, 0, c).relations[k]).count()).sum::<usize>();
        assert_eq!(changed(1, 0.0), 0);
        assert_eq!(changed(1, 1.0), 200 * MOVING_STAGES, "at full Contrast B moves every stage another way");
        assert!(changed(2, 1.0) < changed(1, 1.0) / 2 && changed(2, 1.0) > 0, "C less");
    }

    #[test]
    fn the_adsr_plays_out_within_the_note() {
        for shape in ADSR_SHAPES {
            for length in [0.05f32, 0.2, 1.0] {
                for speed in [0.4f32, 1.0, 2.5] {
                    assert_eq!(shape.at(length, length, speed), 0.0, "it ends with the note");
                    let peak = (1..100).map(|k| shape.at(length * k as f32 / 100.0, length, speed)).fold(0.0f32, f32::max);
                    assert!(peak > 0.5 && peak <= 1.0, "{peak}");
                }
            }
            // A stab moves through it faster: half way in, a long note is
            // still near its start where a short one has moved on.
            let early = |len: f32| shape.at(0.01, len, 1.0);
            assert!(early(0.05) != early(1.0));
        }
    }

    #[test]
    fn the_follower_opens_the_lowpass() {
        // Closed at the seed's low point, fully open at the sweep's peak on a
        // hit, open at no amount.
        assert!((Filter::cutoff(0.0, -1.0, 1.0, 800.0) - 800.0).abs() < 1.0);
        assert!((Filter::cutoff(1.0, 1.0, 1.0, 800.0) - FILTER_OPEN_HZ).abs() < 1.0);
        assert!(Filter::cutoff(0.0, -1.0, 0.0, 800.0) > 19_000.0);
        for seed in 0..50 {
            let low = voice(seed, 0, 0.5).filter_low;
            assert!((FILTER_HZ.0..=FILTER_HZ.1).contains(&low), "{low}");
        }
        let lead = Source { lead: 1.0, ..Source::default() };
        assert_eq!(lead.stage(StageKind::Filter), 0.0);
    }

    #[test]
    fn the_air_grows_over_a_note() {
        assert_eq!(Air::growth(0.0, 0.5, 1.0), 0.0);
        assert!(Air::growth(0.1, 0.5, 1.0) > Air::growth(0.1, 0.5, 0.0));
        assert!(Air::growth(0.1, 0.2, 0.0) > Air::growth(0.1, 1.0, 0.0), "a short note grows them sooner");
        assert_eq!(Air::growth(2.0, 0.5, 0.5), 1.0);
    }

    #[test]
    fn contrast_parts_the_letters() {
        for s in 0..200 {
            for l in 1..4 {
                assert_eq!(voice(s, l, 0.0), voice(s, 0, 0.0), "at Contrast 0 every letter plays A's");
            }
            let (a, b) = (voice(s, 0, 1.0), voice(s, 1, 1.0));
            assert!(b.flip == 1.0 && b.shift.abs() >= B_SHIFT && b.speed != 1.0 && b.adsr != a.adsr, "seed {s}: {b:?}");
            let c = voice(s, 2, 1.0);
            assert!(c.shift.abs() < b.shift.abs() && c.flip == 0.0, "C is the mild one");
        }
    }

    #[test]
    fn processing_does_not_allocate() {
        let sr = 48_000.0;
        let mut n = Neuro::new(sr);
        let knobs = NeuroKnobs { amount: 1.0, sound_seed: 1, ..NeuroKnobs::default() };
        let mut buf = saw(87.3, 0.05, sr);
        let fired = [(10u32, note(41)), (300, note(48))];
        let (_, allocs) = crate::test_support::count_allocs(|| n.process(&mut buf, &fired, &knobs, 1e-4));
        assert_eq!(allocs, 0);
    }
}
