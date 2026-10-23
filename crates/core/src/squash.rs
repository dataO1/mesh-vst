//! Squash: the lane's last stage, a six-band compressor that pushes each
//! band down above its threshold and lifts it below another (OTT style).
//!
//! One amount runs from mild (gentle ratios, high thresholds, fast times)
//! to strong (low thresholds, hard up and down compression, a smiley
//! balance). The drums reach a much lighter strong end than the mudpie,
//! which can be squashed hard. LR4 crossovers with allpass compensation, so
//! the bands sum flat when nothing compresses. The output keeps the input's
//! loudness ([`crate::heat`]'s level match). Times are note values,
//! stretched with the tempo ([`crate::tempo`]).

use std::f32::consts::{PI, SQRT_2};

use crate::heat::LevelMatch;
use crate::tempo::note;
use crate::types::StereoSample;

const BANDS: usize = 6;
const SPLITS: usize = BANDS - 1;

/// One band's dynamics: down threshold (dB) and ratio, up threshold and
/// ratio, attack and release (seconds at the reference tempo), gain (dB).
#[derive(Debug, Clone, Copy)]
struct Band {
    down: (f32, f32),
    up: (f32, f32),
    attack: f32,
    release: f32,
    gain: f32,
}

const fn band(down: (f32, f32), up: (f32, f32), attack: f32, release: f32, gain: f32) -> Band {
    Band { down, up, attack, release, gain }
}

/// A setting: the bands, the most upward gain (dB).
#[derive(Debug, Clone, Copy)]
struct Setting {
    bands: [Band; BANDS],
    up_max: f32,
}

/// The ends of one role's range, and its crossovers (Hz).
struct Voicing {
    splits: [f32; SPLITS],
    mild: Setting,
    strong: Setting,
}

// MUSICAL: first draft, tune by ear (user: mild = gentle ratios, easy
// thresholds, fast times; strong = OTT-like on the mudpie with a smiley
// balance, as Mesh live's bass multiband; the drums much less aggressive;
// user, 2026-10-20: the mids cut too much; 2026-10-23: more aggressive,
// mostly by lifting more, the down thresholds only a little lower; a
// balance kept around each band's own level muffled it, reverted).
// Bands: sub | low | mids | high mids | highs | air.
const MUDPIE: Voicing = Voicing {
    splits: [90.0, 250.0, 2000.0, 5000.0, 10_000.0],
    mild: Setting {
        bands: [
            band((-14.0, 1.6), (-80.0, 1.0), note(1.0 / 64.0), note(1.0 / 8.0), 0.0),
            band((-14.0, 1.6), (-48.0, 1.3), note(1.0 / 256.0), note(1.0 / 16.0), 0.0),
            band((-14.0, 1.6), (-48.0, 1.3), note(1.0 / 256.0), note(1.0 / 16.0), 0.0),
            band((-14.0, 1.6), (-48.0, 1.3), note(1.0 / 256.0), note(1.0 / 16.0), 0.0),
            band((-14.0, 1.6), (-48.0, 1.3), note(1.0 / 256.0), note(1.0 / 16.0), 0.0),
            band((-14.0, 1.6), (-48.0, 1.3), note(1.0 / 256.0), note(1.0 / 16.0), 0.0),
        ],
        up_max: 4.0,
    },
    strong: Setting {
        bands: [
            band((-23.0, 5.0), (-80.0, 1.0), note(1.0 / 64.0), note(1.0 / 8.0), -1.5),
            band((-28.0, 8.0), (-40.0, 3.5), note(1.0 / 128.0), note(1.0 / 16.0), -1.0),
            band((-31.0, 5.0), (-40.0, 4.0), note(1.0 / 256.0), note(1.0 / 16.0), 2.5),
            band((-34.0, 8.0), (-38.0, 8.0), note(1.0 / 512.0), note(3.0 / 64.0), 3.5),
            band((-34.0, 8.0), (-40.0, 8.0), note(1.0 / 1024.0), note(1.0 / 32.0), 3.5),
            band((-34.0, 8.0), (-40.0, 8.0), note(1.0 / 1024.0), note(1.0 / 32.0), 3.0),
        ],
        up_max: 20.0,
    },
};

