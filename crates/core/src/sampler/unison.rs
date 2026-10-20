//! Unison (design D38): each pitched note plays a seeded number of detuned
//! copies around the user's detune and stereo width. Those two settings are
//! the centres: each note draws its own detune and width in a seeded range
//! around them, which Variation widens, and higher notes lean wider and
//! more detuned while low notes stay close and narrow. The copies sum at
//! equal loudness (1/√N for uncorrelated copies). The mode sets how the
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
}

const S_COUNT: u32 = 0xD50;
const S_DETUNE: u32 = 0xD51;
const S_OFFSET: u32 = 0xD52;
const S_WIDTH: u32 = 0xD53;

// MUSICAL: first draft, tune by ear (user: higher notes more detune and
// width and more change, low notes less).
/// Keys at height 0 and 1 (F1 .. two octaves and a half above).
const HEIGHT_KEYS: (f32, f32) = (29.0, 59.0);
/// Detune and width scale at height 0 and 1, and the range's.
const HEIGHT_DETUNE: (f32, f32) = (0.7, 1.25);
const HEIGHT_WIDTH: (f32, f32) = (0.5, 1.0);
const HEIGHT_RANGE: (f32, f32) = (0.35, 1.4);

fn lerp(a: (f32, f32), t: f32) -> f32 {
    a.0 + (a.1 - a.0) * t
}

impl Unison {
    /// The copies of the note `note_id` on `key`: how many, their detune,
    /// pan and gain. Returns the copies and their count.
    pub fn copies(&self, note_id: u16, key: u8) -> ([Copy; MAX_UNISON], usize) {
        let mut out = [Copy { cents: 0.0, pan: 0.0, gain: 1.0 }; MAX_UNISON];
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
        let gain = 1.0 / (n as f32).sqrt();
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
            *c = Copy { cents, pan: x * width, gain };
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
    fn copies_keep_equal_loudness_and_stay_in_range() {
        for mode in [UnisonMode::Linear, UnisonMode::Soft, UnisonMode::Hard, UnisonMode::Random] {
            for id in 0..200u16 {
                let (c, n) = unison(mode).copies(id, 41);
                let power: f32 = c[..n].iter().map(|c| c.gain * c.gain).sum();
                assert!((power - 1.0).abs() < 1e-4, "{mode:?} {id}: {power}");
                assert!(c[..n].iter().all(|c| c.cents.abs() <= 30.0 * 1.25 * 1.8 + 1e-3 && c.pan.abs() <= 1.0));
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
