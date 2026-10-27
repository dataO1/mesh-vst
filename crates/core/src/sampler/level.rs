//! The loudness a note plays at (D64): before a voice starts, what it will
//! play is measured, the stretch of the sample its start, rate, direction
//! and loops reach, through its amp envelope, its stab kick and its lowpass,
//! over the time it sounds (its gate, until the envelope falls to
//! [`FLOOR`], or the sample's end; the release is left out). The note's gain
//! then brings that to its sample's loudness ([`SampleData::note_lufs`]), so
//! stabs, held notes and hits of a lane come out alike before the effects;
//! velocity and accents scale it after. RT-safe: it reads the sample's
//! power table ([`SampleData::power_at`]) at a few points.

use super::data::SampleData;
use super::params::{exp_coef, LaneSamplerParams, CUTOFF_MAX_HZ, CUTOFF_MIN_HZ, CUTOFF_OFFSET_OCTAVES};
use super::voice::{mod_env_value, VoiceInit, HELD_CUTOFF_SUSTAIN, HELD_KICK, PUNCH_CUTOFF, PUNCH_ENV_STRETCH, PUNCH_GAIN, PUNCH_SECS};
use crate::loudness::{BAND_CENTRES_HZ, POWER_BANDS};

// MUSICAL: tune by ear (user, 2026-10-27: the window follows the sound
// actually played and its envelope; stabs with a little release came out
// very silent).
/// The envelope level the measured time ends at (−20 dB): a stab counts
/// while it is heard, not through its faint tail.
const FLOOR: f32 = 0.1;
/// The longest stretch measured (a held note's first seconds stand for it).
const MAX_SECS: f64 = 2.0;
/// Points measured at most per note.
const POINTS: usize = 64;
/// The most a note is cut or boosted, in dB.
pub(crate) const RANGE_DB: f32 = 24.0;

/// What the measure needs of the lane: output rate, musical time (samples
/// per second, [`crate::tempo`]), samples per tick, the lane's parameters
/// and the CC filter shift in octaves (the Tone knob's) at the note's start.
pub(crate) struct Timing<'a> {
    pub sr: f32,
    pub ts: f32,
    pub spt: f64,
    pub params: &'a LaneSamplerParams,
    pub cc_cutoff_oct: f32,
}

/// The time voice `v` sounds for, in output frames.
fn window(v: &VoiceInit, t: &Timing, pos: f64, dir: f64) -> f64 {
    let mut w = MAX_SECS * t.sr as f64;
    if v.held && v.gate_ticks.is_finite() {
        w = w.min(v.gate_ticks * t.spt);
    }
    let looping = (v.held && v.looping) || (v.section.is_some() && !v.reverse);
    if !looping {
        let left = if dir > 0.0 { v.end as f64 - pos } else { pos - v.start as f64 };
        w = w.min(left / v.rate.max(1e-6));
    }
    let (s, c) = (v.sustain, exp_coef(v.decay, t.ts));
    if s < FLOOR && c < 1.0 {
        let attack = (v.attack * t.ts) as f64;
        w = w.min(attack + (((FLOOR - s) / (1.0 - s)).ln() / c.ln()) as f64);
    }
    w.max(1.0)
}

/// The sample frame voice `v` reads `tt` output frames after its start at
/// `pos` going `dir`, through its note section or its zone's loop.
fn frame_at(v: &VoiceInit, pos: f64, dir: f64, tt: f64) -> usize {
    let mut p = pos + dir * v.rate * tt;
    let wrap = |p: f64, lo: f64, len: f64, ping_pong: bool| {
        let u = (p - lo).rem_euclid(if ping_pong { 2.0 * len } else { len });
        lo + if ping_pong && u > len { 2.0 * len - u } else { u }
    };
    if let Some(s) = v.section.filter(|s| !v.reverse && s.len >= 1.0) {
        if p >= s.lo + s.len {
            p = wrap(p, s.lo, s.len, s.ping_pong);
        }
    } else if v.held && v.looping {
        let (lo, hi) = v.loop_range.map_or((v.start as f64, v.end as f64), |(a, b)| (a as f64, b as f64));
        if hi - lo >= 1.0 && (p >= hi || p < lo) {
            p = wrap(p, lo, hi - lo, false);
        }
    }
    p.clamp(v.start as f64, (v.end.max(v.start + 1) - 1) as f64) as usize
}