const DRUMS: Voicing = Voicing {
    splits: [120.0, 400.0, 2500.0, 6000.0, 12_000.0],
    mild: Setting {
        bands: [
            band((-12.0, 1.4), (-80.0, 1.0), note(1.0 / 128.0), note(1.0 / 16.0), 0.0),
            band((-12.0, 1.4), (-50.0, 1.15), note(1.0 / 128.0), note(1.0 / 16.0), 0.0),
            band((-12.0, 1.4), (-50.0, 1.15), note(1.0 / 128.0), note(1.0 / 16.0), 0.0),
            band((-12.0, 1.4), (-50.0, 1.15), note(1.0 / 128.0), note(1.0 / 16.0), 0.0),
            band((-12.0, 1.4), (-50.0, 1.15), note(1.0 / 128.0), note(1.0 / 16.0), 0.0),
            band((-12.0, 1.4), (-50.0, 1.15), note(1.0 / 128.0), note(1.0 / 16.0), 0.0),
        ],
        up_max: 2.0,
    },
    // The attacks stay at a 128th so the hits keep their punch.
    strong: Setting {
        bands: [
            band((-23.0, 3.0), (-80.0, 1.0), note(1.0 / 128.0), note(1.0 / 16.0), 1.0),
            band((-25.0, 3.5), (-39.0, 2.2), note(1.0 / 128.0), note(1.0 / 16.0), 0.0),
            band((-23.0, 2.6), (-39.0, 2.2), note(1.0 / 128.0), note(1.0 / 16.0), 0.5),
            band((-25.0, 3.5), (-39.0, 2.2), note(1.0 / 128.0), note(1.0 / 16.0), 1.0),
            band((-25.0, 3.5), (-39.0, 2.2), note(1.0 / 128.0), note(1.0 / 32.0), 1.5),
            band((-25.0, 3.5), (-39.0, 2.2), note(1.0 / 128.0), note(1.0 / 32.0), 1.0),
        ],
        up_max: 11.0,
    },
};

/// Level below which a band is not lifted (keeps noise and tails down).
const UP_GATE_DB: f32 = -65.0;
/// Gain smoothing after the envelope (against zipper noise).
const GAIN_SMOOTH_SECS: f32 = 0.001;
/// The level match's make-up range (a hard squash takes more than 6 dB).
const LEVEL_RANGE: (f32, f32) = (0.25, 4.0);
// MUSICAL: tune by ear (user, 2026-10-20: matched with K-weighting's highs
// the lifted top pulled the body down, much quieter than before).
/// The highs' extra weight in the level match: none, so the top the
/// squash lifts does not turn the lows and mids down.
const LEVEL_HIGH: f32 = 0.0;
const FLOOR: f32 = 1e-6;

#[inline]
fn flush(v: f32) -> f32 {
    if v.abs() < 1e-20 { 0.0 } else { v }
}

/// Coefficient of a one-pole follower with time constant `secs`.
fn coeff(secs: f32, sr: f32) -> f32 {
    (-1.0 / (secs * sr).max(1.0)).exp()
}

/// Butterworth TPT state-variable filter, stereo, low and high outputs.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    a1: f32,
    a2: f32,
    a3: f32,
    /// [ic1, ic2] per channel.
    ic: [[f32; 2]; 2],
}

impl Svf {
    fn set(&mut self, hz: f32, sr: f32) {
        let g = (PI * hz.min(0.45 * sr) / sr).tan();
        self.a1 = 1.0 / (1.0 + g * (g + SQRT_2));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    fn run(&mut self, x: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let (mut lo, mut hi) = ([0.0; 2], [0.0; 2]);
        for c in 0..2 {
            let [ic1, ic2] = &mut self.ic[c];
            let v3 = x[c] - *ic2;
            let v1 = self.a1 * *ic1 + self.a2 * v3;
            let v2 = *ic2 + self.a2 * *ic1 + self.a3 * v3;
            *ic1 = flush(2.0 * v1 - *ic1);
            *ic2 = flush(2.0 * v2 - *ic2);
            lo[c] = v2;
            hi[c] = x[c] - SQRT_2 * v1 - v2;
        }
        (lo, hi)
    }
}

/// Linkwitz-Riley 4th-order split: its low and high sum to an allpass.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Lr4 {
    first: Svf,
    low: Svf,
    high: Svf,
}

impl Lr4 {
    pub(crate) fn set(&mut self, hz: f32, sr: f32) {
        for f in [&mut self.first, &mut self.low, &mut self.high] {
            f.set(hz, sr);
        }
    }

    #[inline]
    pub(crate) fn split(&mut self, x: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let (lo, hi) = self.first.run(x);
        (self.low.run(lo).0, self.high.run(hi).1)
    }
}

