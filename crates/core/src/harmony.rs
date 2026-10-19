//! Global key, chord progressions and key intent (design §4.6, D15, D22, D30).
//!
//! Generators emit scale degrees plus an octave. The renderer resolves them to
//! MIDI keys with the key active for that bar, so a key change re-voices every
//! lane without touching patterns. Key and progression changes are armed and
//! land on the first bar of the next phrase.
//!
//! Scales follow Camelot letters: minor = A (natural minor), major = B (major).
//!

use crate::key::MusicalKey;

/// Bars per phrase: key and progression changes land on phrase starts.
pub const BARS_PER_PHRASE: u64 = 4;

/// Natural minor (Aeolian), semitones from the root. Camelot A.
pub const MINOR_SCALE: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];
/// Major (Ionian), semitones from the root. Camelot B.
pub const MAJOR_SCALE: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
/// Degrees per octave.
pub const SCALE_LEN: i32 = 7;

/// Maximum chords in one progression (fixed size keeps it `Copy`).
pub const MAX_PROGRESSION_LEN: usize = 16;

/// Scale table for a key's mode.
pub fn scale_for(key: MusicalKey) -> &'static [u8; 7] {
    if key.minor { &MINOR_SCALE } else { &MAJOR_SCALE }
}

// MUSICAL: tune by ear (user: minor by default, the harmony seed picks the
// mode). Modes of the minor and the major family: scale, weight, name.
/// Minor family: natural minor (most), dorian, phrygian, harmonic minor.
pub const MINOR_MODES: [([u8; 7], f32, &str); 4] = [
    (MINOR_SCALE, 0.5, "aeolian"),
    ([0, 2, 3, 5, 7, 9, 10], 0.2, "dorian"),
    ([0, 1, 3, 5, 7, 8, 10], 0.15, "phrygian"),
    ([0, 2, 3, 5, 7, 8, 11], 0.15, "harmonic"),
];
/// Major family: major (most), mixolydian, lydian.
pub const MAJOR_MODES: [([u8; 7], f32, &str); 3] =
    [(MAJOR_SCALE, 0.5, "ionian"), ([0, 2, 4, 5, 7, 9, 10], 0.3, "mixolydian"), ([0, 2, 4, 6, 7, 9, 11], 0.2, "lydian")];
const S_MODE: u32 = 0xE10;

/// The smallest harmony seed that picks the mode named `name` (`"dorian"`,
/// `"phrygian"`, ...) in the key's family; 0 for the plain minor / major.
/// `None` for a name the family lacks.
pub fn seed_for_mode(name: &str, minor: bool) -> Option<u32> {
    let names: Vec<&str> = if minor { MINOR_MODES.iter().map(|m| m.2).collect() } else { MAJOR_MODES.iter().map(|m| m.2).collect() };
    let index = names.iter().position(|n| n.eq_ignore_ascii_case(name))? as u8;
    if index == 0 {
        return Some(0);
    }
    (1..100_000).find(|&s| mode_for(s, minor) == index)
}

/// Mode a harmony seed picks in the key's family (index into
/// [`MINOR_MODES`] or [`MAJOR_MODES`]). Seed 0 is the plain minor / major.
pub fn mode_for(seed: u32, minor: bool) -> u8 {
    if seed == 0 {
        return 0;
    }
    let h = crate::rng::hash(seed, 0, 0, 0, S_MODE);
    let pick = |w: &[f32]| crate::rng::weighted_pick(h, w).unwrap_or(0) as u8;
    if minor {
        pick(&MINOR_MODES.map(|m| m.1))
    } else {
        pick(&MAJOR_MODES.map(|m| m.1))
    }
}

/// Scale of `mode` in the key's family.
pub fn scale_of(key: MusicalKey, mode: u8) -> &'static [u8; 7] {
    if key.minor {
        &MINOR_MODES[(mode as usize).min(MINOR_MODES.len() - 1)].0
    } else {
        &MAJOR_MODES[(mode as usize).min(MAJOR_MODES.len() - 1)].0
    }
}

/// Name of `mode` in the key's family.
pub fn mode_name(key: MusicalKey, mode: u8) -> &'static str {
    if key.minor {
        MINOR_MODES[(mode as usize).min(MINOR_MODES.len() - 1)].2
    } else {
        MAJOR_MODES[(mode as usize).min(MAJOR_MODES.len() - 1)].2
    }
}

