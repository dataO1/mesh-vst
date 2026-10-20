//! Musical times follow the host tempo. Every such constant is a note value
//! (a share of a whole note, 1/8 = an 8th) written with [`note`], which
//! gives its length in seconds at [`REF_BPM`]; the sampler and the effects
//! stretch those seconds by [`stretch`] at the host's tempo. Technical
//! times (anti-click fades, smoothing, analysis windows) stay in seconds.

/// The tempo the musical times are written for.
pub const REF_BPM: f32 = 174.0;

/// Seconds of note value `share` at [`REF_BPM`] (`note(1.0 / 8.0)` = an 8th).
pub const fn note(share: f32) -> f32 {
    share * 240.0 / REF_BPM
}

/// Factor that turns seconds at [`REF_BPM`] into seconds at `bpm`.
pub fn stretch(bpm: f64) -> f32 {
    (REF_BPM as f64 / bpm.max(1.0)) as f32
}

/// The Groove knob's feel for a lane's own times (envelopes, glides, the
/// choke, the LFO; not the effects): triplet (tighter) at low Groove,
/// straight around its middle, dotted (longer) at high Groove.
pub fn feel(groove: f32) -> f32 {
    if groove < FEEL_STRAIGHT.0 {
        2.0 / 3.0
    } else if groove <= FEEL_STRAIGHT.1 {
        1.0
    } else {
        1.5
    }
}

// MUSICAL: tune by ear. The Groove range that plays the times straight.
const FEEL_STRAIGHT: (f32, f32) = (0.35, 0.65);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_values_at_the_reference_and_other_tempos() {
        assert!((note(1.0 / 4.0) - 60.0 / REF_BPM).abs() < 1e-6, "a quarter is a beat");
        assert!((note(1.0 / 8.0) - 0.1724).abs() < 1e-3);
        assert_eq!(stretch(REF_BPM as f64), 1.0);
        assert!((stretch(87.0) - 2.0).abs() < 1e-6, "half the tempo, twice the time");
        assert_eq!((feel(0.0), feel(0.5), feel(1.0)), (2.0 / 3.0, 1.0, 1.5));
    }
}
