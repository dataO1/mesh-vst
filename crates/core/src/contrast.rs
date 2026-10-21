//! Contrast (design D49): how B answers A, and how the fills relate to them.
//!
//! A plays the knobs as set. B's notes always come from its own seed; its
//! parameters start on A's and Contrast moves them away, each in a seeded
//! direction: at 0 B plays A's settings exactly, at 1 B is the other side of
//! the question and answer (short against long, busy against sparse,
//! straight against pushed). Near a knob's end B moves inward, and less, so
//! the knob's character stays for both. C is the mild fill and derives from
//! A, D the aggressive one and derives from B; Contrast widens their moves
//! much less than B's. Variation does not move letters apart: it sets what
//! changes inside a pattern and a form.

use crate::form::{letter_move, letter_seed, second_move, Letter, FILLER_BOOSTS, LETTER_OFFSETS};
use crate::generator::MAX_GEN_PARAMS;
use crate::rng;

/// Contrast's default: B answers A clearly, not as far as it goes.
pub const CONTRAST_DEFAULT: f32 = 0.5;

/// Lowest value a letter moves a non-zero parameter to, so a block never
/// falls silent.
const LETTER_FLOOR: f32 = 0.05;

// MUSICAL: tune by ear (user, 2026-10-21: at a knob's end both letters keep
// its character, Shape 0 stays mostly stabby). Share of B's move left at a
// knob's very end, and how far from the end the full move returns.
const EDGE_KEEP: f32 = 0.5;
const EDGE_ZONE: f32 = 0.25;

// MUSICAL: tune by ear (user, 2026-10-21: C less aggressive, from A; D more
// aggressive, from B; at Contrast 0 they still vary, less than before, at 1
// about as much as before). Scale of the fills' seeded moves at Contrast 0
// and 1, and of the filler boosts.
const C_REACH: (f32, f32) = (0.25, 0.6);
const D_REACH: (f32, f32) = (0.5, 1.0);
const C_BOOST: f32 = 0.5;
const D_BOOST: f32 = 1.3;

// MUSICAL: tune by ear (user, 2026-10-21: B's Morph move ignores the Morph
// knob and wraps around the sample). B's largest Morph move at Contrast 1,
// in knob values (times the stream region: up to half the sample).
const B_MORPH: f32 = 1.25;

// MUSICAL: first draft. Chance B plays one speed step off A's at Contrast 1;
// C's and D's chance off their letter's at Contrast 0 and 1, and their
// chance to go up (fillers speed up more).
const B_RATE: f32 = 0.6;
const C_RATE: (f32, f32) = (0.1, 0.35);
const D_RATE: (f32, f32) = (0.4, 0.7);
const FILL_RATE_UP: f32 = 0.75;

// MUSICAL: first draft. Chance B's line is mirrored (its contour turned)
// and lifted an octave (mid only) at Contrast 1.
const B_CONTOUR: f32 = 0.5;
const B_REGISTER: f32 = 0.35;

const S_RATE_STEP: u32 = 0xD49;
const S_RATE_DIR: u32 = 0xD4A;
const S_CONTOUR: u32 = 0xD4B;
const S_REGISTER: u32 = 0xD4C;

fn lerp((a, b): (f32, f32), t: f32) -> f32 {
    a + (b - a) * t
}

/// B's value of entry `k` at knob value `base`: A's, moved by Contrast in
/// B's seeded direction (inward at an end, and less), or wrapped.
fn answer(k: usize, base: f32, b_seed: u32, lane: u8, contrast: f32, wrap: bool) -> f32 {
    let m = letter_move(b_seed, lane, k);
    let c = contrast.clamp(0.0, 1.0);
    if wrap {
        return base + c * B_MORPH / LETTER_OFFSETS[k].1 * m;
    }
    let d = c * m.abs();
    let mut dir = m.signum();
    let room = |dir: f32| if dir > 0.0 { 1.0 - base } else { base };
    if room(dir) < d && room(-dir) > room(dir) {
        dir = -dir;
    }
    let edge = (base.min(1.0 - base).max(0.0) / EDGE_ZONE).min(1.0);
    let keep = EDGE_KEEP + (1.0 - EDGE_KEEP) * edge * edge * (3.0 - 2.0 * edge);
    base + dir * d * keep
}

