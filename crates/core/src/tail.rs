//! Drum release tails (Shape above its middle) and the transient shaper in
//! front of them.
//!
//! Three ways to grow a tail out of the hits, picked by the sound seed or
//! the switch next to Shape:
//!
//! - [`TailMode::Vocoder`]: the hits' spectrum, band by band, shapes white
//!   noise (the noise-carrier vocoder trick); each band's envelope releases
//!   over the tail time, so every hit leaves a noise tail of its own colour.
//! - [`TailMode::Spectral`]: a spectral sustain. Each FFT bin holds its
//!   peak and lets it fade over the tail time; the part above the dry
//!   spectrum is resynthesized with random phases.
//! - [`TailMode::Noise`]: three noise bands (low, mid, high) follow the
//!   hits' level in their band and fade over the tail time.
//!
//! Each drum voice group grows its own tail ([`VoiceTails`]): the snare the
//! longest and strongest, the hats and cymbals shorter and a little weaker,
//! the kick a very short, faint one, the loops none.
//!
//! Noise and phases come from the song position, never from free-running
//! state, so a render repeats exactly. RT-safe: buffers are allocated in
//! [`Tail::new`].

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::generator::layout::DrumVoice;
use crate::tempo::note;
use crate::types::StereoSample;

/// How the tail is grown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TailMode {
    #[default]
    Vocoder,
    Spectral,
    Noise,
}

impl TailMode {
    pub const ALL: [TailMode; 3] = [TailMode::Vocoder, TailMode::Spectral, TailMode::Noise];

    /// The sound seed's tail.
    pub fn seeded(sound_seed: u32) -> Self {
        Self::ALL[(crate::rng::hash(sound_seed, 0, 0, 0, S_TAIL) % 3) as usize]
    }
}

const S_TAIL: u32 = 0xC53;

// MUSICAL: first draft, tune by ear (user: tails from Shape 50 % up, at
// most an 8th on the snare, the hats shorter and a little weaker, the kick
// very little and short; the shaper brings the attack up a little at low
// Shape and the tail out above).
/// Tail time at the start of the range and at full Shape (the snare's).
const TAIL_SECS: (f32, f32) = (note(1.0 / 64.0), note(1.0 / 8.0));
/// Tail groups: the kick, the snare, the hats, cymbals and percussion.
pub const GROUPS: usize = 3;
/// Each group's tail level and time against the snare's.
const GROUP_GAIN: [f32; GROUPS] = [0.15, 1.0, 0.75];
const GROUP_LENGTH: [f32; GROUPS] = [0.25, 1.0, 0.5];

/// The tail group of a drum voice; the loops (break, top) grow none.
pub fn group(voice: DrumVoice) -> Option<usize> {
    match voice {
        DrumVoice::Kick => Some(0),
        DrumVoice::Snare => Some(1),
        DrumVoice::Hats | DrumVoice::Ride | DrumVoice::Shaker | DrumVoice::Perc => Some(2),
        DrumVoice::Break | DrumVoice::Top => None,
    }
}

/// The drum sampler's bus map: group `g` on bus `g + 1`, the rest on the
/// main output ([`crate::sampler::Sampler::set_bus_map`]).
pub fn bus_map() -> [u8; 256] {
    std::array::from_fn(|t| DrumVoice::of_target(t as u8).and_then(group).map_or(0, |g| g as u8 + 1))
}

/// The drum lane's tails, one per group.
pub struct VoiceTails {
    tails: Vec<Tail>,
}

impl VoiceTails {
    pub fn new(sample_rate: f32) -> Self {
        Self { tails: (0..GROUPS).map(|_| Tail::new(sample_rate)).collect() }
    }

