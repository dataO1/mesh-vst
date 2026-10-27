//! The Heat knob: clip, then bitcrush, then saturation on a lane's output,
//! and the drums' ring modulator (Morph).
//!
//! Heat clips from the first touch, its threshold falling all the way up
//! the knob, the crush comes last. The saturation is multiband (D54, [`crate::mbdist`]): it comes
//! in from a fifth and drives harder up to the top, each band through its
//! own curve, never all wet, made up from the knobs ([`crate::makeup`],
//! D64) so the lane keeps about its
//! loudness. The bass: subtle tape on the low band (the sub stays solid),
//! a tube then a slight tanh on the mids, a soft top; the drums: tape on
//! the kick's band, a drive then a slight hard clip on the snare's, a
//! slight tube on top, all mixed in under the clean hits so the transients
//! stay. From the middle the bitcrush fades in: fewer bits and a
//! held sample rate, mixed in gradually. A tilt around [`TILT_HZ`] before
//! the saturation (undone after it) and the bands' drives move where it
//! bites: low Tone drives the lows, high Tone the highs. RT-safe, no
//! allocation.

use crate::makeup::Ramp;
use crate::mbdist::{BandConfig, Curve, MbConfig, MbDist, Pass};
use crate::types::StereoSample;

// MUSICAL: first draft, tune by ear (user: clip hard enough, saturation
// that scales, a crush fading in from the middle, Tone moving the focus; the
// crush only a flavour: user, too far at 4 bits and 12 held samples).
/// Soft-clip threshold at Heat 0+ and at full Heat.
const CLIP_THRESHOLD: (f32, f32) = (0.95, 0.25);
/// Where the saturation and the crush start, as Heat values.
const SATURATE_FROM: f32 = 0.2;
const CRUSH_FROM: f32 = 0.5;
/// The saturation's curve over Heat.
const DRIVE_CURVE: f32 = 1.3;
// MUSICAL: tune by ear (user, 2026-10-23: very subtle tape on the low
// end up to about 150 Hz, the mids to 1 kHz in two stages, a character
// drive that grows highs from the mids and a slight symmetric one that
// brings it out, the highs only slightly; Heat like the neuro drive with
// slightly different curves). The bands at full Heat.
const BASS_HEAT: MbConfig = MbConfig {
    xover: (150.0, 1000.0),
    bands: [
        BandConfig { passes: [Pass { curve: Curve::Tape, drive_db: 6.0, wet: 0.45 }, Pass::OFF], contain: 0.0, oversample: 1 },
        BandConfig {
            passes: [Pass { curve: Curve::Tube, drive_db: 24.0, wet: 0.7 }, Pass { curve: Curve::Tanh, drive_db: 10.0, wet: 0.4 }],
            contain: 6.0,
            oversample: 2,
        },
        BandConfig { passes: [Pass { curve: Curve::Tanh, drive_db: 8.0, wet: 0.3 }, Pass::OFF], contain: 0.0, oversample: 2 },
    ],
};
const DRUM_HEAT: MbConfig = MbConfig {
    xover: (150.0, 1200.0),
    bands: [
        BandConfig { passes: [Pass { curve: Curve::Tape, drive_db: 8.0, wet: 0.35 }, Pass::OFF], contain: 0.0, oversample: 1 },
        BandConfig {
            passes: [Pass { curve: Curve::Drive, drive_db: 18.0, wet: 0.35 }, Pass { curve: Curve::Hard, drive_db: 8.0, wet: 0.25 }],
            contain: 4.0,
            oversample: 4,
        },
        BandConfig { passes: [Pass { curve: Curve::Tube, drive_db: 6.0, wet: 0.2 }, Pass::OFF], contain: 0.0, oversample: 2 },
    ],
};
/// How far Tone moves drive between the low and the high band (dB at full
/// Tone either way, the mids stay).
const TONE_BAND_DB: f32 = 9.0;
/// Frames Heat works on at a time (its scratch).
const HEAT_CHUNK: usize = 64;
/// Bits at the crush's start and at full Heat, and the most samples held.
const BITS: (f32, f32) = (12.0, 7.0);
const HOLD_MAX: f32 = 4.0;
/// The crush's share of the signal at full Heat.
const CRUSH_MIX_MAX: f32 = 0.5;
/// Tilt split frequency and the tilt at full Tone and full Heat, in dB.
pub const TILT_HZ: f32 = 700.0;
const TILT_DB: f32 = 12.0;

/// One-pole lowpass state per channel.
#[derive(Debug, Clone, Copy, Default)]
struct OnePole {
    z: [f32; 2],
}

