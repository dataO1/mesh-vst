//! Unison (design D38): each pitched note plays a seeded number of detuned
//! copies around the user's detune and stereo width. Those two settings are
//! the centres: each note draws its own detune and width in a seeded range
//! around them, which Variation widens, and higher notes lean wider and
//! more detuned while low notes stay close and narrow. The copies sum at
//! equal loudness: each copy starts at its own seeded phase of the note
//! (as free-running oscillators do), so they do not add up in phase on the
//! attack and fall back as they drift apart, and the gain follows how alike
//! the copies still are over a loudness window (their phases, each pair's
//! beat, where they sit in the stereo field). The mode sets how the
//! copies sit inside the detune range; it is a patch setting, the seed does
//! not change it.

use crate::rng;

/// How the copies are spread inside the detune range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnisonMode {
    /// Even steps.
    #[default]
    Linear,
    /// Clustered near the centre.
    Soft,
    /// Pushed to the edges.
    Hard,
    /// Octave and fifth layers, each slightly detuned.
    Stacked,
    /// Fixed seeded offsets.
    Random,
}

/// Most copies a note plays.
pub const MAX_UNISON: usize = 8;

/// The user's centres and the patch's mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unison {
    /// Most copies (1 = off).
    pub voices: u8,
    /// Detune at the centre, in cents (the copies span ± this).
    pub detune_cents: f32,
    /// Stereo width of the copies at the centre, 0 (mono) .. 1 (full).
    pub width: f32,
    /// How far notes draw their detune and width from the centres, 0..1
    /// (Variation sets it).
    pub range: f32,
    pub mode: UnisonMode,
    /// The sound seed: it picks each note's amount.
    pub seed: u32,
}

impl Default for Unison {
    fn default() -> Self {
        Self { voices: 1, detune_cents: 0.0, width: 1.0, range: 0.4, mode: UnisonMode::Linear, seed: 0 }
    }
}

/// One copy of a note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Copy {
    pub cents: f32,
    /// −1 (left) .. 1 (right).
    pub pan: f32,
    pub gain: f32,
    /// Where it starts into the note, in periods of the note (0..1).
    pub phase: f32,
}

const S_COUNT: u32 = 0xD50;
const S_DETUNE: u32 = 0xD51;
const S_OFFSET: u32 = 0xD52;
const S_WIDTH: u32 = 0xD53;
const S_PHASE: u32 = 0xD54;