/// Letter `letter`'s value of entry `k` (of [`LETTER_OFFSETS`]) at knob
/// value `base`, before clamping. `wrap`: a position (Morph) that wraps.
fn moved(k: usize, base: f32, pattern_seed: u32, lane: u8, letter: Letter, contrast: f32, wrap: bool) -> f32 {
    let c = contrast.clamp(0.0, 1.0);
    let seed = |l: Letter| letter_seed(pattern_seed, lane, l);
    match letter {
        Letter::A => base,
        Letter::B => answer(k, base, seed(Letter::B), lane, c, wrap),
        Letter::C => base + lerp(C_REACH, c) * letter_move(seed(Letter::C), lane, k),
        Letter::D => answer(k, base, seed(Letter::B), lane, c, wrap) + lerp(D_REACH, c) * letter_move(seed(Letter::D), lane, k),
    }
}

/// The generator parameters `letter` plays, from the knob values `params`
/// (named `names`), at Contrast `contrast`; `second`: the form's second
/// pass, which moves every letter alike a little.
pub fn letter_params(
    names: &[&str],
    params: &[f32; MAX_GEN_PARAMS],
    pattern_seed: u32,
    lane: u8,
    letter: Letter,
    second: bool,
    contrast: f32,
) -> [f32; MAX_GEN_PARAMS] {
    let mut out = *params;
    for (k, (name, _)) in LETTER_OFFSETS.iter().enumerate() {
        let Some(i) = names.iter().position(|n| n == name) else { continue };
        let Some(p) = out.get_mut(i) else { continue };
        let base = *p;
        let mut v = moved(k, base, pattern_seed, lane, letter, contrast, false);
        if second {
            v += second_move(pattern_seed, lane, k);
        }
        let floor = if base > 0.0 { LETTER_FLOOR.min(base) } else { 0.0 };
        *p = v.clamp(floor, 1.0);
    }
    let boost = match letter {
        Letter::C => C_BOOST,
        Letter::D => D_BOOST,
        _ => 0.0,
    };
    for (name, b) in FILLER_BOOSTS {
        if let Some(p) = names.iter().position(|n| *n == name).and_then(|i| out.get_mut(i)) {
            *p = (*p + boost * b).clamp(0.0, 1.0);
        }
    }
    out
}

/// Letter `letter`'s Morph move (an offset in knob values, wrapping
/// around the sample, whatever the knob).
pub fn morph_move(pattern_seed: u32, lane: u8, letter: Letter, second: bool, contrast: f32) -> f32 {
    let Some(k) = LETTER_OFFSETS.iter().position(|(n, _)| *n == "morph") else { return 0.0 };
    moved(k, 0.0, pattern_seed, lane, letter, contrast, true) + if second { second_move(pattern_seed, lane, k) } else { 0.0 }
}

/// Letter `letter`'s value of a knob the lane applies itself (`"permute"`)
/// at knob value `base`; a knob at 0 stays off.
pub fn knob_value(name: &str, base: f32, pattern_seed: u32, lane: u8, letter: Letter, second: bool, contrast: f32) -> f32 {
    let Some(k) = LETTER_OFFSETS.iter().position(|(n, _)| *n == name) else { return base };
    if base <= 0.0 {
        return 0.0;
    }
    let v = moved(k, base, pattern_seed, lane, letter, contrast, false) + if second { second_move(pattern_seed, lane, k) } else { 0.0 };
    v.clamp(LETTER_FLOOR.min(base), 1.0)
}

/// One speed step off `base` with chance `p`, in a seeded direction
/// (`up`: the chance to go up), kept within half .. double.
fn step(base: i8, seed: u32, lane: u8, p: f32, up: f32) -> i8 {
    if !rng::chance(rng::hash(seed, lane, 0, 0, S_RATE_STEP), p) {
        return base;
    }
    let dir = if rng::chance(rng::hash(seed, lane, 0, 0, S_RATE_DIR), up) { 1 } else { -1 };
    if (base + dir).abs() > 1 { base - dir } else { base + dir }
}