impl OnePole {
    #[inline]
    fn low(&mut self, x: [f32; 2], a: f32) -> [f32; 2] {
        for (z, v) in self.z.iter_mut().zip(x) {
            *z = flush(*z + a * (v - *z));
        }
        self.z
    }
}

/// Heat's state per lane.
#[derive(Debug, Clone)]
pub struct Heat {
    held: [f32; 2],
    hold_left: u32,
    pre: OnePole,
    post: OnePole,
    /// One-pole coefficient of the tilt split.
    tilt_a: f32,
    /// The make-up from the knobs (D64), and whether it is off (the
    /// tables' measurement).
    makeup: Ramp,
    pub(crate) raw: bool,
    mb: MbDist,
}

impl Default for Heat {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

fn one_pole(hz: f32, sample_rate: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * hz / sample_rate.max(1.0)).exp()
}

impl Heat {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            held: [0.0; 2],
            hold_left: 0,
            pre: OnePole::default(),
            post: OnePole::default(),
            tilt_a: one_pole(TILT_HZ, sample_rate),
            makeup: Ramp::default(),
            raw: false,
            mb: MbDist::new(sample_rate),
        }
    }

    /// Process `buf` in place at Heat `amount` 0..1 (0 = bypass). `tone`
    /// 0..1 tilts the saturation's focus (0.5 = flat). On the `drums` the
    /// transients stay: the clip only shaves the very peaks at every Heat
    /// (the notes come in matched, D64) and the saturation
    /// is mixed in under the clean hits.
    pub fn process(&mut self, buf: &mut [StereoSample], amount: f32, tone: f32, drums: bool) {
        let h = amount.clamp(0.0, 1.0);
        if h <= 0.0 {
            self.makeup = Ramp::default();
            return;
        }
        // Squared: the low end of the knob only shaves the very peaks.
        let clip_t = if drums { CLIP_THRESHOLD.0 } else { CLIP_THRESHOLD.0 + (CLIP_THRESHOLD.1 - CLIP_THRESHOLD.0) * h * h };
        let sat = ((h - SATURATE_FROM) / (1.0 - SATURATE_FROM)).clamp(0.0, 1.0);
        let crush = ((h - CRUSH_FROM) / (1.0 - CRUSH_FROM)).clamp(0.0, 1.0);
        let steps = 2f32.powf(BITS.0 + (BITS.1 - BITS.0) * crush) / 2.0;
        // The drums keep their top: bits only, no held sample rate.
        let hold = if drums { 1 } else { 1 + (crush * crush * (HOLD_MAX - 1.0)).round() as u32 };
        let crush_mix = CRUSH_MIX_MAX * crush * crush * (3.0 - 2.0 * crush);
        // Tilt: lows up and highs down for low Tone, the other way for high;
        // the bands' drives follow it.
        let t = 2.0 * tone.clamp(0.0, 1.0) - 1.0;
        let tilt = 10f32.powf(TILT_DB * sat * t / 20.0);
        let (lo_g, hi_g) = (1.0 / tilt, tilt);
        let band_tilt = [-TONE_BAND_DB * sat * t, 0.0, TONE_BAND_DB * sat * t];
        let cfg = if drums { &DRUM_HEAT } else { &BASS_HEAT };
        let drive = sat.powf(DRIVE_CURVE);
        // The level from the knobs (D64), moving over the buffer.
        let to = if self.raw { 1.0 } else { crate::makeup::heat(drums, tone, h) };
        let makeup = self.makeup.over(to, buf.len());
        for (chunk, block) in buf.chunks_mut(HEAT_CHUNK).enumerate() {
            for s in block.iter_mut() {
                let mut x = [s.left, s.right].map(|v| soft_clip(v, clip_t));
                if sat > 0.0 {
                    let lo = self.pre.low(x, self.tilt_a);
                    x = [0, 1].map(|c| lo[c] * lo_g + (x[c] - lo[c]) * hi_g);
                }
                (s.left, s.right) = (x[0], x[1]);
            }
            if sat > 0.0 {
                self.mb.process(block, cfg, drive, band_tilt);
            }
            for (k, s) in block.iter_mut().enumerate() {
                let mut y = [s.left, s.right];
                if sat > 0.0 {
                    let lo = self.post.low(y, self.tilt_a);
                    y = [0, 1].map(|c| lo[c] / lo_g + (y[c] - lo[c]) / hi_g);
                }
                // The crush on the driven sound (its steps stay sharp).
                if crush_mix > 0.0 {
                    if self.hold_left == 0 {
                        self.held = y.map(|v| (v * steps).round() / steps);
                        self.hold_left = hold;
                    }
                    self.hold_left -= 1;
                    y = [0, 1].map(|c| y[c] + (self.held[c] - y[c]) * crush_mix);
                }
                let g = makeup(chunk * HEAT_CHUNK + k);
                *s = StereoSample { left: flush(y[0] * g), right: flush(y[1] * g) };
            }
        }
    }
}

