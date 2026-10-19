//! Unison (design D38): each pitched note plays a seeded number of detuned
//! copies, with a seeded detune and stereo width, inside the limits the user
//! sets. Some notes come out wide and detuned, others straight and narrow.
//! The copies sum at equal loudness (1/√N for uncorrelated copies). The mode
//! sets how the copies sit inside the detune range; it is a patch setting,
//! the seed does not change it.

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

/// The user's limits and the patch's mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unison {
    /// Most copies (1 = off).
    pub voices: u8,
    /// Widest detune, in cents (the copies span ± this).
    pub detune_cents: f32,
    pub mode: UnisonMode,
    /// The sound seed: it picks each note's amount.
    pub seed: u32,
}

impl Default for Unison {
    fn default() -> Self {
        Self { voices: 1, detune_cents: 0.0, mode: UnisonMode::Linear, seed: 0 }
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

impl Unison {
    /// The copies of the note `note_id`: how many, their detune, pan and gain.
    /// Returns the copies and their count.
    pub fn copies(&self, note_id: u16) -> ([Copy; MAX_UNISON], usize) {
        let mut out = [Copy { cents: 0.0, pan: 0.0, gain: 1.0 }; MAX_UNISON];
        let max = (self.voices as usize).clamp(1, MAX_UNISON);
        if max == 1 || self.detune_cents <= 0.0 {
            return (out, 1);
        }
        let h = |s: u32, k: u8| rng::unit_f32(rng::hash(self.seed, k, (note_id & 0xFF) as u8, (note_id >> 8) as u8, s));
        // MUSICAL: a third of the notes straight, the rest spread, wider ones rarer.
        let amount = h(S_DETUNE, 0);
        let amount = if amount < 0.33 { 0.0 } else { ((amount - 0.33) / 0.67).powf(1.4) };
        if amount <= 0.0 {
            return (out, 1);
        }
        let n = (2 + ((max - 1) as f32 * h(S_COUNT, 0).powf(1.2)) as usize).min(max);
        let detune = self.detune_cents * amount;
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
            *c = Copy { cents, pan: x * amount, gain };
        }
        (out, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unison(mode: UnisonMode) -> Unison {
        Unison { voices: 8, detune_cents: 40.0, mode, seed: 7 }
    }

    #[test]
    fn copies_keep_equal_loudness_and_stay_in_range() {
        for mode in [UnisonMode::Linear, UnisonMode::Soft, UnisonMode::Hard, UnisonMode::Random] {
            for id in 0..200u16 {
                let (c, n) = unison(mode).copies(id);
                let power: f32 = c[..n].iter().map(|c| c.gain * c.gain).sum();
                assert!((power - 1.0).abs() < 1e-4, "{mode:?} {id}: {power}");
                assert!(c[..n].iter().all(|c| c.cents.abs() <= 40.0 + 1e-3 && c.pan.abs() <= 1.0));
            }
        }
    }

    #[test]
    fn notes_vary_from_straight_to_wide() {
        let u = unison(UnisonMode::Linear);
        let counts: Vec<usize> = (0..200u16).map(|id| u.copies(id).1).collect();
        assert!(counts.contains(&1), "some notes play straight");
        assert!(counts.iter().any(|&n| n >= 4), "some notes play wide");
        assert_eq!(u.copies(5), u.copies(5), "seeded");
        // Off: one copy.
        assert_eq!(Unison { voices: 1, ..u }.copies(9).1, 1);
    }

    #[test]
    fn stacked_layers_octaves_and_fifths() {
        let u = unison(UnisonMode::Stacked);
        let (c, n) = (0..200u16).map(|id| u.copies(id)).find(|(_, n)| *n >= 4).unwrap();
        let near = |v: f32| c[..n].iter().any(|c| (c.cents - v).abs() < 15.0);
        assert!(near(1200.0) && near(700.0) && near(-1200.0));
    }
}