// MUSICAL: first draft, tune by ear (user: higher notes more detune and
// width and more change, low notes less).
/// Keys at height 0 and 1 (F1 .. two octaves and a half above).
const HEIGHT_KEYS: (f32, f32) = (29.0, 59.0);
/// Detune and width scale at height 0 and 1, and the range's.
const HEIGHT_DETUNE: (f32, f32) = (0.7, 1.25);
const HEIGHT_WIDTH: (f32, f32) = (0.5, 1.0);
const HEIGHT_RANGE: (f32, f32) = (0.35, 1.4);
// MUSICAL: tune by ear (user, 2026-10-20: 7 copies at 25 cents much louder
// than 3, the attack far louder than the rest). The window a pair's
// likeness is judged over (about a loudness meter's momentary window), and
// the harmonics it hears (falling as a saw's).
const LIKENESS_SECS: f32 = 0.5;
const HARMONICS: usize = 8;
/// Harmonics further apart than this count as unrelated (they beat many
/// times over the window).
const NEAR_HZ: f32 = 20.0;
/// The gain that keeps `copies` (each at gain 1) at one centred copy's
/// loudness: each pair counts by its likeness over [`LIKENESS_SECS`] (per
/// harmonic, the mean of the two in and out of phase as they beat) times
/// its stereo overlap.
fn equal_loudness(copies: &[Copy], key: u8) -> f32 {
    let hz = 440.0 * ((key as f32 - 69.0) / 12.0).exp2();
    let weight: f32 = (1..=HARMONICS).map(|h| 1.0 / (h * h) as f32).sum();
    let pan = |p: f32| {
        let theta = (p.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        (theta.cos() * std::f32::consts::SQRT_2, theta.sin() * std::f32::consts::SQRT_2)
    };
    let mut power = 0.0;
    for a in copies {
        for b in copies {
            // Every pair of harmonics close in pitch (the same harmonic of
            // two detuned copies, or of Stacked layers an octave or a fifth
            // apart): the mean of their beat over the window, from their
            // phases at the start.
            let (fa, fb) = (hz * (a.cents / 1200.0).exp2(), hz * (b.cents / 1200.0).exp2());
            let mut alike = 0.0;
            for i in 1..=HARMONICS {
                for j in 1..=HARMONICS {
                    let (i, j) = (i as f32, j as f32);
                    let beat = i * fa - j * fb;
                    if beat.abs() > NEAR_HZ {
                        continue;
                    }
                    let x = std::f32::consts::TAU * beat * LIKENESS_SECS;
                    let theta = std::f32::consts::TAU * (i * a.phase - j * b.phase);
                    // The mean of cos(theta + x t / T) over the window.
                    let mean = if x.abs() < 1e-4 { theta.cos() } else { ((theta + x).sin() - theta.sin()) / x };
                    alike += mean / (i * j);
                }
            }
            let ((la, ra), (lb, rb)) = (pan(a.pan), pan(b.pan));
            power += alike / weight * 0.5 * (la * lb + ra * rb);
        }
    }
    // Copies that start out of phase can sum below one copy: made up, to 2.
    1.0 / power.max(0.25).sqrt()
}

fn lerp(a: (f32, f32), t: f32) -> f32 {
    a.0 + (a.1 - a.0) * t
}

impl Unison {
    /// The copies of the note `note_id` on `key`: how many, their detune,
    /// pan and gain. Returns the copies and their count.
    pub fn copies(&self, note_id: u16, key: u8) -> ([Copy; MAX_UNISON], usize) {
        let mut out = [Copy { cents: 0.0, pan: 0.0, gain: 1.0, phase: 0.0 }; MAX_UNISON];
        let max = (self.voices as usize).clamp(1, MAX_UNISON);
        if max == 1 || self.detune_cents <= 0.0 {
            return (out, 1);
        }
        let h = |s: u32, k: u8| rng::unit_f32(rng::hash(self.seed, k, (note_id & 0xFF) as u8, (note_id >> 8) as u8, s));
        let height = ((key as f32 - HEIGHT_KEYS.0) / (HEIGHT_KEYS.1 - HEIGHT_KEYS.0)).clamp(0.0, 1.0);
        let range = self.range.clamp(0.0, 1.0) * lerp(HEIGHT_RANGE, height);
        // This note's detune and width, drawn around the centres.
        let draw = |s: u32| (1.0 + range * (2.0 * h(s, 0) - 1.0)).max(0.0);
        let detune = self.detune_cents * lerp(HEIGHT_DETUNE, height) * draw(S_DETUNE);
        let width = (self.width.clamp(0.0, 1.0) * lerp(HEIGHT_WIDTH, height) * draw(S_WIDTH)).min(1.0);
        if detune <= 0.0 {
            return (out, 1);
        }
        // More or fewer copies per note (the seed), up to the limit.
        let n = (2 + ((max - 1) as f32 * h(S_COUNT, 0).powf(1.2)) as usize).min(max);
        for (k, c) in out.iter_mut().enumerate().take(n) {
            // Even positions −1..1, then shaped by the mode.
            let x = if n == 1 { 0.0 } else { -1.0 + 2.0 * k as f32 / (n - 1) as f32 };
            let cents = match self.mode {
                UnisonMode::Linear => x * detune,
                UnisonMode::Soft => x * x.abs() * detune,
                UnisonMode::Hard => x.signum() * x.abs().sqrt() * detune,
                UnisonMode::Random => (h(S_OFFSET, k as u8) * 2.0 - 1.0) * detune,
                UnisonMode::Stacked => {
                    // Root, octave up, fifth up, octave down, then again, each detuned a little.
                    const LAYERS: [f32; 4] = [0.0, 1200.0, 700.0, -1200.0];
                    LAYERS[k % LAYERS.len()] + x * detune * 0.25
                }
            };
            let phase = if k == 0 { 0.0 } else { h(S_PHASE, k as u8) };
            *c = Copy { cents, pan: x * width, gain: 1.0, phase };
        }
        let gain = equal_loudness(&out[..n], key);
        for c in out.iter_mut().take(n) {
            c.gain = gain;
        }
        (out, n)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn unison(mode: UnisonMode) -> Unison {
        Unison { voices: 8, detune_cents: 30.0, width: 0.8, range: 0.5, mode, seed: 7 }
    }

    #[test]
    fn copies_stay_in_range() {
        for mode in [UnisonMode::Linear, UnisonMode::Soft, UnisonMode::Hard, UnisonMode::Random] {
            for id in 0..200u16 {
                let (c, n) = unison(mode).copies(id, 41);
                assert!(c[..n].iter().all(|c| c.gain > 0.0 && c.gain <= 1.0));
                assert!(c[..n].iter().all(|c| c.cents.abs() <= 30.0 * 1.25 * 1.8 + 1e-3 && c.pan.abs() <= 1.0));
            }
        }
    }

    /// Loudness (dB) of the copies of a note playing a harmonic tone (eight
    /// harmonics, falling as a saw's), summed as the sampler pans them,
    /// over `secs` from the note's start.
    fn loudness(copies: &[Copy], key: u8, from: f32, secs: f32) -> f32 {
        const SR: f32 = 8000.0;
        let hz = 440.0 * ((key as f32 - 69.0) / 12.0).exp2();
        let mut power = 0.0;
        let frames = (secs * SR) as usize;
        for i in 0..frames {
            let t = from + i as f32 / SR;
            let (mut l, mut r) = (0.0, 0.0);
            for c in copies {
                let f = hz * (c.cents / 1200.0).exp2();
                let v: f32 = (1..=8).map(|h| (std::f32::consts::TAU * h as f32 * (f * t + c.phase)).sin() / h as f32).sum::<f32>() * c.gain;
                let theta = (c.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                l += v * theta.cos() * std::f32::consts::SQRT_2;
                r += v * theta.sin() * std::f32::consts::SQRT_2;
            }
            power += 0.5 * (l * l + r * r);
        }
        10.0 * (power / frames as f32).log10()
    }

    #[test]
    fn three_and_seven_copies_play_as_loud_as_one() {
        for mode in [UnisonMode::Linear, UnisonMode::Soft, UnisonMode::Hard, UnisonMode::Random, UnisonMode::Stacked] {
            for (detune, width) in [(10.0, 0.0), (25.0, 0.0), (25.0, 0.8), (60.0, 0.5)] {
                for key in [33u8, 45] {
                    let one = loudness(&[Copy { cents: 0.0, pan: 0.0, gain: 1.0, phase: 0.0 }], key, 0.0, 0.5);
                    for voices in [3u8, 7] {
                        let u = Unison { voices, detune_cents: detune, width, range: 0.0, mode, seed: 7 };
                        let notes: Vec<_> = (0..400u16).map(|id| u.copies(id, key)).filter(|(_, n)| *n == voices as usize).take(6).collect();
                        let mut attack = 0.0;
                        for (c, n) in &notes {
                            let db = loudness(&c[..*n], key, 0.0, 0.5) - one;
                            assert!(db.abs() < 0.5, "{mode:?} {voices} copies, {detune} ct, width {width}, key {key}: {db:+.1} dB");
                            attack += loudness(&c[..*n], key, 0.0, 0.05) - loudness(&c[..*n], key, 0.3, 0.6);
                        }
                        // The attack no louder than what follows, on average.
                        let attack = attack / notes.len() as f32;
                        assert!(attack < 1.5, "{mode:?} {voices} copies, {detune} ct, key {key}: attack {attack:+.1} dB over the rest");
                    }
                }
            }
        }
    }

    /// Widest detune and pan of a note.
    fn spread(u: &Unison, id: u16, key: u8) -> (f32, f32) {
        let (c, n) = u.copies(id, key);
        (c[..n].iter().map(|c| c.cents.abs()).fold(0.0, f32::max), c[..n].iter().map(|c| c.pan.abs()).fold(0.0, f32::max))
    }

    #[test]
    fn detune_and_width_are_centres_that_notes_vary_around() {
        let u = unison(UnisonMode::Linear);
        let notes: Vec<(f32, f32)> = (0..200u16).map(|id| spread(&u, id, 41)).collect();
        let mean = notes.iter().map(|n| n.0).sum::<f32>() / notes.len() as f32;
        let (lo, hi) = notes.iter().fold((f32::MAX, 0.0f32), |(a, b), n| (a.min(n.0), b.max(n.0)));
        assert!(lo < 0.85 * mean && hi > 1.15 * mean, "detune {lo}..{hi} around {mean}");
        assert_eq!(u.copies(5, 41), u.copies(5, 41), "seeded");
        // No range: every note at the centre.
        let fixed = Unison { range: 0.0, ..u };
        let d: Vec<f32> = (0..50u16).map(|id| spread(&fixed, id, 41).0).collect();
        assert!(d.iter().all(|v| (v - d[0]).abs() < 1e-3));
        // Width 0: mono copies (a reese).
        assert!((0..50u16).all(|id| spread(&Unison { width: 0.0, ..u }, id, 41).1 == 0.0));
        // Off: one copy.
        assert_eq!(Unison { voices: 1, ..u }.copies(9, 41).1, 1);
    }

    #[test]
    fn higher_notes_go_wider_and_vary_more() {
        let u = unison(UnisonMode::Linear);
        let stats = |key: u8| {
            let v: Vec<(f32, f32)> = (0..300u16).map(|id| spread(&u, id, key)).collect();
            let mean = |f: fn(&(f32, f32)) -> f32| v.iter().map(f).sum::<f32>() / v.len() as f32;
            let (md, mw) = (mean(|n| n.0), mean(|n| n.1));
            let dev = v.iter().map(|n| (n.0 - md).abs()).sum::<f32>() / v.len() as f32;
            (md, mw, dev)
        };
        let (low, high) = (stats(29), stats(60));
        assert!(high.0 > low.0 && high.1 > low.1 && high.2 > 2.0 * low.2, "low {low:?}, high {high:?}");
    }

    #[test]
    fn stacked_layers_octaves_and_fifths() {
        let u = unison(UnisonMode::Stacked);
        let (c, n) = (0..200u16).map(|id| u.copies(id, 41)).find(|(_, n)| *n >= 4).unwrap();
        let near = |v: f32| c[..n].iter().any(|c| (c.cents - v).abs() < 15.0);
        assert!(near(1200.0) && near(700.0) && near(-1200.0));
    }
}