/// Resolve a scale degree to a MIDI key in `key`.
///
/// - `degree`: 0-based scale degree (0 = root). Values outside 0..7 wrap into
///   neighbouring octaves (7 = root an octave up, -1 = 7th below).
/// - `octave`: scientific octave of the key's root (MIDI 60 = C4), so degree 0
///   at octave 4 in A minor is A4 = 69.
/// - `chord_offset`: 0-based degree of the current chord root, added to
///   `degree` (generators write chord-relative degrees).
///
/// Result is clamped to 0..=127. Pure and RT-safe.
pub fn resolve_degree(key: MusicalKey, degree: i32, octave: i32, chord_offset: i32) -> u8 {
    resolve_in_scale(key, scale_for(key), degree, octave, chord_offset)
}

/// [`resolve_degree`] in a given scale of the key (a mode).
pub fn resolve_in_scale(key: MusicalKey, scale: &[u8; 7], degree: i32, octave: i32, chord_offset: i32) -> u8 {
    let d = degree + chord_offset;
    let oct_shift = d.div_euclid(SCALE_LEN);
    let idx = d.rem_euclid(SCALE_LEN) as usize;
    let midi = (octave + 1 + oct_shift) * 12 + key.root as i32 + scale[idx] as i32;
    midi.clamp(0, 127) as u8
}

/// Build a key from its Camelot position (1..=12) and mode.
pub fn key_from_camelot(number: u8, minor: bool) -> Option<MusicalKey> {
    (0..12u8).map(|r| MusicalKey::new(r, minor)).find(|k| k.camelot().0 == number)
}

/// Camelot label, e.g. "8A".
pub fn camelot_label(key: MusicalKey) -> String {
    let (n, l) = key.camelot();
    format!("{n}{l}")
}

/// A chord progression in scale degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progression {
    len: u8,
    /// 0-based chord-root degrees.
    degrees: [u8; MAX_PROGRESSION_LEN],
    /// Bars each chord lasts (>= 1).
    bars_per_chord: u8,
}

impl Progression {
    /// Build from 0-based degrees. Returns `None` when empty, too long, a
    /// degree is outside 0..7 or `bars_per_chord` is 0.
    pub fn new(degrees: &[u8], bars_per_chord: u8) -> Option<Self> {
        if degrees.is_empty()
            || degrees.len() > MAX_PROGRESSION_LEN
            || bars_per_chord == 0
            || degrees.iter().any(|&d| d as i32 >= SCALE_LEN)
        {
            return None;
        }
        let mut arr = [0u8; MAX_PROGRESSION_LEN];
        arr[..degrees.len()].copy_from_slice(degrees);
        Some(Self { len: degrees.len() as u8, degrees: arr, bars_per_chord })
    }

    /// Single-chord progression on the root.
    pub fn tonic() -> Self {
        Self { len: 1, degrees: [0; MAX_PROGRESSION_LEN], bars_per_chord: 1 }
    }

    pub fn degrees(&self) -> &[u8] {
        &self.degrees[..self.len as usize]
    }

    pub fn bars_per_chord(&self) -> u8 {
        self.bars_per_chord
    }

    /// Total bars before the progression repeats.
    pub fn cycle_bars(&self) -> u32 {
        self.len as u32 * self.bars_per_chord as u32
    }

    /// Chord degree for the `n`-th bar since the progression started.
    pub fn degree_at(&self, bars_since_start: u64) -> u8 {
        let idx = (bars_since_start / self.bars_per_chord as u64) % self.len as u64;
        self.degrees[idx as usize]
    }
}

impl Default for Progression {
    fn default() -> Self {
        Self::tonic()
    }
}

/// Harmony as seen by a generator for one bar (design §6.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonyCtx {
    pub key: MusicalKey,
    /// 0-based degree of this bar's chord root.
    pub chord_degree: u8,
    /// Index of the active progression in the style.
    pub progression: usize,
    /// Mode in the key's family ([`mode_for`]), 0 = plain minor / major.
    pub mode: u8,
}

impl HarmonyCtx {
    pub fn scale(&self) -> &'static [u8; 7] {
        scale_of(self.key, self.mode)
    }

    /// Resolve a chord-relative degree to a MIDI key.
    pub fn resolve(&self, degree: i32, octave: i32) -> u8 {
        resolve_in_scale(self.key, self.scale(), degree, octave, self.chord_degree as i32)
    }

    /// Resolve a key-relative degree (ignores the chord).
    pub fn resolve_in_key(&self, degree: i32, octave: i32) -> u8 {
        resolve_in_scale(self.key, self.scale(), degree, octave, 0)
    }
}

/// What [`HarmonyState::begin_bar`] applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HarmonyChange {
    pub key_changed: bool,
    pub progression_changed: bool,
}