/// Zero for values too small to hear, so filter tails never go subnormal.
#[inline]
fn flush(v: f32) -> f32 {
    if v.abs() < 1e-20 { 0.0 } else { v }
}

/// Unchanged below `t`, rounded off above it towards 1.
fn soft_clip(x: f32, t: f32) -> f32 {
    let a = x.abs();
    if a <= t {
        return x;
    }
    let room = (1.0 - t).max(1e-3);
    x.signum() * (t + room * ((a - t) / room).tanh())
}

// MUSICAL: first draft, tune by ear (user: the lows stay clean, the mix
// never above 30 %; 2026-10-27: up to half, so Morph is heard).
/// The ring modulator leaves the band below this alone (the fundamentals).
pub const RING_FROM_HZ: f32 = 400.0;
/// Most of the upper band the ring modulator takes.
pub const RING_MIX_MAX: f32 = 0.5;
/// Carrier shape: sine at 0 .. near square at 1 (tanh drive).
const RING_SQUARE_DRIVE: f32 = 8.0;

/// Below this the monoizer sums the channels.
pub const MONO_BELOW_HZ: f32 = 80.0;

/// The monoizer: the side signal below [`MONO_BELOW_HZ`] goes (two one-pole
/// highpasses on the side, 12 dB/oct), so the lows play in mono and the
/// rest keeps its width.
#[derive(Debug, Clone)]
pub struct MonoLows {
    a: f32,
    low: [OnePole; 2],
}

impl MonoLows {
    pub fn new(sample_rate: f32) -> Self {
        Self { a: one_pole(MONO_BELOW_HZ, sample_rate), low: [OnePole::default(); 2] }
    }

    pub fn process(&mut self, buf: &mut [StereoSample]) {
        for s in buf.iter_mut() {
            let mid = 0.5 * (s.left + s.right);
            let mut side = 0.5 * (s.left - s.right);
            for f in self.low.iter_mut() {
                side -= f.low([side, 0.0], self.a)[0];
            }
            s.left = flush(mid + side);
            s.right = flush(mid - side);
        }
    }
}

impl Default for MonoLows {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

/// A ring modulator on the band above [`RING_FROM_HZ`]: that band times a
/// carrier, mixed with the dry signal; the lows pass clean.
#[derive(Debug, Clone)]
pub struct RingMod {
    phase: f32,
    sample_rate: f32,
    split_a: f32,
    /// Three one-pole highpasses in a row: 18 dB/oct.
    split: [OnePole; 3],
}

impl Default for RingMod {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl RingMod {
    pub fn new(sample_rate: f32) -> Self {
        Self { phase: 0.0, sample_rate: sample_rate.max(1.0), split_a: one_pole(RING_FROM_HZ, sample_rate), split: [OnePole::default(); 3] }
    }