/// One band's running state and its settings for the block.
#[derive(Debug, Clone, Copy, Default)]
struct BandState {
    env: f32,
    gain_db: f32,
    down_t: f32,
    down_slope: f32,
    up_t: f32,
    up_slope: f32,
    attack: f32,
    release: f32,
    makeup_db: f32,
}

/// The Squash stage of one lane.
pub struct Squash {
    sr: f32,
    drums: Option<bool>,
    splits: [Lr4; SPLITS],
    /// Allpass compensation: band `b` passes the splits after its own.
    comp: Vec<Vec<Lr4>>,
    bands: [BandState; BANDS],
    up_max: f32,
    smooth: f32,
    level: LevelMatch,
    /// The amount, stretch and voicing the bands' settings were made for
    /// (made again only when these change).
    set_for: Option<(f32, f32, bool)>,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Times blend on a log scale.
fn lerp_log(a: f32, b: f32, t: f32) -> f32 {
    a * (b / a).powf(t)
}

impl Squash {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        Self {
            sr,
            drums: None,
            splits: [Lr4::default(); SPLITS],
            comp: (0..BANDS).map(|b| vec![Lr4::default(); SPLITS.saturating_sub(b + 1)]).collect(),
            bands: [BandState::default(); BANDS],
            up_max: 0.0,
            smooth: coeff(GAIN_SMOOTH_SECS, sr),
            level: LevelMatch::new(sr, LEVEL_RANGE, LEVEL_HIGH),
            set_for: None,
        }
    }

    /// The bands' settings at `t` 0 (mild) .. 1 (strong) of voicing `v`.
    fn set(&mut self, v: &Voicing, t: f32, stretch: f32) {
        for (st, (m, s)) in self.bands.iter_mut().zip(v.mild.bands.iter().zip(v.strong.bands.iter())) {
            st.down_t = lerp(m.down.0, s.down.0, t);
            st.down_slope = 1.0 - 1.0 / lerp(m.down.1, s.down.1, t).max(1.0);
            st.up_t = lerp(m.up.0, s.up.0, t);
            st.up_slope = 1.0 - 1.0 / lerp(m.up.1, s.up.1, t).max(1.0);
            st.attack = coeff(lerp_log(m.attack, s.attack, t) * stretch, self.sr);
            st.release = coeff(lerp_log(m.release, s.release, t) * stretch, self.sr);
            st.makeup_db = lerp(m.gain, s.gain, t);
        }
        self.up_max = lerp(v.mild.up_max, v.strong.up_max, t);
    }

