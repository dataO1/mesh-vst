//! Multiband distortion (design D54): three bands, each driven through its
//! own curve, a processor configured per use ([`MbConfig`]: the neuro
//! chain's drive, the bass's Heat, the drums' Heat).
//!
//! Two Linkwitz-Riley splits cut the signal into a low, a mid and a high
//! band (the low band passes an allpass of the upper split, so the three
//! sum flat when nothing distorts). Per band:
//!
//! - **Level:** the band's level is followed and its input normalised to a
//!   working level before the curve (and the makeup undoes it after), so a
//!   quiet band distorts as much as a loud one at the same drive, and a
//!   band with little of the signal gets less wet ([`PRESENCE`]): there is
//!   nothing there to grow, only noise.
//! - **Passes** ([`Pass`]): one or two curves in series ([`Curve`]: tape,
//!   a drive, tanh, an asymmetric tube, a sine fold, a hard clip), each
//!   with its drive in dB and its wet, so a band can be driven in stages
//!   (a character drive, then a slight symmetric one to bring it out).
//! - **Wet:** never all of it ([`MAX_WET`]): a stronger drive mixed in under
//!   the dry signal sounds better than a weaker one in full (user).
//! - **Containment:** a lowpass on the band's wet signal at a multiple of
//!   the band's top, so its harmonics do not spray into the top octaves.
//! - **Oversampling:** 1×, 2× or 4× per band.
//!
//! Distorting the bands apart also keeps the mids and the highs from
//! intermodulating. RT-safe: no allocation.

use crate::dsp::{flush, mix, Butter4, Oversample};
use crate::squash::Lr4;
use crate::types::StereoSample;

/// A band's distortion curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    #[default]
    /// Soft saturation, odd harmonics.
    Tanh,
    /// Asymmetric (the positive side clips harder): even harmonics, warm.
    Tube,
    /// A sine fold: many partials, the growl.
    Fold,
    /// A hard clip: square-ish, aggressive.
    Hard,
    /// Tape: a soft cubic knee, slightly biased (a touch of even
    /// harmonics), the gentlest.
    Tape,
    /// A drive pedal's diodes: an exponential knee, harder on the positive
    /// side, its kink at zero breeds upper partials (the mids grow highs).
    Drive,
}

impl Curve {
    /// The RMS of a sine of `peak` through the curve (16 points).
    fn sine_rms(self, peak: f32) -> f32 {
        let ms = (0..16).map(|k| self.shape(peak * (std::f32::consts::TAU * (k as f32 + 0.5) / 16.0).sin()).powi(2)).sum::<f32>() / 16.0;
        ms.sqrt()
    }

    #[inline]
    fn shape(self, v: f32) -> f32 {
        match self {
            Curve::Tanh => v.tanh(),
            Curve::Tube => {
                if v >= 0.0 {
                    v.tanh()
                } else {
                    TUBE_SOFT * (v / TUBE_SOFT).tanh()
                }
            }
            Curve::Fold => (v * std::f32::consts::FRAC_PI_2).sin(),
            Curve::Hard => v.clamp(-1.0, 1.0),
            Curve::Tape => tape(v + TAPE_BIAS) - tape(TAPE_BIAS),
            Curve::Drive => {
                if v >= 0.0 {
                    1.0 - (-v).exp()
                } else {
                    -DIODE_SOFT * (1.0 - (v / DIODE_SOFT).exp())
                }
            }
        }
    }

    /// Whether the curve leaves an offset (blocked after it).
    fn asymmetric(self) -> bool {
        matches!(self, Curve::Tube | Curve::Tape | Curve::Drive)
    }
}

/// The tape knee: cubic up to 1.5, flat at 1 beyond.
#[inline]
fn tape(v: f32) -> f32 {
    let c = v.clamp(-1.5, 1.5);
    c - c * c * c / 6.75
}

/// One curve in a band's series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pass {
    pub curve: Curve,
    /// The drive at full amount, dB.
    pub drive_db: f32,
    /// The wet share at full amount (capped at [`MAX_WET`]); 0: the pass is off.
    pub wet: f32,
}