/// The speed step `letter` plays, from A's (`a`, seeded as before): B one
/// step off with a chance rising with Contrast, C off A's, D off B's,
/// the fills more often and mostly up.
pub fn letter_rate(a: i8, pattern_seed: u32, lane: u8, letter: Letter, contrast: f32) -> i8 {
    let c = contrast.clamp(0.0, 1.0);
    let seed = |l: Letter| letter_seed(pattern_seed, lane, l);
    let b = || step(a, seed(Letter::B), lane, c * B_RATE, 0.5);
    match letter {
        Letter::A => a,
        Letter::B => b(),
        Letter::C => step(a, seed(Letter::C), lane, lerp(C_RATE, c), FILL_RATE_UP),
        Letter::D => step(b(), seed(Letter::D), lane, lerp(D_RATE, c), FILL_RATE_UP),
    }
}

/// Whether B's line is mirrored and whether it is lifted an octave (bass),
/// with chances rising with Contrast; seeded per letter, so all of B
/// answers alike.
pub fn answer_line(pattern_seed: u32, lane: u8, contrast: f32) -> (bool, bool) {
    let c = contrast.clamp(0.0, 1.0);
    let b = letter_seed(pattern_seed, lane, Letter::B);
    (rng::chance(rng::hash(b, lane, 0, 0, S_CONTOUR), c * B_CONTOUR), rng::chance(rng::hash(b, lane, 0, 0, S_REGISTER), c * B_REGISTER))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 12] =
        ["density", "complexity", "variation", "kick_mode", "register", "slide", "accent", "spread", "length", "expression", "stabs", "syncopation"];

    fn knobs(v: f32) -> [f32; MAX_GEN_PARAMS] {
        [v; MAX_GEN_PARAMS]
    }

    fn distance(a: &[f32; MAX_GEN_PARAMS], b: &[f32; MAX_GEN_PARAMS]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum()
    }

    #[test]
    fn at_contrast_0_b_plays_a_and_a_plays_the_knobs() {
        for seed in 0..200 {
            let p = knobs(0.2 + 0.6 * (seed % 7) as f32 / 6.0);
            for second in [false, true] {
                let a = letter_params(&NAMES, &p, seed, 1, Letter::A, second, 0.0);
                let b = letter_params(&NAMES, &p, seed, 1, Letter::B, second, 0.0);
                assert_eq!(a, b, "seed {seed}");
                assert_eq!(morph_move(seed, 1, Letter::A, second, 0.0), morph_move(seed, 1, Letter::B, second, 0.0));
                assert_eq!(knob_value("permute", 0.4, seed, 1, Letter::B, second, 0.0), knob_value("permute", 0.4, seed, 1, Letter::A, second, 0.0));
            }
            assert_eq!(letter_params(&NAMES, &p, seed, 1, Letter::A, false, 1.0), p, "A plays the knobs at any Contrast");
            let a = (seed % 3) as i8 - 1;
            assert_eq!(letter_rate(a, seed, 1, Letter::B, 0.0), a);
            assert_eq!(answer_line(seed, 1, 0.0), (false, false));
        }
    }

    #[test]
    fn b_moves_away_from_a_with_contrast_on_every_axis() {
        let p = knobs(0.5);
        let mut per_axis = [[0.0f32; 3]; NAMES.len()];
        let (mut rate_off, mut morph) = ([0u32; 3], [0.0f32; 3]);
        let mut lines = [0u32; 3];
        for seed in 0..400 {
            for (j, c) in [0.0, 0.5, 1.0].into_iter().enumerate() {
                let b = letter_params(&NAMES, &p, seed, 1, Letter::B, false, c);
                for (i, axis) in per_axis.iter_mut().enumerate() {
                    axis[j] += (b[i] - p[i]).abs();
                }
                rate_off[j] += (letter_rate(0, seed, 1, Letter::B, c) != 0) as u32;
                morph[j] += morph_move(seed, 1, Letter::B, false, c).abs();
                let (mirror, lift) = answer_line(seed, 1, c);
                lines[j] += mirror as u32 + lift as u32;
            }
        }
        for (i, name) in NAMES.iter().enumerate() {
            let moved = LETTER_OFFSETS.iter().any(|(n, _)| n == name);
            let a = per_axis[i];
            if moved {
                assert!(a[0] == 0.0 && a[1] > 0.0 && a[2] > a[1] * 1.5, "{name}: {a:?}");
            } else {
                assert_eq!(a, [0.0; 3], "{name} is not a letter axis");
            }
        }
        assert!(rate_off[0] == 0 && rate_off[1] > 40 && rate_off[2] > rate_off[1], "{rate_off:?}");
        assert!(morph[0] == 0.0 && morph[2] > morph[1] * 1.5, "{morph:?}");
        assert!(lines[0] == 0 && lines[2] > lines[1], "{lines:?}");
    }

    #[test]
    fn at_a_knobs_end_both_letters_keep_its_character() {
        let k = NAMES.iter().position(|n| *n == "length").unwrap();
        let (mut low, mut high) = (0.0f32, 1.0f32);
        let (mut either_way_low, mut either_way_high) = (0, 0);
        for seed in 0..400 {
            let b0 = letter_params(&NAMES, &knobs(0.0), seed, 1, Letter::B, false, 1.0)[k];
            let b1 = letter_params(&NAMES, &knobs(1.0), seed, 1, Letter::B, false, 1.0)[k];
            low = low.max(b0);
            high = high.min(b1);
            let mid = letter_params(&NAMES, &knobs(0.5), seed, 1, Letter::B, false, 1.0)[k];
            either_way_low += (mid < 0.5) as u32;
            either_way_high += (mid > 0.5) as u32;
        }
        // Shape 0: B is longer than A but still short; Shape 1 the other way.
        assert!(low > 0.0 && low <= 0.2, "{low}");
        assert!(high < 1.0 && high >= 0.8, "{high}");
        // In the middle the seed decides which letter is the long one.
        assert!(either_way_low > 100 && either_way_high > 100, "{either_way_low} {either_way_high}");
    }

    #[test]
    fn c_is_the_mild_fill_from_a_and_d_the_aggressive_one_from_b() {
        let p = knobs(0.5);
        for c in [0.0, 1.0] {
            let (mut dc, mut dd, mut dd_b) = (0.0, 0.0, 0.0);
            for seed in 0..300 {
                let b = letter_params(&NAMES, &p, seed, 1, Letter::B, false, c);
                let cc = letter_params(&NAMES, &p, seed, 1, Letter::C, false, c);
                let d = letter_params(&NAMES, &p, seed, 1, Letter::D, false, c);
                dc += distance(&cc, &p);
                dd += distance(&d, &p);
                dd_b += distance(&d, &b);
            }
            assert!(dc > 0.0, "C varies from A at Contrast {c}");
            assert!(dd > dc * 1.3, "D is the aggressive fill at Contrast {c}: {dd} vs {dc}");
            assert!(dd_b < dd || c == 0.0, "D derives from B at Contrast {c}");
        }
        // Contrast widens the fills' moves, less than B's.
        let d = |l: Letter, c: f32| (0..300).map(|s| distance(&letter_params(&NAMES, &p, s, 1, l, false, c), &p)).sum::<f32>();
        assert!(d(Letter::C, 1.0) > d(Letter::C, 0.0) && d(Letter::D, 1.0) > d(Letter::D, 0.0));
        assert!(d(Letter::C, 1.0) / d(Letter::C, 0.0) < d(Letter::B, 1.0) / d(Letter::B, 0.1));
        // Fills speed up more often than B.
        let up = |l: Letter| (0..300).filter(|&s| letter_rate(0, s, 1, l, 0.5) > 0).count();
        assert!(up(Letter::D) > up(Letter::C) && up(Letter::D) > up(Letter::B));
    }

    #[test]
    fn variation_does_not_move_letters_apart() {
        let i = NAMES.iter().position(|n| *n == "variation").unwrap();
        for seed in 0..100 {
            let mut lo = knobs(0.5);
            lo[i] = 0.0;
            let mut hi = lo;
            hi[i] = 1.0;
            let d = |p: &[f32; MAX_GEN_PARAMS]| {
                let mut a = letter_params(&NAMES, p, seed, 1, Letter::A, false, 0.6);
                let mut b = letter_params(&NAMES, p, seed, 1, Letter::B, false, 0.6);
                a[i] = 0.0;
                b[i] = 0.0;
                distance(&a, &b)
            };
            assert!((d(&lo) - d(&hi)).abs() < 1e-5, "seed {seed}");
        }
    }
}