/// The amp envelope and the stab kick of voice `v`, `tt` output frames in.
fn level_at(v: &VoiceInit, t: &Timing, tt: f64) -> f32 {
    let attack = v.attack * t.ts;
    let env = if (tt as f32) < attack {
        let x = tt as f32 / attack;
        let pow = t.params.attack_curve;
        if pow > 1.0 { 1.0 - (1.0 - x).powf(pow) } else { (x * std::f32::consts::FRAC_PI_2).sin() }
    } else {
        let c = exp_coef(v.decay, t.ts);
        v.sustain + (1.0 - v.sustain) * c.powf(tt as f32 - attack.max(0.0))
    };
    let kick = v.punch.max(HELD_KICK * v.body);
    env * (1.0 + PUNCH_GAIN * kick * (-(tt as f32 / t.ts) / PUNCH_SECS).exp())
}

/// The lowpass cutoff of voice `v`, `tt` output frames in (its offset, the
/// CC shift and its mod envelope; the LFO and the zone's own CCs aside).
fn cutoff_at(v: &VoiceInit, t: &Timing, tt: f64) -> f32 {
    let p = t.params;
    let mut oct = v.cutoff_offset * CUTOFF_OFFSET_OCTAVES + t.cc_cutoff_oct;
    let env = &p.modulation.env;
    if env.is_on() && env.to_cutoff != 0.0 {
        let stretch = 1.0 + PUNCH_ENV_STRETCH * v.punch;
        let m = mod_env_value(env, 0.0, tt as f32 / t.ts / stretch, 0.0, false);
        let m = if v.sustain > 0.0 { HELD_CUTOFF_SUSTAIN + (1.0 - HELD_CUTOFF_SUSTAIN) * m } else { m };
        oct += m * env.to_cutoff * p.expression.clamp(0.0, 2.0) * (1.0 + PUNCH_CUTOFF * v.punch) / 1200.0;
    }
    (p.cutoff_hz * oct.exp2()).clamp(CUTOFF_MIN_HZ, CUTOFF_MAX_HZ.min(0.45 * t.sr))
}

/// The mean K-weighted power (L + R) voice `v` plays of `data` while it
/// sounds, at unity gain; `None` when it plays nothing.
pub(crate) fn note_power(v: &VoiceInit, data: &SampleData, t: &Timing) -> Option<f32> {
    if v.end <= v.start {
        return None;
    }
    let len = (v.end - v.start) as f64;
    let (pos, dir) = if v.reverse {
        ((v.end as f64 - 1.0 - v.offset * len).max(v.start as f64), -1.0)
    } else {
        ((v.start as f64 + v.offset * len).min(v.end as f64 - 1.0), 1.0)
    };
    let w = window(v, t, pos, dir);
    let n = POINTS.min(w as usize).max(1);
    let mut acc = 0.0f64;
    for j in 0..n {
        let tt = (j as f64 + 0.5) * w / n as f64;
        let p = data.power_at(frame_at(v, pos, dir, tt));
        let fc = cutoff_at(v, t, tt);
        // The share the lowpass passes, by the bands (a 12 dB/oct lowpass's
        // power response at each band's centre), of the whole.
        let (mut through, mut all) = (0.0f32, 0.0f32);
        for k in 0..POWER_BANDS {
            let r = BAND_CENTRES_HZ[k] / fc;
            through += p[k] / (1.0 + r * r * r * r);
            all += p[k];
        }
        let heard = if all > 0.0 { p[POWER_BANDS] * through / all } else { 0.0 };
        let l = level_at(v, t, tt);
        acc += (l * l * heard) as f64;
    }
    let p = (acc / n as f64) as f32;
    (p > 1e-12).then_some(p)
}

/// The gain that brings a note of mean power `power` to `lufs`.
pub(crate) fn match_gain(power: f32, lufs: f32) -> f32 {
    let target = crate::loudness::lufs_to_power(lufs as f64) as f32;
    let max = 10f32.powf(RANGE_DB / 20.0);
    (target / power).sqrt().clamp(1.0 / max, max)
}