    /// Process `buf` in place: carrier `hz`, wet share `mix` (at most
    /// [`RING_MIX_MAX`]), carrier `shape` 0 (sine) .. 1 (near square).
    /// `at_secs` is the song time of the first frame: the carrier's phase
    /// follows the song, so a render repeats exactly.
    pub fn process(&mut self, buf: &mut [StereoSample], hz: f32, mix: f32, shape: f32, at_secs: f64) {
        let mix = mix.clamp(0.0, RING_MIX_MAX);
        if mix <= 0.0 {
            return;
        }
        self.phase = ((at_secs * hz as f64).rem_euclid(1.0) * std::f64::consts::TAU) as f32;
        let step = std::f32::consts::TAU * hz / self.sample_rate;
        let drive = 1.0 + (RING_SQUARE_DRIVE - 1.0) * shape.clamp(0.0, 1.0);
        let norm = 1.0 / drive.tanh();
        for s in buf.iter_mut() {
            let x = [s.left, s.right];
            let mut hi = x;
            for f in self.split.iter_mut() {
                let lo = f.low(hi, self.split_a);
                hi = [hi[0] - lo[0], hi[1] - lo[1]];
            }
            let c = (drive * self.phase.sin()).tanh() * norm;
            let out = [0, 1].map(|k| x[k] + (hi[k] * c - hi[k]) * mix);
            *s = StereoSample { left: flush(out[0]), right: flush(out[1]) };
            self.phase = (self.phase + step) % std::f32::consts::TAU;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amp: f32, n: usize) -> Vec<StereoSample> {
        (0..n).map(|i| amp * (i as f32 * 0.05).sin()).map(|v| StereoSample { left: v, right: v }).collect()
    }

    fn rms(b: &[StereoSample]) -> f32 {
        (b.iter().map(|s| s.left * s.left).sum::<f32>() / b.len() as f32).sqrt()
    }

    fn heated(amount: f32, tone: f32, input: &[StereoSample]) -> Vec<StereoSample> {
        let mut b = input.to_vec();
        Heat::new(48_000.0).process(&mut b, amount, tone, false);
        b
    }

    #[test]
    fn low_heat_leaves_quiet_signals_alone_and_rounds_peaks() {
        let quiet = sine(0.5, 4800);
        assert_eq!(heated(0.05, 0.5, &quiet), quiet, "below the threshold: transparent");
        // Loud peaks are rounded off: a lower crest factor (the level is matched).
        let crest = |b: &[StereoSample]| b.iter().map(|s| s.left.abs()).fold(0.0f32, f32::max) / rms(b);
        let loud = sine(1.5, 4800);
        assert!(crest(&heated(0.05, 0.5, &loud)[2400..]) < 0.97 * crest(&loud[2400..]));
    }

    #[test]
    fn heat_keeps_about_the_loudness_and_adds_more_harmonics_as_it_rises() {
        let input = sine(0.3, 48_000);
        let mut last = 0.0;
        for h in [0.3, 0.6, 1.0] {
            let out = heated(h, 0.5, &input);
            assert!(out.iter().all(|s| s.left.is_finite()));
            let db = 20.0 * (rms(&out[4800..]) / rms(&input[4800..])).log10();
            assert!(db.abs() < 4.0, "Heat {h}: {db} dB");
            // Distortion: what is left after the best-fit sine at any phase
            // (the crossovers turn it).
            let n = out.len() - 4800;
            let (sn, cs): (Vec<f32>, Vec<f32>) = (0..n).map(|i| (((i + 4800) as f32 * 0.05).sin(), ((i + 4800) as f32 * 0.05).cos())).unzip();
            let o: Vec<f32> = out[4800..].iter().map(|s| s.left).collect();
            let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
            let (a, b) = (dot(&o, &sn) / dot(&sn, &sn), dot(&o, &cs) / dot(&cs, &cs));
            let residue: f32 = (0..n).map(|i| (o[i] - a * sn[i] - b * cs[i]).powi(2)).sum();
            assert!(residue > last, "Heat {h} distorts more");
            last = residue;
        }
    }

    #[test]
    fn drum_hits_keep_their_transients_at_full_heat() {
        // A hit every 250 ms: a click then a decaying 60 Hz body, peaking near 0 dBFS.
        let hits: Vec<StereoSample> = (0..96_000)
            .map(|i| {
                let t = (i % 12_000) as f32 / 48_000.0;
                let v = 0.9 * (-t / 0.004).exp() * (std::f32::consts::TAU * 3000.0 * t).sin().signum() * 0.5
                    + 0.6 * (-t / 0.08).exp() * (std::f32::consts::TAU * 60.0 * t).sin();
                StereoSample { left: v, right: v }
            })
            .collect();
        let crest = |b: &[StereoSample]| 20.0 * (b.iter().map(|s| s.left.abs()).fold(0.0f32, f32::max) / rms(b)).log10();
        let run = |h: f32, drums: bool| {
            let mut b = hits.clone();
            Heat::new(48_000.0).process(&mut b, h, 0.5, drums);
            crest(&b[48_000..])
        };
        let dry = crest(&hits[48_000..]);
        // (Since the multiband Heat, D54, the bass keeps its transients too.)
        for h in [0.55, 0.8, 1.0] {
            let drums = run(h, true);
            assert!(dry - drums < 4.0, "Heat {h}: dry {dry:.1}, drums {drums:.1} dB crest");
        }
    }

    #[test]
    fn the_crush_fades_in_from_the_middle() {
        let input = sine(0.3, 9600);
        let steps = |b: &[StereoSample]| b.windows(2).filter(|w| (w[0].left - w[1].left).abs() < 1e-4).count();
        let held: Vec<usize> = [0.45, 0.75, 0.9, 1.0].iter().map(|&h| steps(&heated(h, 0.5, &input))).collect();
        // Below the middle no held samples beyond chance, above it there are.
        assert!(held[1..].iter().all(|h| *h > held[0]), "{held:?}");
    }

    #[test]
    fn tone_moves_the_saturation_between_lows_and_highs() {
        // A low and a high sine: Tone changes which one distorts.
        let mix: Vec<StereoSample> = (0..48_000)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.25 * (std::f32::consts::TAU * 80.0 * t).sin() + 0.25 * (std::f32::consts::TAU * 5000.0 * t).sin()
            })
            .map(|v| StereoSample { left: v, right: v })
            .collect();
        assert_ne!(heated(0.9, 0.0, &mix), heated(0.9, 1.0, &mix));
    }