    /// Grow each group's tail on its bus and mix it into `main`. `buses`
    /// holds [`GROUPS`] buses of `main.len()` frames back to back (the
    /// sampler's split output); `at` is the song position in samples,
    /// `stretch` the tempo's ([`crate::tempo::stretch`]).
    pub fn process(&mut self, main: &mut [StereoSample], buses: &mut [StereoSample], shape: f32, mode: TailMode, at: u64, stretch: f32) {
        let n = main.len();
        for (g, (tail, bus)) in self.tails.iter_mut().zip(buses.chunks_mut(n.max(1))).enumerate() {
            tail.process(bus, shape, mode, at, (GROUP_GAIN[g], GROUP_LENGTH[g] * stretch));
            for (m, b) in main.iter_mut().zip(bus.iter()) {
                *m += *b;
            }
        }
    }
}
/// Tail level at full Shape (linear, against the hits).
const TAIL_GAIN: f32 = 0.6;
/// Shaper: most attack boost (at Shape 0) and sustain boost (full Shape), dB.
const ATTACK_DB: f32 = 3.0;
const SUSTAIN_DB: f32 = 6.0;
/// Vocoder bands: count and range.
const BANDS: usize = 16;
const BAND_RANGE: (f32, f32) = (300.0, 12_000.0);
const BAND_Q: f32 = 4.0;
/// Level of the summed bands against one (uncorrelated noise bands add in power).
const BAND_SUM: f32 = 0.25;
/// Release of the followers that track the dry hit.
const DRY_SECS: f32 = 0.06;
/// Level of the three noise bands against one.
const NOISE_SUM: f32 = 0.4;
/// Spectral sustain FFT size and hop.
const FFT: usize = 1024;
const HOP: usize = FFT / 4;
/// Noise mode band splits.
const SPLIT_LOW: f32 = 300.0;
const SPLIT_HIGH: f32 = 4000.0;
/// Below this the tail and the shaper leave the signal alone (the kick).
const KEEP_LOWS_HZ: f32 = 200.0;
/// Above [`SPLIT_HIGH`] (the hats, cymbals) the tail is this much of the mids'.
const HIGH_SHARE: f32 = 0.35;

/// What Shape does to the tail: (tail amount 0..1, attack dB, sustain dB).
pub fn shaping(shape: f32) -> (f32, f32, f32) {
    let s = shape.clamp(0.0, 1.0);
    let tail = ((s - 0.5) / 0.5).max(0.0);
    let attack = ATTACK_DB * ((0.5 - s) / 0.5).max(0.0);
    (tail, attack, SUSTAIN_DB * tail)
}

#[inline]
fn flush(v: f32) -> f32 {
    if v.abs() < 1e-20 { 0.0 } else { v }
}

/// White noise −1..1 at song sample `n` (channel `c`): counter based.
#[inline]
fn noise(n: u64, c: u64) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ c.wrapping_mul(0xD1B5_4A32_D192_ED03);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

fn one_pole(hz: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * hz / sr.max(1.0)).exp()
}

/// Coefficient of a follower falling 60 dB over `secs`.
fn release(secs: f32, sr: f32) -> f32 {
    1.0 - (-6.9 / (secs.max(1e-3) * sr)).exp()
}

/// RBJ band-pass (0 dB peak).
#[derive(Debug, Clone, Copy, Default)]
struct BandPass {
    b0: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z: [[f32; 2]; 3],
}

impl BandPass {
    fn new(hz: f32, q: f32, sr: f32) -> Self {
        let w = std::f32::consts::TAU * hz / sr;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self { b0: alpha / a0, b2: -alpha / a0, a1: -2.0 * w.cos() / a0, a2: (1.0 - alpha) / a0, z: [[0.0; 2]; 3] }
    }

    /// Filter `x` on line `k` (0 = analysis, 1 and 2 = the noise channels).
    #[inline]
    fn run(&mut self, k: usize, x: f32) -> f32 {
        let z = &mut self.z[k];
        let y = self.b0 * x + z[0];
        z[0] = flush(-self.a1 * y + z[1]);
        z[1] = flush(self.b2 * x - self.a2 * y);
        y
    }
}

/// Envelope follower: instant attack, release by coefficient.
#[inline]
fn follow(env: &mut f32, x: f32, rel: f32) {
    let a = x.abs();
    *env = flush(if a > *env { a } else { *env + rel * (a - *env) });
}

struct Spectral {
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    held: Vec<f32>,
    out: [Vec<f32>; 2],
    spec_out: Vec<Complex<f32>>,
    time_out: Vec<f32>,
    /// Write position in `input` and read position in `out` (ring, FFT long).
    pos: usize,
    since_hop: usize,
}