impl Pass {
    pub const OFF: Pass = Pass { curve: Curve::Tanh, drive_db: 0.0, wet: 0.0 };
}

/// One band's setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandConfig {
    /// The curves in series (a second [`Pass::OFF`] for one stage).
    pub passes: [Pass; 2],
    /// The containment lowpass, a multiple of the band's top (0: none).
    pub contain: f32,
    /// Oversampling factor (1, 2 or 4).
    pub oversample: u8,
}

/// The processor's setting: the two crossovers and the three bands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MbConfig {
    pub xover: (f32, f32),
    pub bands: [BandConfig; 3],
}

// MUSICAL: tune by ear. The most of a band's signal the wet takes; the level
// a band's input is normalised to before its curve, and the most and least
// that normalisation gains; the band's share of the signal from which it
// gets its full wet (below it less); the tube's and the drive's softer
// negative sides; the tape's bias; the level followers' time; the
// containment's top for the high band (Hz).
pub const MAX_WET: f32 = 0.85;
const WORK_LEVEL: f32 = 0.3;
const NORMALISE_RANGE: (f32, f32) = (0.25, 8.0);
pub const PRESENCE: f32 = 0.08;
const TUBE_SOFT: f32 = 0.6;
const DIODE_SOFT: f32 = 0.7;
const TAPE_BIAS: f32 = 0.08;
const LEVEL_SECS: f32 = 0.3;
const CONTAIN_TOP_HZ: f32 = 18_000.0;
/// How far a pass's drive moves before its makeup is measured again (dB).
const MAKEUP_STEP_DB: f32 = 0.1;

#[derive(Debug, Clone, Copy, Default)]
struct BandState {
    os: Oversample,
    contain: Butter4,
    contain_hz: f32,
    /// The band's mean square (slow) and its DC blocker.
    ms: f32,
    dc: [f32; 2],
    /// Per pass the curve and drive its makeup was measured for, and the
    /// makeup (measured again only when the drive moves by
    /// [`MAKEUP_STEP_DB`]).
    makeup: [(Option<(Curve, f32)>, f32); 2],
}

/// A multiband distortion's state.
#[derive(Debug, Clone)]
pub struct MbDist {
    sr: f32,
    lo_split: Lr4,
    hi_split: Lr4,
    /// The low band's allpass of the upper split (the bands sum flat).
    comp: Lr4,
    xover: (f32, f32),
    bands: [BandState; 3],
    level_a: f32,
    dc_a: f32,
}

