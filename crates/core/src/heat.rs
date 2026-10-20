//! The Heat knob: clip, then bitcrush, then saturation on a lane's output,
//! and the drums' ring modulator (Morph).
//!
//! Heat clips from the first touch, its threshold falling all the way up
//! the knob. The tanh saturation comes in from a fifth and drives harder up
//! to the top, level-matched so the lane keeps about its loudness. From the
//! middle the bitcrush fades in: fewer bits and a held sample rate, mixed
//! in gradually. A tilt around [`TILT_HZ`] before the saturation (undone
//! after it) moves where it bites: low Tone drives the lows, high Tone the
//! highs. RT-safe, no allocation.

use crate::types::StereoSample;

// MUSICAL: first draft, tune by ear (user: clip hard enough, saturation
// that scales, a crush fading in from the middle, Tone moving the focus; the
// crush only a flavour: user, too far at 4 bits and 12 held samples).
/// Soft-clip threshold at Heat 0+ and at full Heat.
const CLIP_THRESHOLD: (f32, f32) = (0.95, 0.25);
/// Where the saturation and the crush start, as Heat values.
const SATURATE_FROM: f32 = 0.2;
const CRUSH_FROM: f32 = 0.5;
/// Saturation drive at full Heat, in dB, and its curve.
const DRIVE_DB: f32 = 30.0;
const DRIVE_CURVE: f32 = 1.3;
/// Asymmetry of the saturation at full Heat (even harmonics).
const ASYMMETRY: f32 = 0.2;
/// Level the saturation's make-up gain keeps (about −10 dBFS).
const MAKEUP_LEVEL: f32 = 0.3;
/// RMS of a sine against a square of the same peak.
const SQUARE_RMS: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Bits at the crush's start and at full Heat, and the most samples held.
const BITS: (f32, f32) = (12.0, 7.0);
const HOLD_MAX: f32 = 4.0;
/// The crush's share of the signal at full Heat.
const CRUSH_MIX_MAX: f32 = 0.5;
/// Tilt split frequency and the tilt at full Tone and full Heat, in dB.
pub const TILT_HZ: f32 = 700.0;
const TILT_DB: f32 = 12.0;
/// DC blocker cutoff.
const DC_HZ: f32 = 10.0;

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
    dc: OnePole,
    pre: OnePole,
    post: OnePole,
    /// One-pole coefficients of the tilt split and of the DC blocker.
    tilt_a: f32,
    dc_a: f32,
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
            dc: OnePole::default(),
            pre: OnePole::default(),
            post: OnePole::default(),
            tilt_a: one_pole(TILT_HZ, sample_rate),
            dc_a: one_pole(DC_HZ, sample_rate),
        }
    }

    /// Process `buf` in place at Heat `amount` 0..1 (0 = bypass). `tone`
    /// 0..1 tilts the saturation's focus (0.5 = flat).
    pub fn process(&mut self, buf: &mut [StereoSample], amount: f32, tone: f32) {
        let h = amount.clamp(0.0, 1.0);
        if h <= 0.0 {
            return;
        }
        let clip_t = CLIP_THRESHOLD.0 + (CLIP_THRESHOLD.1 - CLIP_THRESHOLD.0) * h;
        let sat = ((h - SATURATE_FROM) / (1.0 - SATURATE_FROM)).clamp(0.0, 1.0);
        let crush = ((h - CRUSH_FROM) / (1.0 - CRUSH_FROM)).clamp(0.0, 1.0);
        let drive = 10f32.powf(DRIVE_DB * sat.powf(DRIVE_CURVE) / 20.0);
        let bias = ASYMMETRY * sat;
        // Peaks matched at the make-up level, less the RMS a squarer wave gains.
        let makeup = MAKEUP_LEVEL / ((drive * MAKEUP_LEVEL + bias).tanh() - bias.tanh()).max(1e-6) * (1.0 - (1.0 - SQUARE_RMS) * sat);
        let steps = 2f32.powf(BITS.0 + (BITS.1 - BITS.0) * crush) / 2.0;
        let hold = 1 + (crush * crush * (HOLD_MAX - 1.0)).round() as u32;
        let crush_mix = CRUSH_MIX_MAX * crush * crush * (3.0 - 2.0 * crush);
        // Tilt: lows up and highs down for low Tone, the other way for high.
        let tilt = 10f32.powf(TILT_DB * sat * (2.0 * tone.clamp(0.0, 1.0) - 1.0) / 20.0);
        let (lo_g, hi_g) = (1.0 / tilt, tilt);
        for s in buf.iter_mut() {
            let mut x = [s.left, s.right].map(|v| soft_clip(v, clip_t));
            if crush_mix > 0.0 {
                if self.hold_left == 0 {
                    self.held = x.map(|v| (v * steps).round() / steps);
                    self.hold_left = hold;
                }
                self.hold_left -= 1;
                x = [0, 1].map(|c| x[c] + (self.held[c] - x[c]) * crush_mix);
            }
            if sat > 0.0 {
                let lo = self.pre.low(x, self.tilt_a);
                let tilted = [0, 1].map(|c| lo[c] * lo_g + (x[c] - lo[c]) * hi_g);
                let y = tilted.map(|v| ((drive * v + bias).tanh() - bias.tanh()) * makeup);
                let lo = self.post.low(y, self.tilt_a);
                let flat = [0, 1].map(|c| lo[c] / lo_g + (y[c] - lo[c]) / hi_g);
                // The asymmetry leaves a DC offset: block it.
                let dc = self.dc.low(flat, self.dc_a);
                x = [flat[0] - dc[0], flat[1] - dc[1]];
            }
            *s = StereoSample { left: flush(x[0]), right: flush(x[1]) };
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
// never above 30 %).
/// The ring modulator leaves the band below this alone (the fundamentals).
pub const RING_FROM_HZ: f32 = 400.0;
/// Most of the upper band the ring modulator takes.
pub const RING_MIX_MAX: f32 = 0.3;
/// Carrier shape: sine at 0 .. near square at 1 (tanh drive).
const RING_SQUARE_DRIVE: f32 = 8.0;

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
        Heat::new(48_000.0).process(&mut b, amount, tone);
        b
    }

    #[test]
    fn low_heat_leaves_quiet_signals_alone_and_rounds_peaks() {
        let quiet = sine(0.5, 4800);
        assert_eq!(heated(0.05, 0.5, &quiet), quiet, "below the threshold: transparent");
        assert!(heated(0.05, 0.5, &sine(1.5, 4800)).iter().all(|s| s.left.abs() < 1.0));
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
            // Distortion: what is left after the best-fit sine.
            let k = out[4800..].iter().zip(&input[4800..]).map(|(o, i)| o.left * i.left).sum::<f32>()
                / input[4800..].iter().map(|i| i.left * i.left).sum::<f32>();
            let residue: f32 = out[4800..].iter().zip(&input[4800..]).map(|(o, i)| (o.left - k * i.left).powi(2)).sum();
            assert!(residue > last, "Heat {h} distorts more");
            last = residue;
        }
    }

    #[test]
    fn the_crush_fades_in_from_the_middle() {
        let input = sine(0.3, 9600);
        let steps = |b: &[StereoSample]| b.windows(2).filter(|w| (w[0].left - w[1].left).abs() < 1e-4).count();
        let held: Vec<usize> = [0.45, 0.6, 0.8, 1.0].iter().map(|&h| steps(&heated(h, 0.5, &input))).collect();
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
