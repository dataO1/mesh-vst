//! The Heat knob: clip, then bitcrush, then saturation on a lane's output.
//!
//! Low settings only round off the peaks (a soft clip above a falling
//! threshold, transparent below it). From a third up a tanh saturation
//! drives harder, level-matched so the lane keeps its loudness. From two
//! thirds up the signal is also crushed: fewer bits and a held sample
//! rate. RT-safe, no allocation.

use crate::types::StereoSample;

// MUSICAL: first draft, tune by ear (user: clip softly and transparent,
// saturation in the middle, bitcrush on top).
/// Where the saturation and the crush start, as Heat values.
const SATURATE_FROM: f32 = 0.33;
const CRUSH_FROM: f32 = 0.66;
/// Soft-clip threshold at Heat 0+ and at [`SATURATE_FROM`] and above.
const CLIP_THRESHOLD: (f32, f32) = (0.95, 0.5);
/// Saturation drive at full Heat, in dB.
const DRIVE_DB: f32 = 18.0;
/// Asymmetry of the saturation at full Heat (even harmonics).
const ASYMMETRY: f32 = 0.15;
/// Level the saturation's make-up gain keeps (about −10 dBFS).
const MAKEUP_LEVEL: f32 = 0.3;
/// RMS of a sine against a square of the same peak.
const SQUARE_RMS: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Bits at the crush's start and at full Heat, and the most samples held.
const BITS: (f32, f32) = (14.0, 5.0);
const HOLD_MAX: f32 = 8.0;
/// DC blocker pole (about 10 Hz at 48 kHz).
const DC_POLE: f32 = 0.9987;

/// Heat's state per lane.
#[derive(Debug, Clone, Default)]
pub struct Heat {
    held: StereoSample,
    hold_left: u32,
    dc_in: StereoSample,
    dc_out: StereoSample,
}

impl Heat {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process `buf` in place at Heat `amount` 0..1 (0 = bypass).
    pub fn process(&mut self, buf: &mut [StereoSample], amount: f32) {
        let h = amount.clamp(0.0, 1.0);
        if h <= 0.0 {
            return;
        }
        let clip_t = CLIP_THRESHOLD.0 + (CLIP_THRESHOLD.1 - CLIP_THRESHOLD.0) * (h / SATURATE_FROM).min(1.0);
        let sat = ((h - SATURATE_FROM) / (1.0 - SATURATE_FROM)).clamp(0.0, 1.0);
        let crush = ((h - CRUSH_FROM) / (1.0 - CRUSH_FROM)).clamp(0.0, 1.0);
        let drive = 10f32.powf(DRIVE_DB * sat / 20.0);
        let bias = ASYMMETRY * sat;
        // Peaks matched at the make-up level, less the RMS a squarer wave gains.
        let makeup = MAKEUP_LEVEL / ((drive * MAKEUP_LEVEL + bias).tanh() - bias.tanh()).max(1e-6) * (1.0 - (1.0 - SQUARE_RMS) * sat);
        let steps = 2f32.powf(BITS.0 + (BITS.1 - BITS.0) * crush) / 2.0;
        let hold = 1 + (crush * (HOLD_MAX - 1.0)).round() as u32;
        for s in buf.iter_mut() {
            let mut x = [s.left, s.right].map(|v| soft_clip(v, clip_t));
            if crush > 0.0 {
                if self.hold_left == 0 {
                    self.held = StereoSample { left: (x[0] * steps).round() / steps, right: (x[1] * steps).round() / steps };
                    self.hold_left = hold;
                }
                self.hold_left -= 1;
                x = [self.held.left, self.held.right];
            }
            if sat > 0.0 {
                x = x.map(|v| ((drive * v + bias).tanh() - bias.tanh()) * makeup);
                // The asymmetry leaves a DC offset: block it.
                let out = StereoSample {
                    left: x[0] - self.dc_in.left + DC_POLE * self.dc_out.left,
                    right: x[1] - self.dc_in.right + DC_POLE * self.dc_out.right,
                };
                self.dc_in = StereoSample { left: x[0], right: x[1] };
                self.dc_out = out;
                x = [out.left, out.right];
            }
            *s = StereoSample { left: x[0], right: x[1] };
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amp: f32, n: usize) -> Vec<StereoSample> {
        (0..n).map(|i| amp * (i as f32 * 0.05).sin()).map(|v| StereoSample { left: v, right: v }).collect()
    }

    fn rms(b: &[StereoSample]) -> f32 {
        (b.iter().map(|s| s.left * s.left).sum::<f32>() / b.len() as f32).sqrt()
    }

    #[test]
    fn low_heat_leaves_quiet_signals_alone_and_rounds_peaks() {
        let mut quiet = sine(0.5, 4800);
        let orig = quiet.clone();
        Heat::new().process(&mut quiet, 0.2);
        assert_eq!(quiet, orig, "below the threshold: transparent");
        let mut loud = sine(1.5, 4800);
        Heat::new().process(&mut loud, 0.2);
        assert!(loud.iter().all(|s| s.left.abs() < 1.0));
    }

    #[test]
    fn full_heat_saturates_and_crushes_at_about_the_same_loudness() {
        let input = sine(0.3, 48_000);
        let mut hot = input.clone();
        Heat::new().process(&mut hot, 1.0);
        assert!(hot.iter().all(|s| s.left.is_finite()));
        let db = 20.0 * (rms(&hot[4800..]) / rms(&input[4800..])).log10();
        assert!(db.abs() < 3.0, "{db} dB");
        // Held samples: runs of (nearly, after the DC blocker) equal values.
        let runs = hot.windows(2).filter(|w| (w[0].left - w[1].left).abs() < 1e-3).count();
        assert!(runs > hot.len() / 2, "{runs}");
    }
}
