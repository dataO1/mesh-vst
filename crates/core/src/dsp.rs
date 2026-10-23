//! Small DSP building blocks shared by the processing stages (the neuro
//! chain, the multiband distortion, Heat): an RBJ biquad, a fractional delay
//! line and 2× / 4× oversampling. RT-safe once built.

use std::f32::consts::{SQRT_2, TAU};

/// Zero for values too small to hear, so filter tails never go subnormal.
#[inline]
pub(crate) fn flush(v: f32) -> f32 {
    if v.abs() < 1e-20 { 0.0 } else { v }
}

/// `dry` moved towards `wet` by `m` (0 dry .. 1 wet).
#[inline]
pub(crate) fn mix(dry: [f32; 2], wet: [f32; 2], m: f32) -> [f32; 2] {
    [dry[0] + (wet[0] - dry[0]) * m, dry[1] + (wet[1] - dry[1]) * m]
}

/// A safety soft limit inside a chain (far above the working level).
#[inline]
pub(crate) fn safe(x: f32) -> f32 {
    4.0 * (x / 4.0).tanh()
}

/// RBJ biquad, stereo, transposed direct form II.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [[f32; 2]; 2],
}

fn trig(hz: f32, q: f32, sr: f32) -> (f32, f32, f32) {
    let w = TAU * hz.clamp(20.0, 0.45 * sr) / sr;
    let (s, c) = w.sin_cos();
    (s, c, s / (2.0 * q.max(0.05)))
}

impl Biquad {
    fn set(&mut self, b: [f32; 3], a: [f32; 3]) {
        let n = 1.0 / a[0];
        self.b = [b[0] * n, b[1] * n, b[2] * n];
        self.a = [a[1] * n, a[2] * n];
    }

    pub(crate) fn peaking(&mut self, hz: f32, q: f32, db: f32, sr: f32) {
        let a = 10f32.powf(db / 40.0);
        let (_, c, alpha) = trig(hz, q, sr);
        self.set([1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a]);
    }

    pub(crate) fn highpass(&mut self, hz: f32, q: f32, sr: f32) {
        let (_, c, alpha) = trig(hz, q, sr);
        let b1 = 1.0 + c;
        self.set([b1 / 2.0, -b1, b1 / 2.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha]);
    }

    pub(crate) fn lowpass(&mut self, hz: f32, q: f32, sr: f32) {
        let (_, c, alpha) = trig(hz, q, sr);
        let b1 = 1.0 - c;
        self.set([b1 / 2.0, b1, b1 / 2.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha]);
    }

    /// RBJ high shelf, slope 1.
    pub(crate) fn high_shelf(&mut self, hz: f32, db: f32, sr: f32) {
        let a = 10f32.powf(db / 40.0);
        let w = TAU * hz.clamp(20.0, 0.45 * sr) / sr;
        let (sn, cs) = w.sin_cos();
        let alpha = sn / 2.0 * SQRT_2;
        let k = 2.0 * a.sqrt() * alpha;
        self.set(
            [a * ((a + 1.0) + (a - 1.0) * cs + k), -2.0 * a * ((a - 1.0) + (a + 1.0) * cs), a * ((a + 1.0) + (a - 1.0) * cs - k)],
            [(a + 1.0) - (a - 1.0) * cs + k, 2.0 * ((a - 1.0) - (a + 1.0) * cs), (a + 1.0) - (a - 1.0) * cs - k],
        );
    }

    #[inline]
    pub(crate) fn run(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mut y = [0.0; 2];
        for ch in 0..2 {
            let z = &mut self.z[ch];
            y[ch] = self.b[0] * x[ch] + z[0];
            z[0] = flush(self.b[1] * x[ch] - self.a[0] * y[ch] + z[1]);
            z[1] = flush(self.b[2] * x[ch] - self.a[1] * y[ch]);
        }
        y
    }

    pub(crate) fn reset(&mut self) {
        self.z = [[0.0; 2]; 2];
    }
}

/// Q of the two sections of a fourth-order Butterworth.
pub(crate) const BUTTERWORTH_Q: [f32; 2] = [0.541_196, 1.306_563];

/// A fourth-order Butterworth lowpass or highpass (two biquads).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Butter4 {
    s: [Biquad; 2],
}

impl Butter4 {
    pub(crate) fn lowpass(&mut self, hz: f32, sr: f32) {
        for (b, q) in self.s.iter_mut().zip(BUTTERWORTH_Q) {
            b.lowpass(hz, q, sr);
        }
    }

    pub(crate) fn highpass(&mut self, hz: f32, sr: f32) {
        for (b, q) in self.s.iter_mut().zip(BUTTERWORTH_Q) {
            b.highpass(hz, q, sr);
        }
    }

    #[inline]
    pub(crate) fn run(&mut self, x: [f32; 2]) -> [f32; 2] {
        let y = self.s[0].run(x);
        self.s[1].run(y)
    }

    pub(crate) fn reset(&mut self) {
        self.s.iter_mut().for_each(Biquad::reset);
    }
}