/// Current key and progression, plus changes armed for the next phrase.
///
/// Owned by the bar renderer (non-RT). The renderer calls [`begin_bar`] for
/// each bar it renders, in order, then [`ctx`] for that bar.
///
/// [`begin_bar`]: HarmonyState::begin_bar
/// [`ctx`]: HarmonyState::ctx
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonyState {
    key: MusicalKey,
    progression: usize,
    /// Bar at which the active progression started (its chord 0).
    progression_start_bar: u64,
    pending_key: Option<MusicalKey>,
    pending_progression: Option<usize>,
    /// Harmony seed: picks the mode ([`mode_for`]).
    mode_seed: u32,
    pending_mode_seed: Option<u32>,
}

impl HarmonyState {
    pub fn new(key: MusicalKey, progression: usize) -> Self {
        Self::with_mode_seed(key, progression, 0)
    }

    /// Start with harmony seed `mode_seed` (the style's default mode).
    pub fn with_mode_seed(key: MusicalKey, progression: usize, mode_seed: u32) -> Self {
        Self {
            key,
            progression,
            progression_start_bar: 0,
            pending_key: None,
            pending_progression: None,
            mode_seed,
            pending_mode_seed: None,
        }
    }

    pub fn mode_seed(&self) -> u32 {
        self.mode_seed
    }

    pub fn pending_mode_seed(&self) -> Option<u32> {
        self.pending_mode_seed
    }

    /// Arm a harmony seed for the next phrase.
    pub fn request_mode_seed(&mut self, seed: u32) {
        self.pending_mode_seed = if seed == self.mode_seed { None } else { Some(seed) };
    }

    pub fn key(&self) -> MusicalKey {
        self.key
    }

    pub fn progression(&self) -> usize {
        self.progression
    }

    pub fn pending_key(&self) -> Option<MusicalKey> {
        self.pending_key
    }

    pub fn pending_progression(&self) -> Option<usize> {
        self.pending_progression
    }

    /// Arm a key change for the next phrase. Arming the current key clears it.
    pub fn request_key(&mut self, key: MusicalKey) {
        self.pending_key = if key == self.key { None } else { Some(key) };
    }

    /// Arm a progression change for the next phrase.
    pub fn request_progression(&mut self, index: usize) {
        self.pending_progression = if index == self.progression { None } else { Some(index) };
    }

    pub fn cancel_pending(&mut self) {
        self.pending_key = None;
        self.pending_progression = None;
    }

    /// Enter `bar`. On a phrase start, applies armed changes. A changed
    /// progression restarts at its first chord on that bar.
    pub fn begin_bar(&mut self, bar: u64) -> HarmonyChange {
        let mut change = HarmonyChange::default();
        if bar % BARS_PER_PHRASE != 0 {
            return change;
        }
        if let Some(k) = self.pending_key.take() {
            change.key_changed = k != self.key;
            self.key = k;
        }
        if let Some(p) = self.pending_progression.take() {
            change.progression_changed = p != self.progression;
            self.progression = p;
            self.progression_start_bar = bar;
        }
        if let Some(seed) = self.pending_mode_seed.take() {
            self.mode_seed = seed;
        }
        change
    }

    /// Harmony context for `bar`. `progressions` is the style's list; an out of
    /// range index falls back to a tonic progression.
    pub fn ctx(&self, bar: u64, progressions: &[Progression]) -> HarmonyCtx {
        let prog = progressions.get(self.progression).copied().unwrap_or_default();
        let since = bar.saturating_sub(self.progression_start_bar);
        HarmonyCtx {
            key: self.key,
            chord_degree: prog.degree_at(since),
            progression: self.progression,
            mode: mode_for(self.mode_seed, self.key.minor),
        }
    }
}

// ─── Seeded lane chords ────────────────────────────────────────────────

// MUSICAL: first draft, tune by ear. Chord-root weights per 0-based scale
// degree. The diminished chord (ii° in minor, vii° in major) never comes up.
// Minor: the minor chords (i, iv, v) carry most bars, so the key reads as
// minor (user: it sounded major).
const MINOR_CHORD_WEIGHTS: [f32; 7] = [4.0, 0.0, 1.0, 3.0, 3.0, 2.0, 2.0];
const MAJOR_CHORD_WEIGHTS: [f32; 7] = [3.0, 2.0, 1.0, 3.0, 3.0, 3.0, 0.0];
/// Extra weight of the tonic on a progression's first chord, so it mostly starts home.
const FIRST_SLOT_TONIC_BOOST: f32 = 12.0;
/// RNG stream of the chord draw (separate from every generator stream).
const S_CHORD: u32 = 0x50;