/// The tail stage of a drum lane.
pub struct Tail {
    sr: f32,
    // Shaper followers (fast and slow).
    fast: f32,
    slow: f32,
    fast_rel: f32,
    slow_rel: f32,
    slow_att: f32,
    bands: [BandPass; BANDS],
    band_env: [f32; BANDS],
    /// Fast followers: the dry part the tail leaves alone.
    band_fast: [f32; BANDS],
    noise_fast: [f32; 3],
    dry_rel: f32,
    band_norm: [f32; BANDS],
    /// Each band's share of the tail (the highs a little).
    band_weight: [f32; BANDS],
    /// The shaper's low split per channel, and its coefficient.
    shaper_low: [f32; 2],
    keep_a: f32,
    norm_a: f32,
    low_a: f32,
    high_a: f32,
    split: [[f32; 3]; 3],
    noise_env: [f32; 3],
    noise_norm: [f32; 3],
    spectral: Spectral,
}

impl Tail {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT);
        let ifft = planner.plan_fft_inverse(FFT);
        let window = (0..FFT).map(|k| 0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / FFT as f32).cos()).collect();
        let spectrum = fft.make_output_vec();
        let spec_out = ifft.make_input_vec();
        let time_out = ifft.make_output_vec();
        let top = BAND_RANGE.1.min(0.45 * sr);
        let bands = std::array::from_fn(|b| {
            let hz = BAND_RANGE.0 * (top / BAND_RANGE.0).powf(b as f32 / (BANDS - 1) as f32);
            BandPass::new(hz, BAND_Q, sr)
        });
        Self {
            sr,
            fast: 0.0,
            slow: 0.0,
            fast_rel: release(0.04, sr),
            slow_rel: release(0.25, sr),
            slow_att: one_pole(50.0, sr),
            bands,
            band_env: [0.0; BANDS],
            band_fast: [0.0; BANDS],
            noise_fast: [0.0; 3],
            dry_rel: release(DRY_SECS, sr),
            band_norm: [0.0; BANDS],
            band_weight: std::array::from_fn(|b| if BAND_RANGE.0 * (top / BAND_RANGE.0).powf(b as f32 / (BANDS - 1) as f32) > SPLIT_HIGH { HIGH_SHARE } else { 1.0 }),
            shaper_low: [0.0; 2],
            keep_a: one_pole(KEEP_LOWS_HZ, sr),
            norm_a: one_pole(5.0, sr),
            low_a: one_pole(SPLIT_LOW, sr),
            high_a: one_pole(SPLIT_HIGH, sr),
            split: [[0.0; 3]; 3],
            noise_env: [0.0; 3],
            noise_norm: [0.0; 3],
            spectral: Spectral {
                fft,
                ifft,
                window,
                input: vec![0.0; FFT],
                frame: vec![0.0; FFT],
                spectrum,
                held: vec![0.0; FFT / 2 + 1],
                out: [vec![0.0; FFT], vec![0.0; FFT]],
                spec_out,
                time_out,
                pos: 0,
                since_hop: 0,
            },
        }
    }

    /// Shape `buf` and add its tail. `shape` is the resolved Shape value,
    /// `at` the song position of the first frame in samples, `(gain,
    /// length)` scale the tail's level and time (the group's and the tempo's).
    pub fn process(&mut self, buf: &mut [StereoSample], shape: f32, mode: TailMode, at: u64, (gain, length): (f32, f32)) {
        let (tail, attack_db, sustain_db) = shaping(shape);
        self.shape_transients(buf, attack_db, sustain_db * gain);
        if tail <= 0.0 {
            return;
        }
        let secs = (TAIL_SECS.0 + (TAIL_SECS.1 - TAIL_SECS.0) * tail) * length;
        let gain = TAIL_GAIN * tail * gain;
        match mode {
            TailMode::Vocoder => self.vocoder(buf, secs, gain, at),
            TailMode::Spectral => self.spectral(buf, secs, gain, at),
            TailMode::Noise => self.noise_bands(buf, secs, gain, at),
        }
    }

    /// The attack a little louder at low Shape, the sustain louder above
    /// its middle (the difference of a fast and a slow follower).
    fn shape_transients(&mut self, buf: &mut [StereoSample], attack_db: f32, sustain_db: f32) {
        if attack_db <= 0.0 && sustain_db <= 0.0 {
            return;
        }
        for s in buf.iter_mut() {
            let x = 0.5 * (s.left.abs() + s.right.abs());
            follow(&mut self.fast, x, self.fast_rel);
            self.slow = flush(self.slow + if x > self.slow { self.slow_att } else { self.slow_rel } * (x - self.slow));
            if self.fast < 1e-6 {
                continue;
            }
            // Positive while the hit rises above its average, negative in its decay.
            let d = (20.0 * (self.fast / self.slow.max(1e-6)).log10()).clamp(-24.0, 24.0);
            let (att, sus) = if d > 0.0 { (attack_db * (d / 12.0).min(1.0), 0.0) } else { (0.0, sustain_db * (-d / 12.0).min(1.0)) };
            let (ga, gs) = (10f32.powf(att / 20.0), 10f32.powf(sus / 20.0));
            // The sustain lifts only above the kick's band: the kick keeps its length.
            for (c, v) in [&mut s.left, &mut s.right].into_iter().enumerate() {
                let lo = &mut self.shaper_low[c];
                *lo = flush(*lo + self.keep_a * (*v - *lo));
                *v = (*lo + (*v - *lo) * gs) * ga;
            }
        }
    }

    fn vocoder(&mut self, buf: &mut [StereoSample], secs: f32, gain: f32, at: u64) {
        let rel = release(secs, self.sr);
        for (i, s) in buf.iter_mut().enumerate() {
            let x = 0.5 * (s.left + s.right);
            let n = at + i as u64;
            let (nl, nr) = (noise(n, 1), noise(n, 2));
            let mut out = [0.0f32; 2];
            for b in 0..BANDS {
                let band = &mut self.bands[b];
                let a = band.run(0, x);
                follow(&mut self.band_env[b], a, rel);
                follow(&mut self.band_fast[b], a, self.dry_rel);
                let l = band.run(1, nl);
                let r = band.run(2, nr);
                // Each band's noise normalized to its own level.
                self.band_norm[b] = flush(self.band_norm[b] + self.norm_a * (0.5 * (l.abs() + r.abs()) - self.band_norm[b]));
                // Only what rings on past the dry hit.
                let k = (self.band_env[b] - self.band_fast[b]).max(0.0) / self.band_norm[b].max(1e-4) * BAND_SUM;
                out[0] += l * k * self.band_weight[b];
                out[1] += r * k * self.band_weight[b];
            }
            s.left += flush(out[0] * gain);
            s.right += flush(out[1] * gain);
        }
    }

    fn noise_bands(&mut self, buf: &mut [StereoSample], secs: f32, gain: f32, at: u64) {
        let rel = release(secs, self.sr);
        // Three-way split: low (one-pole), high (input minus one-pole), mid.
        let split = |z: &mut [f32; 3], x: f32, low_a: f32, high_a: f32| {
            z[0] = flush(z[0] + low_a * (x - z[0]));
            z[1] = flush(z[1] + high_a * (x - z[1]));
            let (low, below_high) = (z[0], z[1]);
            [low, below_high - low, x - below_high]
        };
        for (i, s) in buf.iter_mut().enumerate() {
            let n = at + i as u64;
            let bands = split(&mut self.split[0], 0.5 * (s.left + s.right), self.low_a, self.high_a);
            let nl = split(&mut self.split[1], noise(n, 3), self.low_a, self.high_a);
            let nr = split(&mut self.split[2], noise(n, 4), self.low_a, self.high_a);
            let mut out = [0.0f32; 2];
            for b in 0..3 {
                follow(&mut self.noise_env[b], bands[b], rel);
                follow(&mut self.noise_fast[b], bands[b], self.dry_rel);
                self.noise_norm[b] = flush(self.noise_norm[b] + self.norm_a * (0.5 * (nl[b].abs() + nr[b].abs()) - self.noise_norm[b]));
                let k = (self.noise_env[b] - self.noise_fast[b]).max(0.0) / self.noise_norm[b].max(1e-4) * NOISE_SUM;
                // No low band (the kick), the highs a little.
                let w = [0.0, 1.0, HIGH_SHARE][b];
                out[0] += nl[b] * k * w;
                out[1] += nr[b] * k * w;
            }
            s.left += flush(out[0] * gain);
            s.right += flush(out[1] * gain);
        }
    }

    fn spectral(&mut self, buf: &mut [StereoSample], secs: f32, gain: f32, at: u64) {
        let sp = &mut self.spectral;
        // Per hop, a bin falls 60 dB over the tail time.
        let decay = (-6.9 * HOP as f32 / (secs * self.sr)).exp();
        for (i, s) in buf.iter_mut().enumerate() {
            sp.input[sp.pos] = 0.5 * (s.left + s.right);
            let tail = [sp.out[0][sp.pos], sp.out[1][sp.pos]];
            sp.out[0][sp.pos] = 0.0;
            sp.out[1][sp.pos] = 0.0;
            s.left += flush(tail[0] * gain);
            s.right += flush(tail[1] * gain);
            sp.pos = (sp.pos + 1) % FFT;
            sp.since_hop += 1;
            if sp.since_hop < HOP {
                continue;
            }
            sp.since_hop = 0;
            // The last FFT samples, windowed.
            for k in 0..FFT {
                sp.frame[k] = sp.input[(sp.pos + k) % FFT] * sp.window[k];
            }
            if sp.fft.process(&mut sp.frame, &mut sp.spectrum).is_err() {
                continue;
            }
            let hop_index = (at + i as u64) / HOP as u64;
            for c in 0..2 {
                for (k, (bin, held)) in sp.spectrum.iter().zip(sp.held.iter_mut()).enumerate() {
                    let mag = bin.norm();
                    if c == 0 {
                        *held = flush((*held * decay).max(mag));
                    }
                    // Only what rings on above the dry spectrum.
                    // Above the kick's band, the highs a little.
                    let hz = k as f32 * self.sr / FFT as f32;
                    let w = if hz < KEEP_LOWS_HZ { 0.0 } else if hz > SPLIT_HIGH { HIGH_SHARE } else { 1.0 };
                    let excess = (*held - mag).max(0.0) * w;
                    let phase = std::f32::consts::PI * noise(hop_index * 4096 + k as u64, 5 + c as u64);
                    sp.spec_out[k] = Complex::from_polar(excess, phase);
                }
                sp.spec_out[0].im = 0.0;
                sp.spec_out[FFT / 2].im = 0.0;
                if sp.ifft.process(&mut sp.spec_out, &mut sp.time_out).is_err() {
                    continue;
                }
                // Hann analysis and synthesis at 75 % overlap: scale by 2/3 / N.
                let scale = 2.0 / 3.0 / FFT as f32;
                for k in 0..FFT {
                    let idx = (sp.pos + k) % FFT;
                    sp.out[c][idx] += sp.time_out[k] * sp.window[k] * scale;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A noisy hit every half second, decaying in 40 ms.
    fn hits(secs: f32) -> Vec<StereoSample> {
        (0..(secs * SR) as usize)
            .map(|i| {
                let t = (i % (SR as usize / 2)) as f32 / SR;
                let v = noise(i as u64, 9) * (-t / 0.04).exp() * 0.5;
                StereoSample { left: v, right: v }
            })
            .collect()
    }

    /// RMS of the left channel from `a` to `b` seconds after each hit.
    fn after_hits(buf: &[StereoSample], a: f32, b: f32) -> f32 {
        let half = SR as usize / 2;
        let (mut sum, mut n) = (0.0, 0);
        for (i, s) in buf.iter().enumerate().skip(half) {
            let t = (i % half) as f32 / SR;
            if t >= a && t < b {
                sum += s.left * s.left;
                n += 1;
            }
        }
        (sum / n.max(1) as f32).sqrt()
    }

    /// What a stage added to `dry`.
    fn added(wet: &[StereoSample], dry: &[StereoSample]) -> Vec<StereoSample> {
        wet.iter().zip(dry).map(|(w, d)| StereoSample { left: w.left - d.left, right: w.right - d.right }).collect()
    }

    fn run(shape: f32, mode: TailMode, at: u64) -> Vec<StereoSample> {
        let mut b = hits(3.0);
        let mut t = Tail::new(SR);
        for (k, chunk) in b.chunks_mut(256).enumerate() {
            t.process(chunk, shape, mode, at + 256 * k as u64, (1.0, 1.0));
        }
        b
    }

    #[test]
    fn below_the_middle_there_is_no_tail_and_full_shape_rings_on() {
        for mode in TailMode::ALL {
            let dry = run(0.5, mode, 0);
            let wet = run(1.0, mode, 0);
            let (d, w) = (after_hits(&dry, 0.07, 0.12), after_hits(&wet, 0.07, 0.12));
            assert!(w > 1.2 * d.max(1e-6), "{mode:?}: tail {w} vs dry {d}");
            let hit = after_hits(&wet, 0.0, 0.04);
            assert!(w < 0.7 * hit, "{mode:?}: the tail stays under the hit ({w} vs {hit})");
            assert!(wet.iter().all(|s| s.left.is_finite()));
        }
    }

    #[test]
    fn the_kick_keeps_its_length() {
        // A 55 Hz kick every half second, decaying in 60 ms.
        let kick: Vec<StereoSample> = (0..(2.0 * SR) as usize)
            .map(|i| {
                let t = (i % (SR as usize / 2)) as f32 / SR;
                let v = 0.6 * (std::f32::consts::TAU * 55.0 * t).sin() * (-t / 0.06).exp();
                StereoSample { left: v, right: v }
            })
            .collect();
        for mode in TailMode::ALL {
            let mut wet = kick.clone();
            let mut t = Tail::new(SR);
            for (k, chunk) in wet.chunks_mut(256).enumerate() {
                t.process(chunk, 1.0, mode, 256 * k as u64, (1.0, 1.0));
            }
            let (d, w) = (after_hits(&kick, 0.15, 0.3), after_hits(&wet, 0.15, 0.3));
            assert!(w < 1.5 * d + 1e-4, "{mode:?}: the kick rings on ({w} vs {d})");
        }
    }

    #[test]
    fn tails_repeat_with_the_song_position() {
        for mode in TailMode::ALL {
            assert_eq!(run(0.9, mode, 0), run(0.9, mode, 0), "{mode:?}");
            assert_ne!(run(0.9, mode, 0), run(0.9, mode, 777), "{mode:?}: the noise follows the song");
        }
    }

    #[test]
    fn low_shape_lifts_the_attack_a_little() {
        let dry = hits(1.0);
        let mut b = dry.clone();
        Tail::new(SR).process(&mut b, 0.0, TailMode::Vocoder, 0, (1.0, 1.0));
        let peak = |v: &[StereoSample]| v.iter().map(|s| s.left.abs()).fold(0.0f32, f32::max);
        let lift = 20.0 * (peak(&b) / peak(&dry)).log10();
        assert!((0.3..=3.5).contains(&lift), "{lift} dB");
    }

    #[test]
    fn the_snare_rings_longest_the_hats_less_the_kick_barely() {
        // The same hit on each group's bus, alone.
        let tail_of = |g: usize| {
            let dry = hits(2.0);
            let n = dry.len();
            let mut buses = vec![StereoSample::default(); GROUPS * n];
            buses[g * n..(g + 1) * n].copy_from_slice(&dry);
            let mut main = vec![StereoSample::default(); n];
            let mut tails = VoiceTails::new(SR);
            tails.process(&mut main, &mut buses, 1.0, TailMode::Noise, 0, 1.0);
            after_hits(&added(&main, &dry), 0.02, 0.2)
        };
        let (kick, snare, hats) = (tail_of(0), tail_of(1), tail_of(2));
        assert!(snare > 1.5 * hats && hats > 2.0 * kick, "kick {kick}, snare {snare}, hats {hats}");
        assert_eq!(group(DrumVoice::Break), None);
    }

    #[test]
    fn tails_stretch_with_the_tempo() {
        let ring = |stretch: f32| {
            let dry = hits(2.0);
            let mut b = dry.clone();
            let mut t = Tail::new(SR);
            t.process(&mut b, 1.0, TailMode::Noise, 0, (1.0, stretch));
            after_hits(&added(&b, &dry), 0.12, 0.25)
        };
        assert!(ring(2.0) > 1.5 * ring(1.0), "half the tempo, a longer tail");
    }

    #[test]
    fn the_seed_picks_every_mode() {
        let modes: std::collections::BTreeSet<u8> = (0..60).map(|s| TailMode::seeded(s) as u8).collect();
        assert_eq!(modes.len(), 3);
    }
}