    /// Squash `buf` in place at `amount` 0 (mild) .. 1 (strong), voiced for
    /// the drums or the mudpie; `stretch` is the tempo's
    /// ([`crate::tempo::stretch`]).
    pub fn process(&mut self, buf: &mut [StereoSample], amount: f32, drums: bool, stretch: f32) {
        let v = if drums { &DRUMS } else { &MUDPIE };
        if self.drums != Some(drums) {
            self.drums = Some(drums);
            for (i, s) in self.splits.iter_mut().enumerate() {
                s.set(v.splits[i], self.sr);
            }
            for (b, chain) in self.comp.iter_mut().enumerate() {
                for (k, s) in chain.iter_mut().enumerate() {
                    s.set(v.splits[b + 1 + k], self.sr);
                }
            }
        }
        let t = amount.clamp(0.0, 1.0);
        if self.set_for != Some((t, stretch, drums)) {
            self.set_for = Some((t, stretch, drums));
            self.set(v, t, stretch);
        }
        for s in buf.iter_mut() {
            let x = [s.left, s.right];
            // Band b is the low side of split b, the last band the rest.
            let mut parts = [[0.0f32; 2]; BANDS];
            let mut rest = x;
            for (i, split) in self.splits.iter_mut().enumerate() {
                let (lo, hi) = split.split(rest);
                parts[i] = lo;
                rest = hi;
            }
            parts[SPLITS] = rest;
            // Phase-align the early bands with the later splits.
            for (part, chain) in parts.iter_mut().zip(self.comp.iter_mut()) {
                for split in chain.iter_mut() {
                    let (lo, hi) = split.split(*part);
                    *part = [lo[0] + hi[0], lo[1] + hi[1]];
                }
            }
            let mut y = [0.0f32; 2];
            for (p, b) in parts.iter().zip(self.bands.iter_mut()) {
                let peak = p[0].abs().max(p[1].abs());
                let k = if peak > b.env { b.attack } else { b.release };
                b.env = flush(k * b.env + (1.0 - k) * peak);
                let level = 20.0 * b.env.max(FLOOR).log10();
                let down = (level - b.down_t).max(0.0) * b.down_slope;
                let up = if level > UP_GATE_DB { ((b.up_t - level).max(0.0) * b.up_slope).min(self.up_max) } else { 0.0 };
                b.gain_db = flush(self.smooth * b.gain_db + (1.0 - self.smooth) * (up - down));
                let g = 10f32.powf((b.gain_db + b.makeup_db) / 20.0);
                y[0] += p[0] * g;
                y[1] += p[1] * g;
            }
            let g = self.level.gain(x, y);
            *s = StereoSample { left: flush(y[0] * g), right: flush(y[1] * g) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A tone burst pattern: loud 50 ms, quiet 200 ms, at `hz`.
    fn bursts(hz: f32, secs: f32) -> Vec<StereoSample> {
        (0..(secs * SR) as usize)
            .map(|i| {
                let t = i as f32 / SR;
                let amp = if (t % 0.25) < 0.05 { 0.5 } else { 0.02 };
                let v = amp * (std::f32::consts::TAU * hz * t).sin();
                StereoSample { left: v, right: v }
            })
            .collect()
    }

    /// Peak-to-quiet ratio in dB over the last second.
    fn contrast(b: &[StereoSample]) -> f32 {
        let tail = &b[b.len() - SR as usize..];
        let (mut loud, mut quiet) = (0.0f32, 0.0f32);
        for (i, s) in tail.iter().enumerate() {
            let t = i as f32 / SR % 0.25;
            if (0.01..0.05).contains(&t) {
                loud = loud.max(s.left.abs());
            } else if (0.1..0.24).contains(&t) {
                quiet = quiet.max(s.left.abs());
            }
        }
        20.0 * (loud / quiet.max(1e-9)).log10()
    }

    fn run(amount: f32, drums: bool, hz: f32) -> Vec<StereoSample> {
        let mut b = bursts(hz, 3.0);
        let mut s = Squash::new(SR);
        for chunk in b.chunks_mut(256) {
            s.process(chunk, amount, drums, 1.0);
        }
        b
    }

    #[test]
    fn strong_squashes_more_than_mild_and_the_mudpie_most() {
        let dry = contrast(&bursts(1000.0, 3.0));
        let (mild, strong) = (contrast(&run(0.0, false, 1000.0)), contrast(&run(1.0, false, 1000.0)));
        let drums = contrast(&run(1.0, true, 1000.0));
        assert!(dry > mild && mild > drums && drums > strong, "dry {dry}, mild {mild}, drums strong {drums}, mudpie strong {strong}");
        assert!(dry - strong > 8.0, "strong squashes hard: {dry} -> {strong} dB");
    }

    #[test]
    fn keeps_the_loudness_and_stays_finite() {
        let rms = |b: &[StereoSample]| (b[b.len() / 2..].iter().map(|s| s.left * s.left).sum::<f32>() / (b.len() / 2) as f32).sqrt();
        let dry = rms(&bursts(300.0, 3.0));
        for drums in [false, true] {
            for amount in [0.0, 0.5, 1.0] {
                let wet = run(amount, drums, 300.0);
                assert!(wet.iter().all(|s| s.left.is_finite()));
                let db = 20.0 * (rms(&wet) / dry).log10();
                assert!(db.abs() < 2.5, "drums {drums} at {amount}: {db} dB");
            }
        }
    }

    #[test]
    fn the_bands_sum_flat_when_nothing_compresses() {
        // A quiet steady sine sits between the thresholds: unchanged level.
        let sine: Vec<StereoSample> = (0..(2.0 * SR) as usize)
            .map(|i| {
                let v = 0.05 * (std::f32::consts::TAU * 2000.0 * i as f32 / SR).sin();
                StereoSample { left: v, right: v }
            })
            .collect();
        let mut b = sine.clone();
        Squash::new(SR).process(&mut b, 0.0, true, 1.0);
        let peak = |v: &[StereoSample]| v[v.len() / 2..].iter().map(|s| s.left.abs()).fold(0.0f32, f32::max);
        assert!((peak(&b) / peak(&sine) - 1.0).abs() < 0.1, "{} vs {}", peak(&b), peak(&sine));
    }
}