/// Chord root (0-based scale degree) of chord `slot` of a pitched lane's
/// seeded progression ([`form_chord`](crate::form::form_chord) draws its 4
/// chords from the master seed). Reseeding the lane changes them all. Stays
/// diatonic in the active mode.
pub fn seeded_chord(seed: u32, lane: u8, slot: u8, minor: bool) -> u8 {
    let mut weights = if minor { MINOR_CHORD_WEIGHTS } else { MAJOR_CHORD_WEIGHTS };
    if slot == 0 {
        weights[0] += FIRST_SLOT_TONIC_BOOST;
    }
    let h = crate::rng::hash(seed, lane, slot, 0, S_CHORD);
    crate::rng::weighted_pick(h, &weights).unwrap_or(0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmony_seed_picks_the_mode_on_the_phrase() {
        let am = MusicalKey::new(9, true);
        assert_eq!(mode_for(0, true), 0, "a fresh session plays natural minor");
        let phrygian = seed_for_mode("Phrygian", true).unwrap();
        assert_eq!(mode_for(phrygian, true), 2);
        assert_eq!(seed_for_mode("aeolian", true), Some(0));
        assert_eq!(seed_for_mode("phrygian", false), None, "not a major-family mode");
        let prog = [Progression::new(&[0], 1).unwrap()];
        assert_eq!(HarmonyState::with_mode_seed(MusicalKey::new(5, true), 0, phrygian).ctx(0, &prog).mode, 2);
        let modes: std::collections::BTreeSet<u8> = (1..200).map(|s| mode_for(s, true)).collect();
        assert_eq!(modes.len(), MINOR_MODES.len(), "seeds reach every minor mode");
        assert!((1..200).all(|s| (mode_for(s, false) as usize) < MAJOR_MODES.len()));
        // Dorian raises the 6th: F# over A.
        let dorian = (1..200).find(|&s| mode_for(s, true) == 1).unwrap();
        let mut h = HarmonyState::new(am, 0);
        h.request_mode_seed(dorian);
        h.begin_bar(1);
        assert_eq!(h.ctx(1, &[]).mode, 0, "waits for the phrase");
        h.begin_bar(4);
        let ctx = h.ctx(4, &[]);
        assert_eq!((ctx.mode, mode_name(am, ctx.mode)), (1, "dorian"));
        assert_eq!(ctx.resolve_in_key(5, 4) % 12, 6, "F#");
    }

    fn all_keys() -> impl Iterator<Item = MusicalKey> {
        (0..24u8).map(|i| MusicalKey::new(i % 12, i >= 12))
    }

    #[test]
    fn resolve_all_24_keys() {
        for key in all_keys() {
            let scale = scale_for(key);
            for octave in 1..7 {
                for deg in 0..7 {
                    let midi = resolve_degree(key, deg, octave, 0);
                    let expect = (octave + 1) * 12 + key.root as i32 + scale[deg as usize] as i32;
                    assert_eq!(midi as i32, expect, "{key} deg {deg} oct {octave}");
                    // Pitch class always belongs to the key.
                    let pc = (midi as i32 - key.root as i32).rem_euclid(12) as u8;
                    assert!(scale.contains(&pc));
                }
                // Octave wrap: degree 7 = root one octave up, -7 = one down.
                let root = resolve_degree(key, 0, octave, 0);
                assert_eq!(resolve_degree(key, 7, octave, 0), root + 12);
                assert_eq!(resolve_degree(key, -7, octave, 0), root - 12);
            }
        }
    }

    #[test]
    fn seeded_chords_are_diatonic_and_varied() {
        for minor in [true, false] {
            let dim = if minor { 1 } else { 6 };
            let mut seen = [0u32; 7];
            let mut first_tonic = 0;
            for seed in 0..2000u32 {
                for slot in 0..8u8 {
                    let c = seeded_chord(seed, 1, slot, minor);
                    assert!(c < 7 && c != dim, "degree {c}");
                    assert_eq!(c, seeded_chord(seed, 1, slot, minor), "deterministic");
                    seen[c as usize] += 1;
                    if slot == 0 && c == 0 {
                        first_tonic += 1;
                    }
                }
            }
            assert_eq!(seen.iter().filter(|&&n| n > 0).count(), 6, "{seen:?}");
            assert!(first_tonic > 1000, "first slot mostly tonic: {first_tonic}");
        }
        // Lanes sharing a seed get different progressions.
        let differ = (0..100u32).filter(|&s| seeded_chord(s, 1, 2, true) != seeded_chord(s, 2, 2, true)).count();
        assert!(differ > 50);
    }

    #[test]
    fn resolve_known_notes() {
        let am = MusicalKey::new(9, true);
        assert_eq!(resolve_degree(am, 0, 4, 0), 69); // A4
        assert_eq!(resolve_degree(am, 2, 4, 0), 72); // C5
        assert_eq!(resolve_degree(am, -1, 4, 0), 67); // G4
        let c = MusicalKey::new(0, false);
        assert_eq!(resolve_degree(c, 0, 4, 0), 60);
        assert_eq!(resolve_degree(c, 2, 4, 0), 64);
        // Chord offset: degree 0 over chord VI (5) in A minor = F5.
        assert_eq!(resolve_degree(am, 0, 4, 5), 77);
        // Chord-relative fifth (degree 4) over iv (3) in A minor: D + fifth = A5.
        assert_eq!(resolve_degree(am, 4, 4, 3), 81);
    }

    #[test]
    fn resolve_clamps() {
        let b = MusicalKey::new(11, false);
        assert_eq!(resolve_degree(b, 100, 9, 0), 127);
        assert_eq!(resolve_degree(b, -100, -1, 0), 0);
    }

    #[test]
    fn camelot_round_trip() {
        for key in all_keys() {
            let (n, l) = key.camelot();
            assert_eq!(key_from_camelot(n, l == 'A'), Some(key));
        }
        assert_eq!(key_from_camelot(13, true), None);
    }

    #[test]
    fn progression_degrees() {
        let p = Progression::new(&[0, 5, 3, 4], 1).unwrap();
        let seq: Vec<u8> = (0..8).map(|b| p.degree_at(b)).collect();
        assert_eq!(seq, [0, 5, 3, 4, 0, 5, 3, 4]);
        let p2 = Progression::new(&[0, 5], 2).unwrap();
        let seq: Vec<u8> = (0..6).map(|b| p2.degree_at(b)).collect();
        assert_eq!(seq, [0, 0, 5, 5, 0, 0]);
        assert_eq!(p2.cycle_bars(), 4);
        assert!(Progression::new(&[], 1).is_none());
        assert!(Progression::new(&[7], 1).is_none());
        assert!(Progression::new(&[0], 0).is_none());
        assert!(Progression::new(&[0; 17], 1).is_none());
    }

    #[test]
    fn pending_changes_wait_for_phrase() {
        let am = MusicalKey::new(9, true);
        let em = MusicalKey::new(4, true);
        let progs = [Progression::tonic(), Progression::new(&[0, 5, 3, 4], 1).unwrap()];
        let mut h = HarmonyState::new(am, 0);
        assert_eq!(h.begin_bar(0), HarmonyChange::default());
        h.begin_bar(1);
        h.request_key(em);
        h.request_progression(1);
        for bar in 2..4 {
            assert_eq!(h.begin_bar(bar), HarmonyChange::default());
            assert_eq!(h.ctx(bar, &progs).key, am);
            assert_eq!(h.ctx(bar, &progs).chord_degree, 0);
        }
        let c = h.begin_bar(4);
        assert!(c.key_changed && c.progression_changed);
        assert_eq!(h.key(), em);
        assert_eq!(h.pending_key(), None);
        // Progression restarts at the phrase.
        let degs: Vec<u8> = (4..9).map(|b| h.ctx(b, &progs).chord_degree).collect();
        assert_eq!(degs, [0, 5, 3, 4, 0]);
    }

    #[test]
    fn rerequesting_current_clears_pending() {
        let am = MusicalKey::new(9, true);
        let mut h = HarmonyState::new(am, 0);
        h.request_key(MusicalKey::new(4, true));
        h.request_key(am);
        assert_eq!(h.pending_key(), None);
        h.request_progression(2);
        h.cancel_pending();
        assert_eq!(h.pending_progression(), None);
    }

    #[test]
    fn out_of_range_progression_falls_back_to_tonic() {
        let h = HarmonyState::new(MusicalKey::new(0, false), 9);
        assert_eq!(h.ctx(3, &[]).chord_degree, 0);
    }

    #[test]
    fn harmony_ctx_resolves_against_chord() {
        let ctx = HarmonyCtx { key: MusicalKey::new(9, true), chord_degree: 5, progression: 0, mode: 0 };
        assert_eq!(ctx.resolve(0, 2), resolve_degree(ctx.key, 5, 2, 0));
        assert_eq!(ctx.resolve_in_key(0, 2), 45);
    }


}
