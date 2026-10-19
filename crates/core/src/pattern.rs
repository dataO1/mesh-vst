//! One lane's bar, rendered from its seeds, form and parameters (design D6).
//!
//! [`PatternLane::render`] is a pure function of the lane and the bar index:
//! the same inputs always give the same events, so a DAW loop, a scrub and a
//! bounce all play the same notes. It follows Mesh live's bar renderer for one
//! lane: the form places a letter on the bar, the letter derives its seed,
//! filler letters push some parameters up, a primed letter takes its second
//! half from a derived seed, and the letter's feel and the bass's block speed
//! are applied last. A plugin instance plays alone, so the drum onsets the
//! Mesh bass follows are empty here.

use std::collections::BTreeMap;

use crate::event::{flags, BarEvents, EventKind};
use crate::form::{
    form_chord, letter_offset, letter_seed, prime_seed, splice_second_half, Feel, Form, FormPos, Letter, Play, SeedClass,
    DEFAULT_FORMS, FILLER_BOOSTS, LETTER_OFFSETS, NOTE_JITTER_SHARE,
};
use crate::generator::bass::{self, BassGenerator, BassRules};
use crate::generator::blocks::defaults_from_spec;
use crate::generator::drums::{self, DrumGenerator, DrumRules};
use crate::generator::{idiom, BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use crate::harmony::{HarmonyCtx, BARS_PER_PHRASE};
use crate::key::MusicalKey;
use crate::role::Role;
use crate::{rng, STEPS_PER_BAR, TICKS_PER_BAR};

/// Lane index the generators' seeds are hashed with. Mesh numbered its lanes
/// (0 = drums, 1 = bass); keeping the numbers keeps the seeds' sound.
pub const fn lane_index(role: Role) -> u8 {
    match role {
        Role::Drums => 0,
        _ => 1,
    }
}

/// Lowest value a letter moves a non-zero parameter to.
const LETTER_FLOOR: f32 = 0.05;

// MUSICAL: from Mesh live (user: the seed and the form set the bass speed per
// block). Speed steps (exponent on the centre speed) and their weights: main
// letters stay home more, fillers speed up more.
const BLOCK_RATE_MAIN: [(i8, f32); 3] = [(0, 0.65), (1, 0.25), (-1, 0.1)];
const BLOCK_RATE_FILLER: [(i8, f32); 3] = [(0, 0.35), (1, 0.45), (-1, 0.2)];
const S_BLOCK_RATE: u32 = 0xF51;
/// RNG stream of the sound seed a filler letter derives.
const S_SOUND: u32 = 0xF52;

/// One rendered bar.
#[derive(Clone, Default)]
pub struct RenderedBar {
    pub events: BarEvents,
    /// Drums: the voices the seed has on (bit per [`DrumVoice`](crate::generator::layout::DrumVoice)).
    pub voices: u8,
    /// Letter index 0..=3 (A..D) of the bar's first part.
    pub letter: u8,
}

/// A lane's render inputs.
pub struct PatternLane {
    role: Role,
    generator: Box<dyn LaneGenerator>,
    layout: DrumLayout,
    /// Index 0: one bar of A forever (or a custom form), then the default forms.
    forms: Vec<Form>,
    form: usize,
    /// Seed of the groove (letters, rhythm, pitch).
    pub pattern_seed: u32,
    /// Seed of the sample picks.
    pub sound_seed: u32,
    /// Speed exponent on top of the seeded block speed (bass): −1 half, 1 double.
    pub rate: i8,
    /// Mode in the key's family (0 = plain minor or major).
    pub mode: u8,
    /// Bars of a phrase: the drums fill its last bar (D25).
    pub phrase_bars: u32,
    /// Generator parameters, indexed like [`LaneGenerator::param_names`].
    pub params: [f32; MAX_GEN_PARAMS],
}

impl PatternLane {
    /// A drum lane on `layout` with the built-in archetypes.
    pub fn drums(layout: DrumLayout, pattern_seed: u32, sound_seed: u32) -> Self {
        let none = BTreeMap::new();
        let (archetypes, _) = idiom::drum_archetypes(&[], None);
        let defaults = defaults_from_spec(&drums::PARAM_NAMES, &drums::PARAM_DEFAULTS, &none);
        let generator = DrumGenerator::new(archetypes, layout, defaults, DrumRules::from_spec(&none));
        Self::new(Role::Drums, Box::new(generator), layout, pattern_seed, sound_seed, defaults)
    }

    /// A bass lane with the built-in rules.
    pub fn bass(pattern_seed: u32, sound_seed: u32) -> Self {
        let none = BTreeMap::new();
        let defaults = defaults_from_spec(&bass::PARAM_NAMES, &bass::PARAM_DEFAULTS, &none);
        let generator = BassGenerator::new(defaults, BassRules::from_spec(&none), Vec::new());
        Self::new(Role::Bass, Box::new(generator), DrumLayout::standard(), pattern_seed, sound_seed, defaults)
    }

    fn new(
        role: Role,
        generator: Box<dyn LaneGenerator>,
        layout: DrumLayout,
        pattern_seed: u32,
        sound_seed: u32,
        params: [f32; MAX_GEN_PARAMS],
    ) -> Self {
        let mut forms = vec![Form::single()];
        forms.extend(DEFAULT_FORMS.iter().map(|f| Form::parse(f).expect("built-in form parses")));
        Self { role, generator, layout, forms, form: 0, pattern_seed, sound_seed, rate: 0, mode: 0, phrase_bars: 8, params }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn layout(&self) -> &DrumLayout {
        &self.layout
    }

    pub fn param_names(&self) -> &'static [&'static str] {
        self.generator.param_names()
    }

    /// Set a generator parameter by name. Unknown names are ignored.
    pub fn set_param(&mut self, name: &str, value: f32) {
        if let Some(i) = self.param_names().iter().position(|n| *n == name) {
            self.params[i] = value.clamp(0.0, 1.0);
        }
    }

    pub fn form(&self) -> &Form {
        &self.forms[self.form]
    }

    /// Play `form` (it takes slot 0).
    pub fn set_form(&mut self, form: Form) {
        self.forms[0] = form;
        self.form = 0;
    }

    /// Play form `index`: 0 = slot 0 (one bar of A unless [`Self::set_form`]
    /// put a form there), 1..=10 = [`DEFAULT_FORMS`]. RT-safe.
    pub fn select_form(&mut self, index: usize) {
        self.form = index.min(self.forms.len() - 1);
    }

    /// Render `bar` (0 = the first bar of the song, or of the note in
    /// retrigger mode) in `key` (the held note's pitch class and the scale
    /// family) into `out`.
    pub fn render(&self, bar: u64, key: MusicalKey, out: &mut RenderedBar, scratch: &mut Scratch) {
        let lane = lane_index(self.role);
        let (parts, n_parts) = self.forms[self.form].parts(bar, self.pattern_seed, lane);
        let pos = parts[0].pos;
        let seed = letter_seed(self.pattern_seed, lane, pos.letter);
        let mut harmony = HarmonyCtx { key, chord_degree: 0, progression: 0, mode: self.mode };
        if self.role != Role::Drums {
            harmony.chord_degree = form_chord(self.pattern_seed, lane, &pos, seed, key.minor);
        }
        let ctx = PartCtx {
            lane,
            bar,
            bar_in_phrase: (bar % BARS_PER_PHRASE) as u8,
            phrase: (bar / BARS_PER_PHRASE) as u32,
            harmony,
        };
        out.events.clear();
        out.voices = self.render_part(&ctx, &pos, &mut out.events, &mut scratch.second);
        out.letter = pos.letter.index();
        for p in &parts[1..n_parts] {
            scratch.part.clear();
            self.render_part(&ctx, &p.pos, &mut scratch.part, &mut scratch.second);
            splice_range(&mut out.events, &scratch.part, p.from, p.to, &mut scratch.second);
        }
    }

    fn render_part(&self, c: &PartCtx, pos: &FormPos, out: &mut BarEvents, scratch: &mut BarEvents) -> u8 {
        // A plugin instance has no drum lane to break with: break-only
        // segments rest like rests.
        if matches!(pos.play, Play::Rest | Play::Breaks) {
            out.clear();
            return 0;
        }
        let seed = letter_seed(self.pattern_seed, c.lane, pos.letter);
        let class = pos.letter.class();
        let mut params = self.params;
        let names = self.generator.param_names();
        vary_letter(names, seed, c.lane, pos.letter, pos.second, &mut params);
        if class == SeedClass::Filler {
            boost_filler(names, &mut params);
        }
        // Drums: the Swing knob swings every off 16th of the bar (breaks,
        // ghosts, percussion and tops too), on top of the letter's feel, so
        // the generator itself plays straight.
        let knob_swing = match names.iter().position(|n| *n == "swing") {
            Some(i) if self.role == Role::Drums => std::mem::take(&mut params[i]),
            _ => 0.0,
        };
        // The drum lane fills the last bar of a filler segment, and the last
        // bar of every phrase (D25).
        let phrase_end = self.phrase_bars > 1 && (c.bar + 1) % self.phrase_bars as u64 == 0;
        let fill = self.role == Role::Drums && ((class == SeedClass::Filler && pos.last_of_segment) || phrase_end);
        let no_mask = [0u8; STEPS_PER_BAR];
        let sound_seed = match class {
            SeedClass::Main => self.sound_seed,
            SeedClass::Filler => rng::hash(self.sound_seed, c.lane, pos.letter.index(), 0, S_SOUND) as u32,
        };
        let mut bar_ctx = BarCtx {
            seed,
            sound_seed,
            class,
            lane: c.lane,
            role: self.role,
            bar_in_phrase: c.bar_in_phrase,
            phrase: c.phrase,
            fill,
            half_time: false,
            params: &params,
            harmony: c.harmony,
            drum_mask: &no_mask,
            jitter: note_jitter(names),
            bass_onsets: 0,
            bass_phrase: [0; 4],
            drum_fill: false,
        };
        self.generator.render_bar(&bar_ctx, out);
        out.sort();
        let voices = self.generator.voice_mask(&bar_ctx);
        if pos.prime {
            // A': same seed, a different second half.
            bar_ctx.seed = prime_seed(seed, c.lane);
            scratch.clear();
            self.generator.render_bar(&bar_ctx, scratch);
            scratch.sort();
            splice_second_half(out, scratch);
        }
        let mut feel = Feel::of(seed, c.lane, class);
        feel.swing = (feel.swing + knob_swing).min(1.0);
        feel.apply(out, seed, c.lane);
        if self.role != Role::Drums {
            let block = if self.role == Role::Bass { block_rate(seed, c.lane, class) } else { 0 };
            apply_speed(out, self.rate + block, c.bar, scratch);
        }
        voices
    }
}

/// Buffers [`PatternLane::render`] works in, allocated once.
#[derive(Clone, Default)]
pub struct Scratch {
    part: BarEvents,
    second: BarEvents,
}

#[derive(Debug, Clone, Copy)]
struct PartCtx {
    lane: u8,
    bar: u64,
    bar_in_phrase: u8,
    phrase: u32,
    harmony: HarmonyCtx,
}

/// Speed step of a bass block, seeded by its letter seed.
fn block_rate(seed: u32, lane: u8, class: SeedClass) -> i8 {
    let table = if class == SeedClass::Main { &BLOCK_RATE_MAIN } else { &BLOCK_RATE_FILLER };
    let weights = table.map(|t| t.1);
    table[rng::weighted_pick(rng::hash(seed, lane, 0, 0, S_BLOCK_RATE), &weights).unwrap_or(0)].0
}

/// Replace the events of `out` in ticks `from..to` with those of `part`.
fn splice_range(out: &mut BarEvents, part: &BarEvents, from: u16, to: u16, scratch: &mut BarEvents) {
    scratch.clear();
    for e in out.as_slice().iter().filter(|e| e.tick < from || e.tick >= to) {
        scratch.push(*e);
    }
    for e in part.as_slice().iter().filter(|e| e.tick >= from && e.tick < to) {
        scratch.push(*e);
    }
    scratch.sort();
    out.clear();
    for e in scratch.as_slice() {
        out.push(*e);
    }
}

/// Move a letter's parameters around their values ([`LETTER_OFFSETS`]).
fn vary_letter(names: &[&str], seed: u32, lane: u8, letter: Letter, second: bool, params: &mut [f32; MAX_GEN_PARAMS]) {
    for (k, (name, _)) in LETTER_OFFSETS.iter().enumerate() {
        if let Some(i) = names.iter().position(|n| n == name) {
            if let Some(p) = params.get_mut(i) {
                // A parameter above 0 stays above it, so a block never falls silent.
                let floor = if *p > 0.0 { LETTER_FLOOR.min(*p) } else { 0.0 };
                *p = (*p + letter_offset(seed, lane, letter, k, second)).clamp(floor, 1.0);
            }
        }
    }
}

/// Per-note jitter amplitude per parameter ([`NOTE_JITTER_SHARE`] of the
/// letter offsets).
fn note_jitter(names: &[&str]) -> [f32; MAX_GEN_PARAMS] {
    let mut out = [0.0; MAX_GEN_PARAMS];
    for (name, max) in LETTER_OFFSETS {
        if let Some(i) = names.iter().position(|n| *n == name) {
            if let Some(j) = out.get_mut(i) {
                *j = NOTE_JITTER_SHARE * max;
            }
        }
    }
    out
}

/// Push a filler bar's parameters up ([`FILLER_BOOSTS`]).
fn boost_filler(names: &[&str], params: &mut [f32; MAX_GEN_PARAMS]) {
    for (name, boost) in FILLER_BOOSTS {
        if let Some(i) = names.iter().position(|n| *n == name) {
            if let Some(p) = params.get_mut(i) {
                *p = (*p + boost).clamp(0.0, 1.0);
            }
        }
    }
}

/// Play the bar at `2^exponent` speed (clamped to −2..=1). Below 1 each bar
/// plays the next part of the rendered bar stretched over the whole bar; a
/// note held over the part start comes back on its first tick. 1 plays the
/// bar compressed into each half.
fn apply_speed(out: &mut BarEvents, exponent: i8, bar: u64, scratch: &mut BarEvents) {
    let half = TICKS_PER_BAR / 2;
    match exponent.clamp(-2, 1) {
        0 => return,
        e if e < 0 => {
            let k = 1u16 << (-e) as u16;
            let part = TICKS_PER_BAR / k;
            let from = (bar % k as u64) as u16 * part;
            scratch.clear();
            for e in out.as_slice().iter().filter(|e| {
                e.kind == EventKind::NoteOn && e.gate > 0 && e.tick < from && e.tick + e.gate > from
            }) {
                let mut e = *e;
                e.gate = (e.tick + e.gate - from).min(part) * k;
                e.tick = 0;
                e.flags &= !(flags::TIE | flags::ACCENT);
                e.note_id = e.note_id.wrapping_add(0x2000);
                scratch.push(e);
            }
            for e in out.as_slice().iter().filter(|e| e.tick >= from && e.tick < from + part) {
                let mut e = *e;
                e.tick = (e.tick - from) * k;
                e.gate = e.gate.saturating_mul(k).min(TICKS_PER_BAR - e.tick);
                scratch.push(e);
            }
        }
        _ => {
            scratch.clear();
            for offset in [0, half] {
                for e in out.as_slice() {
                    let mut e = *e;
                    e.tick = e.tick / 2 + offset;
                    e.gate = if e.gate > 0 { (e.gate / 2).max(1) } else { 0 };
                    if offset > 0 {
                        // Distinct voices for the repeat (NoteOff matching).
                        e.note_id = e.note_id.wrapping_add(0x4000);
                    }
                    scratch.push(e);
                }
            }
            scratch.sort();
        }
    }
    std::mem::swap(out, scratch);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_all(lane: &PatternLane, bars: u64) -> Vec<Vec<crate::event::LiveEvent>> {
        let key = MusicalKey::new(5, true);
        let mut out = RenderedBar::default();
        let mut scratch = Scratch::default();
        (0..bars)
            .map(|b| {
                lane.render(b, key, &mut out, &mut scratch);
                out.events.as_slice().to_vec()
            })
            .collect()
    }

    #[test]
    fn a_bar_is_a_pure_function_of_its_inputs() {
        for lane in [PatternLane::drums(DrumLayout::standard(), 7, 3), PatternLane::bass(7, 3)] {
            let a = render_all(&lane, 32);
            let key = MusicalKey::new(5, true);
            let mut out = RenderedBar::default();
            let mut scratch = Scratch::default();
            // Any bar, in any order, renders the same events again.
            for b in [17u64, 3, 31, 0, 17] {
                lane.render(b, key, &mut out, &mut scratch);
                assert_eq!(out.events.as_slice(), a[b as usize].as_slice(), "bar {b}");
            }
            assert!(a.iter().any(|bar| !bar.is_empty()), "{:?} plays", lane.role());
        }
    }

    #[test]
    fn seeds_change_the_groove() {
        let base = render_all(&PatternLane::drums(DrumLayout::standard(), 1, 1), 8);
        let other = render_all(&PatternLane::drums(DrumLayout::standard(), 2, 1), 8);
        assert_ne!(base, other);
        let bass = render_all(&PatternLane::bass(1, 1), 8);
        let bass2 = render_all(&PatternLane::bass(2, 1), 8);
        assert_ne!(bass, bass2);
    }

    #[test]
    fn forms_place_filler_letters() {
        let mut lane = PatternLane::drums(DrumLayout::standard(), 5, 5);
        lane.set_form(Form::parse(DEFAULT_FORMS[0]).unwrap());
        let key = MusicalKey::new(0, true);
        let mut out = RenderedBar::default();
        let mut scratch = Scratch::default();
        let letters: Vec<u8> = (0..16)
            .map(|b| {
                lane.render(b, key, &mut out, &mut scratch);
                out.letter
            })
            .collect();
        // `A15 C1`: fifteen bars of A, then a C.
        assert!(letters[..15].iter().all(|&l| l == 0), "{letters:?}");
        assert_eq!(letters[15], 2);
    }

    #[test]
    fn bass_follows_the_root() {
        let lane = PatternLane::bass(9, 9);
        let mut out = RenderedBar::default();
        let mut scratch = Scratch::default();
        let keys = |root: u8, out: &mut RenderedBar, scratch: &mut Scratch| -> Vec<i32> {
            lane.render(0, MusicalKey::new(root, true), out, scratch);
            out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| e.key as i32).collect()
        };
        let f = keys(5, &mut out, &mut scratch);
        let g = keys(7, &mut out, &mut scratch);
        assert!(!f.is_empty());
        assert_eq!(f.len(), g.len());
        // The same line, moved by the root (within the register's folding).
        assert!(f.iter().zip(&g).all(|(a, b)| (b - a).rem_euclid(12) == 2), "{f:?} {g:?}");
    }

    #[test]
    fn drums_fill_the_last_bar_of_each_phrase() {
        let key = MusicalKey::new(0, true);
        let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
        let mut bar = |phrase: u32, b: u64| {
            let mut lane = PatternLane::drums(DrumLayout::standard(), 4, 4);
            lane.phrase_bars = phrase;
            lane.render(b, key, &mut out, &mut scratch);
            out.events.as_slice().to_vec()
        };
        // Bar 3 ends a 4-bar phrase but not a 16-bar one.
        assert_ne!(bar(4, 3), bar(16, 3));
        // Bar 2 ends neither.
        assert_eq!(bar(4, 2), bar(16, 2));
    }

    #[test]
    fn rendering_does_not_allocate() {
        for lane in [PatternLane::drums(DrumLayout::standard(), 11, 4), PatternLane::bass(11, 4)] {
            let key = MusicalKey::new(2, true);
            let mut out = RenderedBar::default();
            let mut scratch = Scratch::default();
            lane.render(0, key, &mut out, &mut scratch);
            let (_, allocs) = crate::test_support::count_allocs(|| {
                for b in 0..64 {
                    lane.render(b, key, &mut out, &mut scratch);
                }
            });
            assert_eq!(allocs, 0, "{:?}", lane.role());
        }
    }
}