/// A stereo delay line with fractional reads.
#[derive(Debug, Clone)]
pub(crate) struct Delay {
    buf: Vec<[f32; 2]>,
    at: usize,
}

impl Delay {
    pub(crate) fn new(len: usize) -> Self {
        Self { buf: vec![[0.0; 2]; len.max(4)], at: 0 }
    }

    #[inline]
    pub(crate) fn push(&mut self, x: [f32; 2]) {
        self.at = (self.at + 1) % self.buf.len();
        self.buf[self.at] = x;
    }

    /// The frame `d` samples back (fractional, linear), per channel.
    #[inline]
    pub(crate) fn read(&self, d: f32, ch: usize) -> f32 {
        let n = self.buf.len();
        let d = d.clamp(1.0, (n - 2) as f32);
        let i = d.floor();
        let t = d - i;
        let a = self.buf[(self.at + n - i as usize) % n][ch];
        let b = self.buf[(self.at + n - i as usize - 1) % n][ch];
        a + (b - a) * t
    }

    pub(crate) fn reset(&mut self) {
        self.buf.fill([0.0; 2]);
    }
}

// MUSICAL: the oversampler's cutoff, a share of the base rate (its Nyquist
// is half of it).
const OVERSAMPLE_CUTOFF: f32 = 0.46;

/// 2× oversampling for a distortion (its harmonics would fold back as
/// inharmonic fizz, worse the higher the note): zero-stuffed, a
/// fourth-order Butterworth lowpass on the way up and on the way down.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Oversampler {
    up: Butter4,
    down: Butter4,
    sr: f32,
}

impl Oversampler {
    /// Set up for base rate `sr` (cheap when it did not change).
    pub(crate) fn ready(&mut self, sr: f32) {
        if self.sr != sr {
            self.up.lowpass(OVERSAMPLE_CUTOFF * sr, 2.0 * sr);
            self.down.lowpass(OVERSAMPLE_CUTOFF * sr, 2.0 * sr);
            self.sr = sr;
        }
    }

    /// `f` on frame `x` at twice the rate.
    #[inline]
    pub(crate) fn run(&mut self, x: [f32; 2], mut f: impl FnMut([f32; 2]) -> [f32; 2]) -> [f32; 2] {
        let mut out = [0.0; 2];
        for k in 0..2 {
            let x = if k == 0 { [2.0 * x[0], 2.0 * x[1]] } else { [0.0; 2] };
            let u = self.up.run(x);
            let d = self.down.run(f(u));
            if k == 1 {
                out = d;
            }
        }
        out
    }

    pub(crate) fn reset(&mut self) {
        self.up.reset();
        self.down.reset();
    }
}

/// 1×, 2× or 4× oversampling (two 2× stages in a row for 4×).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Oversample {
    outer: Oversampler,
    inner: Oversampler,
}

impl Oversample {
    /// `f` on frame `x` at `factor` (1, 2 or 4) times base rate `sr`.
    #[inline]
    pub(crate) fn run(&mut self, factor: u8, sr: f32, x: [f32; 2], mut f: impl FnMut([f32; 2]) -> [f32; 2]) -> [f32; 2] {
        match factor {
            0 | 1 => f(x),
            2 => {
                self.outer.ready(sr);
                self.outer.run(x, f)
            }
            _ => {
                self.outer.ready(sr);
                self.inner.ready(2.0 * sr);
                let inner = &mut self.inner;
                self.outer.run(x, |u| inner.run(u, &mut f))
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.outer.reset();
        self.inner.reset();
    }
}

/// A topology-preserving state-variable filter (modulation-safe): its
/// lowpass, bandpass and highpass at once; `k` is 1/Q.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Svf {
    a: [f32; 3],
    k: f32,
    ic: [[f32; 2]; 2],
}

impl Svf {
    pub(crate) fn set(&mut self, hz: f32, k: f32, sr: f32) {
        let g = (std::f32::consts::PI * hz.clamp(10.0, 0.45 * sr) / sr).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        self.a = [a1, g * a1, g * g * a1];
        self.k = k;
    }

    /// (lowpass, bandpass, highpass) of `x`.
    #[inline]
    pub(crate) fn run(&mut self, x: [f32; 2]) -> [[f32; 2]; 3] {
        let mut out = [[0.0; 2]; 3];
        for c in 0..2 {
            let [ic1, ic2] = &mut self.ic[c];
            let v3 = x[c] - *ic2;
            let v1 = self.a[0] * *ic1 + self.a[1] * v3;
            let v2 = *ic2 + self.a[1] * *ic1 + self.a[2] * v3;
            *ic1 = flush(2.0 * v1 - *ic1);
            *ic2 = flush(2.0 * v2 - *ic2);
            out[0][c] = v2;
            out[1][c] = v1;
            out[2][c] = x[c] - self.k * v1 - v2;
        }
        out
    }

    /// The allpass of `x` (lowpass − k·bandpass + highpass).
    #[inline]
    pub(crate) fn allpass(&mut self, x: [f32; 2]) -> [f32; 2] {
        let [_, b, _] = self.run(x);
        [x[0] - 2.0 * self.k * b[0], x[1] - 2.0 * self.k * b[1]]
    }