    #[test]
    fn the_monoizer_centres_the_lows_and_leaves_the_highs_wide() {
        // Opposite-phase sines: fully side, at 40 Hz and at 2 kHz.
        let side = |hz: f32| -> Vec<StereoSample> {
            (0..48_000).map(|i| 0.3 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).map(|v| StereoSample { left: v, right: -v }).collect()
        };
        let side_level = |b: &[StereoSample]| (b[4800..].iter().map(|s| (0.5 * (s.left - s.right)).powi(2)).sum::<f32>() / (b.len() - 4800) as f32).sqrt();
        for (hz, keeps) in [(40.0, false), (2000.0, true)] {
            let input = side(hz);
            let mut out = input.clone();
            MonoLows::new(48_000.0).process(&mut out);
            let kept = side_level(&out) / side_level(&input);
            if keeps {
                assert!(kept > 0.95, "{hz} Hz keeps its width: {kept}");
            } else {
                assert!(kept < 0.4, "{hz} Hz goes mono: {kept}");
            }
        }
    }

    #[test]
    fn the_ring_mod_leaves_the_lows_and_takes_part_of_the_highs() {
        let tone = |hz: f32| -> Vec<StereoSample> {
            (0..48_000).map(|i| 0.3 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).map(|v| StereoSample { left: v, right: v }).collect()
        };
        let change = |input: &[StereoSample], mix: f32| {
            let mut wet = input.to_vec();
            RingMod::new(48_000.0).process(&mut wet, 700.0, mix, 0.5, 0.0);
            let diff: Vec<StereoSample> = wet.iter().zip(input).map(|(w, i)| StereoSample { left: w.left - i.left, right: 0.0 }).collect();
            20.0 * (rms(&diff[4800..]) / rms(&input[4800..])).log10()
        };
        let (low, high) = (change(&tone(60.0), 1.0), change(&tone(4000.0), 1.0));
        assert!(low < -20.0, "60 Hz stays clean: {low} dB");
        assert!(high > -12.0, "4 kHz rings: {high} dB");
        // Capped: a mix of 1 rings no more than the cap.
        assert!((high - change(&tone(4000.0), RING_MIX_MAX)).abs() < 0.1);
        assert_eq!(change(&tone(4000.0), 0.0), f32::NEG_INFINITY);
    }
}
#[cfg(test)]
mod top_tests {
    use super::*;

    #[test]
    fn drum_heat_keeps_the_top() {
        // User, 2026-10-23: Heat on the drums lost the highs above 10 kHz
        // (the crush's held samples, the oversampler's cutoff). Noise hits.
        let sr = 48_000.0;
        let mut seed = 1u32;
        let x: Vec<StereoSample> = (0..96_000).map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let n = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            let env = (-((i % 12_000) as f32) / 2400.0).exp();
            let v = 0.5 * n * env;
            StereoSample { left: v, right: v }
        }).collect();
        let top = |b: &[StereoSample]| {
            let mut hp = crate::dsp::Butter4::default();
            hp.highpass(10_000.0, sr);
            let (mut e, mut a) = (0.0, 0.0);
            for s in &b[48_000..] { let h = hp.run([s.left, s.right]); e += h[0] * h[0]; a += s.left * s.left; }
            (10.0 * f32::log10(e), 10.0 * f32::log10(a))
        };
        // The top's share of the whole (the level is the make-up's, D64).
        let share = |b: &[StereoSample]| {
            let (t, a) = top(b);
            t - a
        };
        let dry = share(&x);
        for h in [0.3, 0.5, 0.75, 1.0] {
            let mut y = x.clone();
            let mut heat = Heat::new(sr);
            for b in y.chunks_mut(256) { heat.process(b, h, 0.5, true); }
            let t = share(&y);
            assert!(dry - t < 1.0, "Heat {h}: the top {:.1} dB down", dry - t);
        }
    }
}