impl MbDist {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        Self {
            sr,
            lo_split: Lr4::default(),
            hi_split: Lr4::default(),
            comp: Lr4::default(),
            xover: (0.0, 0.0),
            bands: [BandState::default(); 3],
            level_a: 1.0 - (-1.0 / (LEVEL_SECS * sr)).exp(),
            dc_a: 1.0 - (-std::f32::consts::TAU * 10.0 / sr).exp(),
        }
    }

    fn set_xover(&mut self, (lo, hi): (f32, f32)) {
        let hi = hi.max(lo * 1.5).min(0.4 * self.sr);
        let lo = lo.min(hi / 1.5);
        if self.xover != (lo, hi) {
            self.lo_split.set(lo, self.sr);
            self.hi_split.set(hi, self.sr);
            self.comp.set(hi, self.sr);
            self.xover = (lo, hi);
        }
    }

    /// The bands' tops (Hz): the crossovers and the containment's top.
    fn tops(&self) -> [f32; 3] {
        [self.xover.0, self.xover.1, CONTAIN_TOP_HZ]
    }

    /// Distort `buf` in place by `cfg` at `amount` 0..1 (scales every band's
    /// drive and wet; 0 leaves it alone), each band's drive offset by
    /// `tilt_db` (the Tone knob's focus).
    pub fn process(&mut self, buf: &mut [StereoSample], cfg: &MbConfig, amount: f32, tilt_db: [f32; 3]) {
        let a = amount.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        self.set_xover(cfg.xover);
        let tops = self.tops();
        for (k, b) in self.bands.iter_mut().enumerate() {
            let hz = if cfg.bands[k].contain > 0.0 { (tops[k] * cfg.bands[k].contain).min(0.45 * self.sr) } else { 0.0 };
            if b.contain_hz != hz && hz > 0.0 {
                b.contain.lowpass(hz, self.sr);
                b.contain_hz = hz;
            }
        }
        let total: f32 = self.bands.iter().map(|b| b.ms).sum::<f32>().max(1e-12);
        // Per band: the normalisation (in and out), and per pass the gain, the
        // makeup and the wet.
        let mut setting = [(1.0f32, [(1.0f32, 1.0f32, 0.0f32); 2]); 3];
        for (k, (b, set)) in self.bands.iter_mut().zip(setting.iter_mut()).enumerate() {
            let rms = b.ms.sqrt();
            let norm = if rms > 1e-6 { (WORK_LEVEL / rms).clamp(NORMALISE_RANGE.0, NORMALISE_RANGE.1) } else { 1.0 };
            let presence = (b.ms / total / PRESENCE).clamp(0.0, 1.0).sqrt();
            set.0 = norm;
            for (j, p) in cfg.bands[k].passes.iter().enumerate() {
                let db = p.drive_db * a + tilt_db[k];
                let drive = 10f32.powf(db / 20.0);
                // A sine at the working level comes out at it.
                let (key, made) = &mut b.makeup[j];
                if !matches!(*key, Some((c, d)) if c == p.curve && (d - db).abs() < MAKEUP_STEP_DB) {
                    *made = WORK_LEVEL / p.curve.sine_rms(WORK_LEVEL * std::f32::consts::SQRT_2 * drive).max(1e-4);
                    *key = Some((p.curve, db));
                }
                set.1[j] = (drive, *made, (p.wet.min(MAX_WET) * a * presence).clamp(0.0, MAX_WET));
            }
        }
        let sr = self.sr;
        for s in buf.iter_mut() {
            let x = [s.left, s.right];
            let (low, rest) = self.lo_split.split(x);
            let (mid, high) = self.hi_split.split(rest);
            let low = self.comp.split(low);
            let low = [low.0[0] + low.1[0], low.0[1] + low.1[1]];
            let parts = [low, mid, high];
            let mut out = [0.0f32; 2];
            for (k, b) in self.bands.iter_mut().enumerate() {
                let p = parts[k];
                b.ms = flush(b.ms + self.level_a * (0.5 * (p[0] * p[0] + p[1] * p[1]) - b.ms));
                let (norm, passes) = setting[k];
                let c = &cfg.bands[k];
                let y = if passes.iter().any(|p| p.2 > 0.0) {
                    let curves = c.passes.map(|p| p.curve);
                    let d = b.os.run(c.oversample, sr, p, |u| {
                        let mut v = [u[0] * norm, u[1] * norm];
                        for (curve, (drive, makeup, wet)) in curves.iter().zip(passes) {
                            if wet > 0.0 {
                                v = mix(v, [curve.shape(v[0] * drive) * makeup, curve.shape(v[1] * drive) * makeup], wet);
                            }
                        }
                        [v[0] / norm, v[1] / norm]
                    });
                    // An asymmetric curve leaves an offset: block it.
                    let d = if c.passes.iter().any(|p| p.wet > 0.0 && p.curve.asymmetric()) {
                        let mut d = d;
                        for ch in 0..2 {
                            b.dc[ch] = flush(b.dc[ch] + self.dc_a * (d[ch] - b.dc[ch]));
                            d[ch] -= b.dc[ch];
                        }
                        d
                    } else {
                        d
                    };
                    // The containment on what the curves added, not on the band.
                    if b.contain_hz > 0.0 && c.contain > 0.0 {
                        let added = b.contain.run([d[0] - p[0], d[1] - p[1]]);
                        [p[0] + added[0], p[1] + added[1]]
                    } else {
                        d
                    }
                } else {
                    p
                };
                out[0] += y[0];
                out[1] += y[1];
            }
            (s.left, s.right) = (out[0], out[1]);
        }
    }

    pub fn reset(&mut self) {
        for b in self.bands.iter_mut() {
            b.os.reset();
            b.contain.reset();
            b.dc = [0.0; 2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(hz: &[f32], secs: f32, sr: f32) -> Vec<StereoSample> {
        (0..(secs * sr) as usize)
            .map(|i| {
                let v = hz.iter().map(|f| 0.25 * (TAU * f * i as f32 / sr).sin()).sum::<f32>();
                StereoSample { left: v, right: v }
            })
            .collect()
    }

    fn rms(b: &[StereoSample]) -> f32 {
        (b.iter().map(|s| 0.5 * (s.left * s.left + s.right * s.right)).sum::<f32>() / b.len() as f32).sqrt()
    }

    fn cfg(curves: [Curve; 3], wet: f32) -> MbConfig {
        let band = |curve| BandConfig { passes: [Pass { curve, drive_db: 24.0, wet }, Pass::OFF], contain: 4.0, oversample: 4 };
        MbConfig { xover: (300.0, 2500.0), bands: curves.map(band) }
    }

    #[test]
    fn nothing_driven_sums_back_to_the_input_level() {
        let sr = 48_000.0;
        let x = tone(&[110.0, 900.0, 4000.0], 1.0, sr);
        let mut y = x.clone();
        let mut m = MbDist::new(sr);
        m.process(&mut y, &cfg([Curve::Tanh; 3], 0.0), 1.0, [0.0; 3]);
        let db = 20.0 * (rms(&y[24_000..]) / rms(&x[24_000..])).log10();
        assert!(db.abs() < 0.3, "{db:.2} dB");
    }

    #[test]
    fn it_never_goes_all_wet_and_stays_near_its_level() {
        let sr = 48_000.0;
        let x = tone(&[110.0, 900.0, 4000.0], 1.0, sr);
        for curve in [Curve::Tanh, Curve::Tube, Curve::Fold, Curve::Hard, Curve::Tape, Curve::Drive] {
            let mut y = x.clone();
            let mut m = MbDist::new(sr);
            for block in y.chunks_mut(256) {
                m.process(block, &cfg([curve; 3], 1.0), 1.0, [0.0; 3]);
            }
            assert!(y.iter().all(|s| s.left.is_finite()));
            let db = 20.0 * (rms(&y[24_000..]) / rms(&x[24_000..])).log10();
            assert!(db.abs() < 4.0, "{curve:?}: {db:.1} dB");
        }
    }

    #[test]
    fn a_quiet_band_distorts_like_a_loud_one() {
        // The normalisation: the same drive on a band 20 dB down still bites.
        let sr = 48_000.0;
        let quiet: Vec<StereoSample> = tone(&[900.0], 1.0, sr).into_iter().map(|s| StereoSample { left: 0.1 * s.left, right: 0.1 * s.right }).collect();
        let mut m = MbDist::new(sr);
        let mut y = quiet.clone();
        for block in y.chunks_mut(256) {
            m.process(block, &cfg([Curve::Hard; 3], 0.8), 1.0, [0.0; 3]);
        }
        let diff: f32 = y[24_000..].iter().zip(&quiet[24_000..]).map(|(a, b)| (a.left - b.left).powi(2)).sum::<f32>() / 24_000.0;
        assert!(diff.sqrt() > 0.1 * rms(&quiet[24_000..]), "the quiet band is driven");
    }

    #[test]
    fn processing_does_not_allocate() {
        let sr = 48_000.0;
        let mut m = MbDist::new(sr);
        let mut buf = tone(&[110.0, 900.0], 0.05, sr);
        let c = cfg([Curve::Fold, Curve::Hard, Curve::Tanh], 0.7);
        let (_, allocs) = crate::test_support::count_allocs(|| m.process(&mut buf, &c, 1.0, [0.0; 3]));
        assert_eq!(allocs, 0);
    }
}