    pub(crate) fn reset(&mut self) {
        self.ic = [[0.0; 2]; 2];
    }
}

/// A topology-preserving one-pole: its lowpass and highpass.
#[derive(Debug, Clone, Copy, Default)]
struct OnePole {
    g: f32,
    s: [f32; 2],
}

impl OnePole {
    fn set(&mut self, hz: f32, sr: f32) {
        let g = (std::f32::consts::PI * hz.clamp(10.0, 0.45 * sr) / sr).tan();
        self.g = g / (1.0 + g);
    }

    #[inline]
    fn run(&mut self, x: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let (mut lo, mut hi) = ([0.0; 2], [0.0; 2]);
        for c in 0..2 {
            let v = (x[c] - self.s[c]) * self.g;
            lo[c] = v + self.s[c];
            self.s[c] = flush(lo[c] + v);
            hi[c] = x[c] - lo[c];
        }
        (lo, hi)
    }
}

/// A crossover split whose two sides sum to an allpass: Linkwitz-Riley
/// fourth order (24 dB/octave) or Butterworth third order (18 dB/octave).
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    steep: bool,
    first2: Svf,
    first1: OnePole,
    low: Svf,
    high: Svf,
}

impl Split {
    fn set(&mut self, hz: f32, steep: bool, sr: f32) {
        self.steep = steep;
        let k = if steep { SQRT_2 } else { 1.0 };
        if steep {
            self.first2.set(hz, k, sr);
        } else {
            self.first1.set(hz, sr);
        }
        self.low.set(hz, k, sr);
        self.high.set(hz, k, sr);
    }

    #[inline]
    fn run(&mut self, x: [f32; 2]) -> ([f32; 2], [f32; 2]) {
        let (lo, hi) = if self.steep {
            let [l, _, h] = self.first2.run(x);
            (l, h)
        } else {
            self.first1.run(x)
        };
        (self.low.run(lo)[0], self.high.run(hi)[2])
    }
}

/// A band-reject made of two crossovers: the band between them is taken
/// out (a true gap with the crossovers' slopes, not a bell), the sides
/// pass with an allpass's phase. Its edges may move every few frames.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Gap {
    lo: Split,
    hi: Split,
    /// The low side's allpass of the upper crossover.
    comp: Svf,
}

impl Gap {
    /// Edges at `lo_hz` and `hi_hz`; `steep`: 24 dB/octave, else 18.
    pub(crate) fn set(&mut self, lo_hz: f32, hi_hz: f32, steep: bool, sr: f32) {
        let hi_hz = hi_hz.max(lo_hz * 1.05);
        self.lo.set(lo_hz, steep, sr);
        self.hi.set(hi_hz, steep, sr);
        self.comp.set(hi_hz, if steep { SQRT_2 } else { 1.0 }, sr);
    }

    /// `x` with the band between the edges at `keep` (0 gone .. 1 all).
    #[inline]
    pub(crate) fn run(&mut self, x: [f32; 2], keep: f32) -> [f32; 2] {
        let (low, rest) = self.lo.run(x);
        let (mid, high) = self.hi.run(rest);
        let low = self.comp.allpass(low);
        [low[0] + keep * mid[0] + high[0], low[1] + keep * mid[1] + high[1]]
    }

    pub(crate) fn reset(&mut self) {
        for s in [&mut self.lo, &mut self.hi] {
            s.first1.s = [0.0; 2];
            s.first2.reset();
            s.low.reset();
            s.high.reset();
        }
        self.comp.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// The level of a sine at `hz` through the gap (dB).
    fn through(hz: f32, (lo, hi): (f32, f32), steep: bool, keep: f32) -> f32 {
        let sr = 48_000.0;
        let mut g = Gap::default();
        g.set(lo, hi, steep, sr);
        let n = 48_000;
        let mut e = (0.0f32, 0.0f32);
        for i in 0..n {
            let x = (TAU * hz * i as f32 / sr).sin();
            let y = g.run([x, x], keep)[0];
            if i > n / 2 {
                e.0 += x * x;
                e.1 += y * y;
            }
        }
        10.0 * (e.1 / e.0).log10()
    }

    #[test]
    fn a_gap_passes_all_when_kept_and_takes_out_its_band() {
        for steep in [true, false] {
            for hz in [60.0, 300.0, 1000.0, 4000.0] {
                assert!(through(hz, (200.0, 800.0), steep, 1.0).abs() < 0.2, "{steep} {hz}");
            }
            // Outside the gap it passes, in its middle it is gone.
            assert!(through(60.0, (200.0, 800.0), steep, 0.0) > -1.5);
            assert!(through(4000.0, (200.0, 800.0), steep, 0.0) > -1.5);
        }
        assert!(through(400.0, (200.0, 800.0), true, 0.0) < -15.0, "{}", through(400.0, (200.0, 800.0), true, 0.0));
        assert!(through(400.0, (200.0, 800.0), false, 0.0) < -10.0, "{}", through(400.0, (200.0, 800.0), false, 0.0));
    }
}
