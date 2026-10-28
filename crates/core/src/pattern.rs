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

use crate::event::{flags, BarEvents, EventKind, LiveEvent};
use crate::sampler::MORPH_REGION;
use crate::arrange::{FormBuilder, FormSpec};
use crate::contrast;
use crate::form::{
    form_chord, letter_seed, prime_from, prime_seed, splice_from, Feel, Form, FormPos, Letter, Play, Segment, SeedClass,
    LETTER_OFFSETS, NOTE_JITTER_SHARE,
};
use crate::slots::{PatternBook, Slot, UserPattern, VelOp};
use crate::generator::bass::{self, BassGenerator, BassRules};
use crate::generator::blocks::{defaults_from_spec, patterned, NoteShape};
use crate::generator::drums::{self, DrumGenerator, DrumRules};
use crate::generator::{idiom, BarCtx, DrumLayout, LaneGenerator, MAX_GEN_PARAMS};
use crate::harmony::{HarmonyCtx, BARS_PER_PHRASE};
use crate::key::MusicalKey;
use crate::ops::{self, Op, OpChain, OpCtx, SpanNote, MAX_SPAN_BARS};
use crate::role::Role;
use crate::chain::speed;
use crate::{rng, STEPS_PER_BAR, TICKS_PER_BAR};

/// Lane index the generators' seeds are hashed with. Mesh numbered its lanes
/// (0 = drums, 1 = bass); keeping the numbers keeps the seeds' sound.
pub const fn lane_index(role: Role) -> u8 {
    match role {
        Role::Drums => 0,
        _ => 1,
    }
}

/// Steps of the Splice relation on either side a follower's form is built
/// for (D48), so a slider move does not build one on every block.
const RELATION_BANDS: i8 = 4;


// MUSICAL: from Mesh live (user: the seed and the form set the bass speed per
// block). Speed steps (exponent on the centre speed) and their weights: main
// letters stay home more, fillers speed up more.
const BLOCK_RATE_MAIN: [(i8, f32); 3] = [(0, 0.65), (1, 0.25), (-1, 0.1)];
const BLOCK_RATE_FILLER: [(i8, f32); 3] = [(0, 0.35), (1, 0.45), (-1, 0.2)];
const S_BLOCK_RATE: u32 = 0xF51;
/// RNG stream of the sound seed a filler letter derives.
const S_SOUND: u32 = 0xF52;
const S_PERMUTE: u32 = 0xF53;

/// Parameters the Splice focus moves per step, the rest stay (Variation
/// plays as set).
const FOCUS_PARAMS: [&str; 2] = ["density", "spread"];
const S_SPLICE: u32 = 0xF54;

/// How far the focus moves each generator parameter (negative: only down,
/// off the focus): none while Splice is off.
fn focus_shift(names: &[&str], focus: &[f32; STEPS_PER_BAR]) -> [f32; MAX_GEN_PARAMS] {
    let mut out = [0.0; MAX_GEN_PARAMS];
    if focus.iter().all(|f| *f == 1.0) {
        return out;
    }
    for (i, name) in names.iter().enumerate().take(MAX_GEN_PARAMS) {
        if *name == "density" {
            out[i] = -crate::splice::PARAM_SHIFT;
        } else if FOCUS_PARAMS.contains(name) {
            out[i] = crate::splice::PARAM_SHIFT;
        }
    }
    out
}

/// Splice: hits outside the focus move into it or thin out (seeded per
/// hit). A hit on a step of low focus moves a 16th or an 8th to a step of
/// more focus where it does not land on another of its kind; where it can't
/// it plays softer (up to [`crate::splice::SOFTEN_MAX`]) and, past
/// [`crate::splice::DROP_FROM`] of the knob's way out, drops, up to
/// [`crate::splice::DROP_MAX`] of them where the focus is
/// lowest. A dropped note's time is held by the note before it at chance
/// `hold` (a pitched lane's Density near the top: the line keeps sounding,
/// its onsets leave the host's), unless that one is a stab. The bar stays
/// sorted, ties and gates move with their notes.
fn splice_thin(out: &mut BarEvents, focus: &[f32; STEPS_PER_BAR], seed: u32, lane: u8, hold: f32) {
    if focus.iter().all(|f| *f >= 1.0) {
        return;
    }
    // How far the knob is out of its centre (the focus's widest move).
    let reach = focus.iter().map(|f| (f - 1.0).abs()).fold(0.0, f32::max) / crate::splice::FOCUS_RANGE;
    let step = TICKS_PER_BAR / STEPS_PER_BAR as u16;
    let mut taken = [0u64; STEPS_PER_BAR];
    for e in out.as_slice().iter().filter(|e| matches!(e.kind, EventKind::Trigger | EventKind::NoteOn)) {
        taken[((e.tick / step) as usize).min(STEPS_PER_BAR - 1)] |= 1 << (e.target % 64);
    }
    for e in out.as_mut_slice() {
        if !matches!(e.kind, EventKind::Trigger | EventKind::NoteOn) {
            continue;
        }
        let s = ((e.tick / step) as usize).min(STEPS_PER_BAR - 1);
        let low = ((1.0 - focus[s]) / crate::splice::FOCUS_RANGE).clamp(0.0, 1.0);
        let h = rng::hash(seed, lane, 0, (e.tick / 4) as u8 ^ e.target, S_SPLICE);
        if low <= 0.0 {
            continue;
        }
        let try_move = rng::unit_f32(h) < low * crate::splice::MOVE_MAX;
        let bit = 1u64 << (e.target % 64);
        let better = [1i32, -1, 2, -2]
            .into_iter()
            .map(|d| s as i32 + d)
            .filter(|&t| (0..STEPS_PER_BAR as i32).contains(&t))
            .map(|t| t as usize)
            .filter(|&t| try_move && focus[t] > focus[s] && taken[t] & bit == 0)
            .max_by(|a, b| focus[*a].total_cmp(&focus[*b]));
        match better {
            Some(t) => {
                taken[s] &= !bit;
                taken[t] |= bit;
                let off = e.tick % step;
                e.tick = t as u16 * step + off;
                e.gate = e.gate.min(TICKS_PER_BAR - e.tick);
            }
            // No place to go: with the knob far out it drops (velocity 0,
            // removed below), else it plays softer.
            None => {
                let far = ((reach - crate::splice::DROP_FROM) / (1.0 - crate::splice::DROP_FROM)).clamp(0.0, 1.0);
                if rng::unit_f32(h.rotate_left(17)) < low * far * crate::splice::DROP_MAX {
                    e.vel = 0;
                } else {
                    e.vel = ((e.vel as f32 * (1.0 - crate::splice::SOFTEN_MAX * low)).round() as u8).max(1);
                }
            }
        }
    }
    if hold > 0.0 {
        let ev = out.as_mut_slice();
        let mut prev: Option<usize> = None;
        for i in 0..ev.len() {
            if ev[i].kind != EventKind::NoteOn {
                continue;
            }
            if ev[i].vel > 0 {
                prev = Some(i);
                continue;
            }
            let h = rng::hash(seed, lane, 0, (ev[i].tick / 4) as u8 ^ ev[i].target, S_SPLICE + 1);
            let Some(p) = prev.filter(|&p| ev[p].flags & flags::STAB == 0 && rng::unit_f32(h) < hold) else { continue };
            // Up to the next note that plays (no slur into it).
            let next = ev[i + 1..].iter().filter(|e| e.kind == EventKind::NoteOn && e.vel > 0).map(|e| e.tick).next().unwrap_or(TICKS_PER_BAR);
            let end = (ev[i].tick + ev[i].gate).min(next);
            ev[p].gate = ev[p].gate.max(end.saturating_sub(ev[p].tick));
        }
    }
    out.retain(|_, e| !(matches!(e.kind, EventKind::Trigger | EventKind::NoteOn) && e.vel == 0));
    out.sort();
}

/// Chunks a bar is cut into for Permute: half beats.
pub const PERMUTE_CHUNKS: usize = 8;

/// Rearrange the bar (the Permute knob): the bar is cut into
/// [`PERMUTE_CHUNKS`] chunks and seeded swaps move them, the first
/// `amount × chunks` of a fixed seeded swap list, so turning the knob up only
/// adds moves. The first chunk (the downbeat) stays. Events keep their place
/// inside their chunk; the bar stays sorted.
fn permute(out: &mut BarEvents, amount: f32, seed: u32, lane: u8) {
    let swaps = (amount.clamp(0.0, 1.0) * PERMUTE_CHUNKS as f32).round() as usize;
    if swaps == 0 {
        return;
    }
    // Where each chunk's content plays.
    let mut dest: [usize; PERMUTE_CHUNKS] = std::array::from_fn(|i| i);
    // The same swaps in every bar of the letter (snapshotted, D57).
    let h = |k: u8, s: u32| rng::hash(seed, lane, 0, k, s);
    for k in 0..swaps {
        let a = 1 + rng::below(h(k as u8, S_PERMUTE), (PERMUTE_CHUNKS - 1) as u32) as usize;
        let mut b = 1 + rng::below(h(k as u8, S_PERMUTE + 1), (PERMUTE_CHUNKS - 2) as u32) as usize;
        if b >= a {
            b += 1;
        }
        dest.swap(a, b);
    }
    let chunk = TICKS_PER_BAR / PERMUTE_CHUNKS as u16;
    for e in out.as_mut_slice() {
        let c = (e.tick / chunk) as usize % PERMUTE_CHUNKS;
        e.tick = dest[c] as u16 * chunk + e.tick % chunk;
    }
    out.sort();
}

/// One rendered bar.
#[derive(Clone, Default)]
pub struct RenderedBar {
    pub events: BarEvents,
    /// Drums: the voices the seed has on (bit per [`DrumVoice`](crate::generator::layout::DrumVoice)).
    pub voices: u8,
    /// Letter index 0..=3 (A..D) of the bar's first part.
    pub letter: u8,
    /// The parameters the bar's first part played with.
    pub params: BarParams,
}

// MUSICAL: first draft, tune by ear (user: very slight).
/// Largest Morph move of a bass note, in knob values (at full Variation).
pub const MORPH_NOTE: f32 = 0.04;
// MUSICAL: tune by ear (user, 2026-10-21: A and B vary in Morph by the
// Variation knob). Largest Morph move of a letter off A's at Variation 1,
// in knob values (wrapping, times the stream region), and its curve.
const LETTER_MORPH: f32 = 0.6;
const LETTER_MORPH_CURVE: f32 = 1.5;
const S_LETTER_MORPH: u32 = 0xF56;

/// Letter `letter`'s Morph move by Variation (`variation` 0..1): none for
/// A, B, C and D each a seeded way off it, up to [`LETTER_MORPH`].
fn letter_morph(pattern_seed: u32, lane: u8, letter: Letter, variation: f32) -> f32 {
    if letter == Letter::A {
        return 0.0;
    }
    let u = rng::unit_f32(rng::hash(letter_seed(pattern_seed, lane, letter), lane, 0, 0, S_LETTER_MORPH));
    // At least a third of the way, so a letter never lands next to A's.
    let way = (2.0 * u - 1.0).signum() * (1.0 / 3.0 + 2.0 / 3.0 * (2.0 * u - 1.0).abs());
    LETTER_MORPH * variation.clamp(0.0, 1.0).powf(LETTER_MORPH_CURVE) * way
}

/// Seed index of the Morph move, past the generator parameters.
const MORPH_INDEX: usize = MAX_GEN_PARAMS;
/// Seed index of a note's stereo side.
const PAN_INDEX: usize = MAX_GEN_PARAMS + 1;
// MUSICAL: first draft, tune by ear.
/// Pan of a note this far above the home root at full Spread, and the
/// semitones over which it gets there.
const WIDTH_PAN: f32 = 0.6;
const WIDTH_FULL_SEMIS: f32 = 19.0;

/// The generator parameters a bar played with, after its letter and filler
/// moves (the editor shows them per hit).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BarParams {
    /// Indexed like [`LaneGenerator::param_names`].
    pub values: [f32; MAX_GEN_PARAMS],
    jitter: [f32; MAX_GEN_PARAMS],
    seed: u32,
    lane: u8,
    /// Pitch class of the key root.
    root: u8,
    /// How far Variation lets letters and notes move the parameters.
    reach: f32,
    /// Drums: the swing every off 16th plays (the knob's and the letter's).
    pub swing: f32,
    /// Bass: the block's Morph move, in knob values.
    pub morph: f32,
    /// The Permute the bar played with.
    pub permute: f32,
}

impl BarParams {
    /// Parameter `index` as a generator reading it per note read it for a
    /// note of `shape`.
    pub fn at(&self, index: usize, shape: NoteShape) -> f32 {
        let (Some(v), Some(amp)) = (self.values.get(index), self.jitter.get(index)) else { return 0.0 };
        patterned(*v, *amp, self.seed, self.lane, index, shape)
    }

    /// The shape of a played note `ev` (its step, gate and height).
    pub fn shape_of(&self, ev: &LiveEvent) -> NoteShape {
        NoteShape::note(ev.step, ev.gate, ev.key as i32, self.root)
    }

    /// The Morph move of bass note `ev`, in knob values (see [`MORPH_NOTE`]).
    pub fn morph_move(&self, ev: &LiveEvent) -> f32 {
        patterned(0.5, MORPH_NOTE * self.reach, self.seed, self.lane, MORPH_INDEX, self.shape_of(ev)) - 0.5
    }
}

/// A lane's render inputs.
pub struct PatternLane {
    role: Role,
    generator: Box<dyn LaneGenerator>,
    layout: DrumLayout,
    /// The lane's form: one bar of A, a form it was given
    /// ([`Self::set_form`]) or one built from its seeds
    /// ([`Self::set_form_spec`], D48).
    form: Form,
    /// What `form` was built from.
    spec: Option<FormSpec>,
    builder: FormBuilder,
    /// Splice: the host's form, built from the host's seeds, and the form
    /// a follower plays, derived from it and its own by the relation
    /// (D48); what they were built from.
    host_form: Form,
    host_spec: Option<FormSpec>,
    derived: Form,
    derived_from: Option<(FormSpec, Option<FormSpec>, i8)>,
    /// Seed of the groove (letters, rhythm, pitch).
    pub pattern_seed: u32,
    /// Seed of the sample picks.
    pub sound_seed: u32,
    /// Rate setting on top of the seeded block speed (bass): −1 half, 1
    /// double (D57: Normal plays the generator's own pace).
    pub rate: i8,
    /// Mode in the key's family (0 = plain minor or major).
    pub mode: u8,
    /// Bars of a phrase: the drums fill its last bar (D25).
    pub phrase_bars: u32,
    /// Permute 0..1: how many of the bar's chunks swap places (seeded).
    pub permute: f32,
    /// Contrast 0..1: how far B moves from A, and C and D from them (D49).
    pub contrast: f32,
    /// Splice: the focus per step the player sets for the next render
    /// ([`crate::splice::focus`]; all 1 = off).
    pub focus: [f32; STEPS_PER_BAR],
    /// Splice: the pitched host's line under the next render (D46).
    pub host_line: crate::splice::HostLine,
    /// Splice: the host's form position for the next render, so the blocks
    /// change together (D46); `None` plays the lane's own.
    pub follow: Option<FormFollow>,
    /// Generator parameters, indexed like [`LaneGenerator::param_names`].
    pub params: [f32; MAX_GEN_PARAMS],
    /// The patterns the user edited (D60), and a count of the books set
    /// (the span cache keys on it).
    patterns: std::sync::Arc<PatternBook>,
    patterns_version: u64,
    /// A DAW note's pattern (D60): its slot and operator, and the form of
    /// it the lane plays instead of its own while it is set.
    ext: Option<(Slot, VelOp)>,
    ext_form: Form,
    /// Hold a new form until the next 16-bar section starts (D61, the
    /// player sets it): the spec waiting and the form it built.
    pub defer_forms: bool,
    pending: Option<FormSpec>,
    next_form: Form,
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
        Self::bass_with_clips(pattern_seed, sound_seed, crate::clips::ClipSet::builtin())
    }

    /// A bass lane with these clips for clip mode (D50; the `source`
    /// parameter picks it, the generated line by default).
    pub fn bass_with_clips(pattern_seed: u32, sound_seed: u32, clips: std::sync::Arc<crate::clips::ClipSet>) -> Self {
        let none = BTreeMap::new();
        let defaults = defaults_from_spec(&bass::PARAM_NAMES, &bass::PARAM_DEFAULTS, &none);
        let generator = BassGenerator::new(defaults, BassRules::from_spec(&none)).with_clips(clips);
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
        Self {
            role,
            generator,
            layout,
            form: Form::generated(),
            spec: None,
            builder: FormBuilder::new(),
            host_form: Form::generated(),
            host_spec: None,
            derived: Form::generated(),
            derived_from: None,
            pattern_seed,
            sound_seed,
            rate: 0,
            mode: 0,
            phrase_bars: 8,
            permute: 0.0,
            contrast: contrast::CONTRAST_DEFAULT,
            focus: [1.0; STEPS_PER_BAR],
            host_line: crate::splice::HostLine::default(),
            follow: None,
            params,
            patterns: std::sync::Arc::new(PatternBook::default()),
            patterns_version: 0,
            ext: None,
            ext_form: Form::generated(),
            defer_forms: false,
            pending: None,
            next_form: Form::generated(),
        }
    }

    /// The user's patterns (D60). Returns the book it played before, for
    /// the caller to drop off the audio thread. RT-safe.
    pub fn set_patterns(&mut self, book: std::sync::Arc<PatternBook>) -> std::sync::Arc<PatternBook> {
        self.patterns_version = self.patterns_version.wrapping_add(1);
        std::mem::replace(&mut self.patterns, book)
    }

    pub fn patterns(&self) -> &PatternBook {
        &self.patterns
    }

    /// The pattern the user edited for `pos`'s slot, if any.
    fn user_of(&self, pos: &FormPos) -> Option<&UserPattern> {
        self.patterns.get(Slot { letter: pos.letter, variant: pos.variant })
    }

    /// The seed of `pos`'s pattern: its letter's, or its second's (D60).
    fn seed_of(&self, lane: u8, pos: &FormPos) -> u32 {
        self.patterns.seed_of(Slot { letter: pos.letter, variant: pos.variant }, letter_seed(self.pattern_seed, lane, pos.letter), lane)
    }

    /// Play slot `slot`'s pattern with `op` from bar 0, looping, instead of
    /// the form (a DAW note, D60); `None` goes back to the form. The pattern
    /// loops its seeded phrase, or the window of the user's pattern. RT-safe.
    pub fn play_slot(&mut self, play: Option<(Slot, VelOp)>) {
        self.ext = play;
        let Some((slot, op)) = play else { return };
        let seg = self.slot_segment(slot, op);
        self.ext_form.set_one(seg);
    }

    /// The DAW note playing, if any.
    pub fn playing_slot(&self) -> Option<(Slot, VelOp)> {
        self.ext
    }

    /// The segment slot `slot` plays as (one loop of its phrase) with `op`.
    pub fn slot_segment(&self, slot: Slot, op: VelOp) -> Segment {
        let (phrase, offset) = self.slot_phrase(slot);
        let mut seg = Segment { variant: slot.variant, phrase, offset, ..Segment::new(slot.letter, phrase as u16, 0) };
        match op {
            VelOp::Plain => {}
            VelOp::Prime => seg.prime = true,
            VelOp::Op(o) => {
                seg.ops.push(o);
            }
        }
        seg
    }

    /// Slot `slot`'s phrase in beats and its offset: the user's window, else
    /// the phrase its letter plays in the lane's form (one bar without one).
    pub fn slot_phrase(&self, slot: Slot) -> (u8, u8) {
        if let Some(u) = self.patterns.get(slot) {
            return (u.window_beats().clamp(1, u8::MAX as u32) as u8, 0);
        }
        let Some(spec) = self.spec else { return (crate::form::BEATS_PER_BAR as u8, 0) };
        let p = crate::arrange::letter_phrase(&spec, slot.letter, slot.variant);
        (p.beats.max(1), p.offset)
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
        &self.form
    }

    /// What the lane's form was built from (`None`: one bar of A or a form
    /// it was given).
    pub fn form_spec(&self) -> Option<FormSpec> {
        self.spec
    }

    /// The lane index its form positions are varied for.
    pub fn form_lane(&self) -> u8 {
        lane_index(self.role)
    }

    /// Play `form` (not RT-safe).
    pub fn set_form(&mut self, form: Form) {
        self.form = form;
        self.spec = None;
    }

    /// Play the form `spec` builds (D48), unless it plays it already.
    /// RT-safe.
    pub fn set_form_spec(&mut self, spec: FormSpec) {
        if self.spec == Some(spec) {
            self.pending = None;
        } else if !self.defer_forms || self.spec.is_none() {
            self.builder.build(&spec, &mut self.form);
            self.spec = Some(spec);
            self.pending = None;
        } else if self.pending != Some(spec) {
            self.builder.build(&spec, &mut self.next_form);
            self.pending = Some(spec);
        }
    }

    /// Play the form held back ([`Self::defer_forms`]) when pattern bar
    /// `bar` starts a 16-bar section, or `now`; whether it did. RT-safe.
    pub fn advance_form(&mut self, bar: u64, now: bool) -> bool {
        if self.pending.is_none() || !(now || bar % 16 == 0) {
            return false;
        }
        std::mem::swap(&mut self.form, &mut self.next_form);
        self.spec = self.pending.take();
        true
    }

    /// Play one bar of A forever (a style without forms). RT-safe.
    pub fn clear_form(&mut self) {
        self.pending = None;
        if self.spec.is_some() {
            self.form.set_single();
            self.spec = None;
        }
    }

    /// Splice: follow a host's form at its position (`None`: the lane's
    /// own form). The follower plays a form of its own derived from the
    /// host's by the relation (D48), built again when the host's seeds,
    /// the lane's own or the relation's band change. RT-safe.
    pub fn set_follow(&mut self, follow: Option<FormFollow>) {
        if let Some(f) = follow {
            let band = (f.relation as f32 / 100.0 * RELATION_BANDS as f32).round() as i8;
            let key = (f.spec, self.spec, band);
            if self.derived_from != Some(key) {
                if self.host_spec != Some(f.spec) {
                    self.builder.build(&f.spec, &mut self.host_form);
                    self.host_spec = Some(f.spec);
                }
                let seed = self.spec.map_or(self.pattern_seed, |s| s.seeds.fill);
                let relation = band as f32 / RELATION_BANDS as f32;
                self.builder.follow(&self.host_form, &self.form, relation, seed, self.role == Role::Drums, &mut self.derived);
                self.derived_from = Some(key);
            }
        }
        self.follow = follow;
    }

    /// Render `bar` (0 = the first bar of the song, or of the note in
    /// retrigger mode) in `key` (the held note's pitch class and the scale
    /// family) into `out`.
    pub fn render(&self, bar: u64, key: MusicalKey, out: &mut RenderedBar, scratch: &mut Scratch) {
        let lane = lane_index(self.role);
        // Splice: the host's form position, so the blocks change with its.
        let (parts, n_parts) = match self.follow {
            _ if self.ext.is_some() => self.ext_form.parts(bar, self.pattern_seed, lane),
            Some(f) if self.derived_from.is_some_and(|d| d.0 == f.spec) => self.derived.parts(f.bar, f.spec.seeds.fill, lane),
            _ => self.form.parts(bar, self.pattern_seed, lane),
        };
        let pos = parts[0].pos;
        let seed = self.seed_of(lane, &pos);
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
        out.params = BarParams::default();
        let form_seed = self.follow.map_or(self.pattern_seed, |f| f.spec.seeds.fill);
        out.voices = self.render_part(&ctx, &pos, 0, form_seed, &mut out.events, &mut scratch.work, &mut scratch.span, Some(&mut out.params));
        out.letter = pos.letter.index();
        for p in &parts[1..n_parts] {
            scratch.part.clear();
            self.render_part(&ctx, &p.pos, p.from, form_seed, &mut scratch.part, &mut scratch.work, &mut scratch.span, None);
            splice_range(&mut out.events, &scratch.part, p.from, p.to, &mut scratch.work.a);
        }
    }

    /// One part of a bar through the render chain (D57, [`crate::chain`]):
    /// R0 here, R1 to R8 in [`Self::front`] (per bar of the span when the
    /// part plays operators, R9 in [`Self::render_span`]), then R10 to R12.
    #[allow(clippy::too_many_arguments)]
    fn render_part(
        &self,
        c: &PartCtx,
        pos: &FormPos,
        from: u16,
        form_seed: u32,
        out: &mut BarEvents,
        work: &mut Work,
        span: &mut [SpanCache; SPAN_SLOTS],
        played: Option<&mut BarParams>,
    ) -> u8 {
        // A plugin instance has no drum lane to break with: break-only
        // segments rest like rests.
        if matches!(pos.play, Play::Rest | Play::Breaks) {
            out.clear();
            return 0;
        }
        // R0 Resolve.
        let r = self.resolve(c, pos);
        let bar_ctx = self.bar_ctx(c, pos, &r);
        let names = self.generator.param_names();
        let (seed, class, reach, params, block_morph, knob_swing, phrase_end, front) = (r.seed, r.class, r.reach, r.params, r.block_morph, r.knob_swing, r.phrase_end, r.front);
        let voices = self.generator.voice_mask(&bar_ctx);
        let chain = self.resolve_ops(pos, form_seed, c.lane);
        // A phrase loops from its own start (D48); the drums' phrase-end
        // bar plays the letter's fill on the song's bars.
        let plain = if pos.phrase > 0 { self.role == Role::Drums && phrase_end } else { chain.is_empty() };
        if plain {
            self.front(&bar_ctx, &front, c.bar, pos.prime, out, work);
        } else {
            // R9 Operators (D47): the letter's first bars, transformed over
            // the segment, this bar's part of it.
            self.render_span(c, pos, &bar_ctx, &front, &chain, from, out, work, span);
        }
        if let Some(p) = played {
            *p = BarParams {
                values: params,
                jitter: bar_ctx.jitter,
                seed,
                lane: c.lane,
                root: c.harmony.key.root,
                reach,
                swing: if self.role == Role::Drums { knob_swing } else { (Feel::of(seed, c.lane, class).swing + knob_swing).min(1.0) },
                morph: if self.role == Role::Drums { 0.0 } else { block_morph },
                permute: front.permute,
            };
        }
        // R10 Splice.
        if self.role != Role::Drums && self.host_line.on {
            // The bar as it plays fitted to the host's line (D46, D47), then
            // the lengths alike or against it. The bar's first note is the
            // anchor (music theory §3.3); figures and slid notes keep theirs.
            fit_to_host(out, &self.host_line, &c.harmony);
            let ev = out.as_mut_slice();
            let mut first = true;
            for i in 0..ev.len() {
                if ev[i].kind != EventKind::NoteOn {
                    continue;
                }
                if std::mem::take(&mut first) {
                    continue;
                }
                let next = ev[i + 1..].iter().find(|e| e.kind == EventKind::NoteOn);
                if ev[i].has(flags::FIGURE) || next.is_some_and(|n| n.has(flags::TIE)) {
                    continue;
                }
                let room = next.map_or(TICKS_PER_BAR, |n| n.tick).saturating_sub(ev[i].tick);
                let s = (ev[i].tick / crate::TICKS_PER_STEP) as usize % STEPS_PER_BAR;
                ev[i].gate = crate::splice::contrast_gate(&self.host_line, s, ev[i].gate, room);
            }
        }
        let hold = match names.iter().position(|n| *n == "density") {
            Some(i) if self.role == Role::Bass => crate::chain::fill::amount(params[i]).powi(2),
            _ => 0.0,
        };
        splice_thin(out, &self.focus, seed, c.lane, hold);
        // R11 Expression.
        if self.role != Role::Drums {
            self.generator.expression(&bar_ctx, out);
            // Each note moves Morph a touch, as its shape says.
            let played = BarParams { seed, lane: c.lane, root: c.harmony.key.root, reach, ..BarParams::default() };
            // Spread is width too: notes above the home root spread out in
            // the stereo field, the sub stays centred.
            let spread = names.iter().position(|n| *n == "spread").map_or(0.0, |i| params[i]);
            let home = bass::home_root(c.harmony.key);
            for e in out.as_mut_slice().iter_mut().filter(|e| e.kind == EventKind::NoteOn) {
                let at = e.start as f64 / u16::MAX as f64 + (block_morph + played.morph_move(e)) as f64 * MORPH_REGION;
                e.start = (at.rem_euclid(1.0) * u16::MAX as f64) as u16;
                let above = ((e.key as i32 - home) as f32 / WIDTH_FULL_SEMIS).clamp(0.0, 1.0);
                let side = patterned(0.5, 0.5, seed, c.lane, PAN_INDEX, played.shape_of(e)) * 2.0 - 1.0;
                e.pan = (side.signum() * WIDTH_PAN * spread * above * 127.0).round() as i8;
            }
            // R12 Cleanup.
            crate::chain::cleanup(out);
        } else {
            // D68: Groove spreads the hits' velocities, as the last step.
            crate::chain::groove::spread(out, knob_swing);
            out.sort();
        }
        voices
    }

    /// R1 to R8 (D57) of one bar (`bar` the bar it renders for, the speed's
    /// part of the skeleton) into `out`.
    fn front(&self, ctx: &BarCtx, f: &Front, bar: u64, prime: bool, out: &mut BarEvents, work: &mut Work) {
        let pitched = self.role != Role::Drums;
        // R1 Skeleton.
        out.clear();
        self.generator.render_bar(ctx, out);
        out.sort();
        // R2 Structure: A′ (the same seed, a different second half), Permute.
        if prime {
            let pctx = BarCtx { seed: prime_seed(ctx.seed, ctx.lane), ..*ctx };
            work.a.clear();
            self.generator.render_bar(&pctx, &mut work.a);
            work.a.sort();
            splice_from(out, &work.a, prime_from(self.op_amount()));
        }
        if f.source {
            // R8 only: the pitches of the skeleton as written.
            self.generator.melody(ctx, out);
            out.sort();
            return;
        }
        permute(out, f.permute, ctx.seed, ctx.lane);
        // R3 Speed.
        if pitched {
            speed::apply_speed(out, f.exponent, bar, &mut work.a);
            speed::limit_speed(out, self.rate, f.exponent);
        }
        // R4 Recursion: a note's own pattern is the skeleton again with its
        // seed (R1 and R3), once.
        if pitched {
            let (gen, rate, exponent) = (&self.generator, self.rate, f.exponent);
            let (a, b, child) = (&mut work.a, &mut work.b, &mut work.c);
            let mut render = |seed: u32, buf: &mut BarEvents| {
                let cctx = BarCtx { seed, ..*ctx };
                buf.clear();
                gen.render_bar(&cctx, buf);
                buf.sort();
                speed::apply_speed(buf, exponent, bar, child);
                speed::limit_speed(buf, rate, exponent);
            };
            crate::chain::recursion::pitched(out, f.groove, f.variation, ctx.seed, ctx.lane, &mut render, a, b);
        } else {
            crate::chain::recursion::drums(out, f.groove, ctx.seed, ctx.lane, &mut work.a);
        }
        // R5 Density fill, R6 Length.
        let finest = speed::grid(self.rate, f.exponent);
        self.generator.fill(ctx, out, finest);
        self.generator.lengths(ctx, out, finest);
        // R7 Groove: the letter's feel (its swing and drift; the triplet
        // reflow on the drums only, D57), then the knob's.
        let mut feel = Feel::of(ctx.seed, ctx.lane, ctx.class);
        if pitched {
            feel.triplet = false;
            feel.apply(out, ctx.seed, ctx.lane);
            crate::chain::groove::pitched(out, f.groove, f.variation, ctx.seed, ctx.lane);
        } else {
            // The drums swing as the knob says (no letter's own swing).
            feel.swing = f.swing;
            feel.apply(out, ctx.seed, ctx.lane);
            crate::chain::groove::drums(out, f.swing);
        }
        // R8 Melody.
        self.generator.melody(ctx, out);
        out.sort();
    }
}

/// What [`PatternLane::front`] reads besides the bar's context: the speed
/// exponent, Permute, the Groove knob (the recursion's and the groove's),
/// Variation, the drums' Swing knob.
#[derive(Debug, Clone, Copy)]
struct Front {
    exponent: i8,
    permute: f32,
    groove: f32,
    variation: f32,
    swing: f32,
    /// The pattern's source only (D60): R1, A′ and R8, none of the knobs'
    /// phases (speed, recursion, Density, Length, Groove).
    source: bool,
}

// The editor's source of a generated pattern (D60): its first bars.
const SOURCE_BARS: u32 = 4;

/// No drum lane's onsets (the drums and the bass render without one).
const NO_MASK: [u8; STEPS_PER_BAR] = [0; STEPS_PER_BAR];

/// R0 Resolve (D57) of one part: its seeds, the user's pattern and its own
/// knob values.
struct Resolved<'a> {
    seed: u32,
    sound_seed: u32,
    user: Option<&'a UserPattern>,
    class: SeedClass,
    params: [f32; MAX_GEN_PARAMS],
    jitter: [f32; MAX_GEN_PARAMS],
    /// How far Variation lets letters and notes move the parameters.
    reach: f32,
    /// Bass: the block's Morph move, in knob values.
    block_morph: f32,
    /// Drums: the Swing knob, taken out of the parameters.
    knob_swing: f32,
    /// The bar ends a phrase (the drums fill it, D25); the drums fill it.
    phrase_end: bool,
    fill: bool,
    front: Front,
}

impl PatternLane {
    /// R0 Resolve: part `pos` of bar `c`, its seeds and knob values.
    fn resolve(&self, c: &PartCtx, pos: &FormPos) -> Resolved<'_> {
        let seed = self.seed_of(c.lane, pos);
        let class = pos.letter.class();
        let names = self.generator.param_names();
        // Variation widens how far notes move the parameters; Contrast how
        // far B moves from A, and C and D from them (D49).
        let reach = variation_reach(names, &self.params);
        let mut params = contrast::letter_params(names, &self.params, self.pattern_seed, c.lane, pos.letter, pos.second, self.contrast);
        // The drums' B plays another style (user, 2026-10-26).
        if let Some(i) = names.iter().position(|n| *n == "style") {
            params[i] = contrast::letter_style(params[i], self.pattern_seed, c.lane, pos.letter);
        }
        // The block's own Permute and (bass) Morph. A Permute at 0 stays off.
        let permute = contrast::knob_value("permute", self.permute, self.pattern_seed, c.lane, pos.letter, pos.second, self.contrast);
        let variation = names.iter().position(|n| *n == "variation").map_or(0.0, |i| params[i]);
        let block_morph = contrast::morph_move(self.pattern_seed, c.lane, pos.letter, pos.second, self.contrast) + letter_morph(self.pattern_seed, c.lane, pos.letter, variation);
        // Drums: the Swing knob swings every off 16th of the bar (breaks,
        // ghosts, percussion and tops too), on top of the letter's feel, so
        // the generator itself plays straight.
        let knob_swing = match names.iter().position(|n| *n == "swing") {
            // The knob's own value: no letter moves the drums' Groove (user,
            // 2026-10-27: it moved with the letters).
            Some(i) if self.role == Role::Drums => {
                params[i] = 0.0;
                self.params[i]
            }
            _ => 0.0,
        };
        // The drum lane fills the last bar of a filler segment, and the last
        // bar of every phrase (D25).
        let phrase_end = self.phrase_bars > 1 && (c.bar + 1) % self.phrase_bars as u64 == 0;
        let fill = self.role == Role::Drums && ((class == SeedClass::Filler && pos.last_of_segment) || phrase_end);
        let sound_seed = match class {
            SeedClass::Main => self.sound_seed,
            SeedClass::Filler => rng::hash(self.sound_seed, c.lane, pos.letter.index(), 0, S_SOUND) as u32,
        };
        let mut jitter = note_jitter(names, reach);
        if let Some(i) = names.iter().position(|n| *n == "length") {
            jitter[i] = shape_note(names, &params, self.pattern_seed, c.lane);
        }
        let exponent = self.rate + self.letter_rate(c.lane, pos.letter);
        let groove = self.groove_of(&params, knob_swing);
        let front = Front { exponent, permute, groove, variation, swing: knob_swing, source: false };
        Resolved { seed, sound_seed, user: self.user_of(pos), class, params, jitter, reach, block_morph, knob_swing, phrase_end, fill, front }
    }

    /// The generator's context of part `pos` of bar `c`, resolved as `r`.
    fn bar_ctx<'a>(&'a self, c: &PartCtx, pos: &FormPos, r: &'a Resolved<'a>) -> BarCtx<'a> {
        let names = self.generator.param_names();
        BarCtx {
            style_seed: self.pattern_seed,
            seed: r.seed,
            sound_seed: r.sound_seed,
            class: r.class,
            lane: c.lane,
            role: self.role,
            bar_in_phrase: c.bar_in_phrase,
            phrase: c.phrase,
            fill: r.fill,
            half_time: false,
            params: &r.params,
            harmony: c.harmony,
            drum_mask: &NO_MASK,
            jitter: r.jitter,
            bass_onsets: 0,
            bass_phrase: [0; 4],
            drum_fill: false,
            focus: self.focus,
            focus_shift: focus_shift(names, &self.focus),
            // The host's line as the bar will play after the speed (half
            // speed plays a part of it stretched).
            host: if self.role == Role::Drums { self.host_line } else { speed::host_in_render(&self.host_line, r.front.exponent, c.bar) },
            letter: pos.letter.index(),
            material_bar: (c.bar % (u16::MAX as u64 + 1)) as u16,
            user: r.user,
            contrast: self.contrast,
        }
    }

    /// What slot `slot` offers the editor (D60), in key `key`: the user's
    /// pattern, else the seeded one's whole source (its clip, or four bars
    /// of the generator's) with the window the seed plays of it. The
    /// source is the skeleton and its pitches (R1, R8), before the knobs'
    /// phases; bass rows are semitones over the home root. Not RT-safe.
    pub fn slot_source(&self, slot: Slot, key: MusicalKey) -> UserPattern {
        if let Some(u) = self.patterns.get(slot) {
            return u.clone();
        }
        let lane = lane_index(self.role);
        let pos = FormPos { letter: slot.letter, variant: slot.variant, ..FormPos::default() };
        let harmony = HarmonyCtx { key, chord_degree: 0, progression: 0, mode: self.mode };
        let part = |bar: u64| PartCtx { lane, bar, bar_in_phrase: (bar % BARS_PER_PHRASE) as u8, phrase: (bar / BARS_PER_PHRASE) as u32, harmony };
        let beat = TICKS_PER_BAR as u32 / crate::form::BEATS_PER_BAR as u32;
        let first = self.resolve(&part(0), &pos);
        let material = self.generator.material_beats(&self.bar_ctx(&part(0), &pos, &first));
        let bars = material.map_or(SOURCE_BARS, |b| b.div_ceil(crate::form::BEATS_PER_BAR as u32)).clamp(1, crate::slots::MAX_SOURCE_TICKS / TICKS_PER_BAR as u32);
        let mut notes = Vec::new();
        let mut work = Work::default();
        let mut out = BarEvents::default();
        for j in 0..bars as u64 {
            let c = part(j);
            let mut r = self.resolve(&c, &pos);
            r.fill = false;
            r.front.source = true;
            let ctx = self.bar_ctx(&c, &pos, &r);
            self.front(&ctx, &r.front, j, false, &mut out, &mut work);
            let home = self.generator.home_key(&ctx);
            for e in out.as_slice().iter().filter(|e| matches!(e.kind, EventKind::NoteOn | EventKind::Trigger) && e.vel > 0) {
                let row = if self.role == Role::Drums { e.target as i16 } else { (e.key as i32 - home) as i16 };
                notes.push(crate::slots::UserNote { tick: j as u32 * TICKS_PER_BAR as u32 + e.tick as u32, len: e.gate as u32, row, vel: e.vel, start: e.start, choke: e.choke });
            }
        }
        let ticks = bars * TICKS_PER_BAR as u32;
        let (phrase, offset) = self.slot_phrase(slot);
        let window = material.map_or(0, |b| clip_window(first.seed, lane, b));
        let len = (phrase as u32 * beat).clamp(crate::slots::MIN_WINDOW, ticks);
        let start = ((offset as u32 + window) * beat) % ticks;
        let mut p = UserPattern { notes, ticks, start: start.min(ticks - len), len };
        p.normalize();
        p
    }
    /// The operators segment `pos` plays (D47): the seeded `?` resolved by
    /// the form's seed (a follower's are its host's).
    fn resolve_ops(&self, pos: &FormPos, form_seed: u32, lane: u8) -> OpChain {
        let mut chain = OpChain::default();
        let place = ops::Place { period_end: pos.period_end, repeat: pos.repeat, before_change: pos.before_change };
        for (k, op) in pos.ops.iter().enumerate() {
            let op = if op == Op::Seeded {
                let h = rng::hash(form_seed, lane, pos.segment, k as u8, S_SEEDED_OP ^ pos.second as u32);
                ops::seeded(h, place)
            } else {
                op
            };
            chain.push(op);
        }
        // Contrast (D49): B's line may answer mirrored, or an octave up.
        // Against a pitched host the line's direction is Splice's (D46).
        if pos.letter == Letter::B && self.role != Role::Drums {
            let (mirror, lift) = contrast::answer_line(self.pattern_seed, lane, self.contrast);
            if mirror && !self.host_line.on && !chain.contains(Op::Inversion) {
                chain.push(Op::Inversion);
            }
            if lift && !chain.contains(Op::OctaveUp) && !chain.contains(Op::OctaveDown) {
                chain.push(Op::OctaveUp);
            }
        }
        // Splice (D47): a follower plays its host's operators; towards the
        // counter end its questions and answers swap roles with the host's.
        if let Some(f) = self.follow {
            let r = f.relation.max(0) as f32 / 100.0;
            if r > 0.0 && rng::chance(rng::hash(form_seed, lane, pos.segment, 0, S_SWAP_QA ^ pos.second as u32), r) {
                chain = chain.swap_question_answer();
            }
        }
        chain
    }

    /// What a DAW note asks for to play segment `pos` (D60): its written
    /// operator (the seeded `?` resolved) as a velocity band, or `′`.
    pub fn velocity_op(&self, pos: &FormPos) -> VelOp {
        if pos.prime {
            return VelOp::Prime;
        }
        let place = ops::Place { period_end: pos.period_end, repeat: pos.repeat, before_change: pos.before_change };
        pos.ops
            .iter()
            .enumerate()
            .map(|(k, op)| if op == Op::Seeded { ops::seeded(rng::hash(self.pattern_seed, self.form_lane(), pos.segment, k as u8, S_SEEDED_OP ^ pos.second as u32), place) } else { op })
            .map(VelOp::Op)
            .find(|v| v.velocity().is_some())
            .unwrap_or(VelOp::Plain)
    }

    /// The bass speed step `letter` plays: A's seeded, the others from it
    /// by Contrast (D49).
    fn letter_rate(&self, lane: u8, letter: Letter) -> i8 {
        let a = block_rate(letter_seed(self.pattern_seed, lane, Letter::A), lane, SeedClass::Main);
        contrast::letter_rate(a, self.pattern_seed, lane, letter, self.contrast)
    }

    /// The Groove a letter plays with (`params` its own values): the bass's
    /// syncopation, the drums' Swing knob (`swing`, taken out of them).
    fn groove_of(&self, params: &[f32; MAX_GEN_PARAMS], swing: f32) -> f32 {
        if self.role == Role::Drums {
            return swing.clamp(0.0, 1.0);
        }
        self.param_names().iter().position(|n| *n == "syncopation").map_or(0.0, |i| params[i]).clamp(0.0, 1.0)
    }

    /// How far the operators go: Variation 0 still a little (D47).
    fn op_amount(&self) -> f32 {
        let v = self.param_names().iter().position(|n| *n == "variation").map_or(0.5, |i| self.params[i]);
        OP_AMOUNT_FLOOR + (1.0 - OP_AMOUNT_FLOOR) * v.clamp(0.0, 1.0)
    }

    /// The part of the segment `pos` belongs to under this bar, from tick
    /// `from`: the letter's first bars (as the letter first plays them),
    /// transformed by `chain` over the whole segment, cached per span.
    #[allow(clippy::too_many_arguments)]
    fn render_span(&self, c: &PartCtx, pos: &FormPos, base: &BarCtx, front: &Front, chain: &OpChain, from: u16, out: &mut BarEvents, work: &mut Work, spans: &mut [SpanCache; SPAN_SLOTS]) {
        let bar_ticks = TICKS_PER_BAR as u32;
        let beat_ticks = bar_ticks / crate::form::BEATS_PER_BAR as u32;
        // A phrase (D48) is its own length wherever it plays; a segment
        // without one spans itself from its first bar.
        let phrased = pos.phrase > 0;
        // The user's pattern (D60) loops its own window.
        let span_beats = match base.user {
            Some(u) if phrased => u.window_beats(),
            _ if phrased => pos.phrase as u32,
            _ => pos.segment_beats as u32,
        };
        let ticks = (span_beats * beat_ticks).clamp(1, (MAX_SPAN_BARS as u32) * bar_ticks);
        let bars = ticks.div_ceil(bar_ticks) as u16;
        // Clip mode (D50): a phrase is a window of its letter's clip, which
        // renders whole (its material) first.
        let material = if phrased && self.role != Role::Drums { self.generator.material_beats(base) } else { None };
        let material_ticks = material.map(|b| (b * beat_ticks).clamp(1, MAX_SPAN_BARS as u32 * bar_ticks));
        let (render_bars, limit) = match material_ticks {
            Some(m) => (m.div_ceil(bar_ticks) as u16, m),
            None => (bars, ticks),
        };
        let first_bar = if phrased { 0 } else { c.bar.saturating_sub(pos.bar_in_segment as u64) };
        let segment = if phrased { 0 } else { pos.segment as u64 };
        // The form's second pass goes further (it escalates).
        let amount = (self.op_amount() + if pos.second { SECOND_PASS_OPS } else { 0.0 }).min(1.0);
        let mut key = mix(rng::hash(self.pattern_seed, c.lane, 0, 0, S_SPAN), base.seed as u64);
        for v in [
            self.sound_seed as u64,
            chain.fingerprint(),
            pos.prime as u64,
            ticks as u64,
            first_bar,
            segment | (pos.second as u64) << 8 | (pos.letter.index() as u64) << 16 | (pos.offset as u64) << 24 | (phrased as u64) << 32,
            base.harmony.key.root as u64 | (base.harmony.key.minor as u64) << 8 | (self.mode as u64) << 16,
            self.phrase_bars as u64,
            amount.to_bits() as u64,
            limit as u64,
            (front.exponent as u8 as u64) | (self.rate as u8 as u64) << 8,
            front.groove.to_bits() as u64 | (front.permute.to_bits() as u64) << 32,
            front.swing.to_bits() as u64 | (base.contrast.to_bits() as u64) << 32,
            self.patterns_version,
        ] {
            key = mix(key, v);
        }
        key = base.params.iter().fold(key, |h, p| mix(h, p.to_bits() as u64));
        // Against a pitched host the letter's bars are walked against the
        // host's line under this bar (D46), so the span renders per bar.
        if base.host.on {
            key = base.host.keys.iter().chain(&base.host.lens).fold(mix(key, base.host.relation.to_bits() as u64), |h, v| mix(h, *v as u64));
            key = mix(key, base.harmony.chord_degree as u64);
        }

        // The slot holding this span, else the one not used last.
        let slot = spans.iter().position(|s| s.valid && s.key == key).unwrap_or_else(|| (0..SPAN_SLOTS).min_by_key(|&i| spans[i].used).unwrap_or(0));
        let stamp = spans.iter().map(|s| s.used).max().unwrap_or(0) + 1;
        let span = &mut spans[slot];
        span.used = stamp;
        if !(span.valid && span.key == key) {
            span.notes.clear();
            for j in 0..render_bars {
                // The letter's bar `j` as its segment first plays it: its
                // chord, its fill, no Splice (that applies after).
                let reference = FormPos { bar_in_segment: j, bars_since_main: j, last_of_segment: j + 1 == bars, prime: false, ops: OpChain::default(), ..*pos };
                let mut harmony = base.harmony;
                if self.role != Role::Drums {
                    // A phrase is written on the tonic and follows the song's
                    // chords where it plays (below).
                    harmony.chord_degree = match (phrased, base.host.on) {
                        (true, true) => base.harmony.chord_degree,
                        (true, false) => 0,
                        _ => form_chord(self.pattern_seed, c.lane, &reference, base.seed, harmony.key.minor),
                    };
                }
                let phrase_end = !phrased && self.phrase_bars > 1 && (first_bar + j as u64 + 1) % self.phrase_bars as u64 == 0;
                let fill = self.role == Role::Drums && ((base.class == SeedClass::Filler && j + 1 == bars) || phrase_end);
                let ctx = BarCtx {
                    bar_in_phrase: (j as u64 % BARS_PER_PHRASE) as u8,
                    phrase: (first_bar / BARS_PER_PHRASE) as u32,
                    fill,
                    harmony,
                    focus: [1.0; STEPS_PER_BAR],
                    focus_shift: [0.0; MAX_GEN_PARAMS],
                    material_bar: if phrased { j } else { (first_bar + j as u64) as u16 },
                    ..*base
                };
                let mut tmp = std::mem::take(&mut work.d);
                self.front(&ctx, front, first_bar + j as u64, pos.prime, &mut tmp, work);
                for e in tmp.as_slice() {
                    let at = j as u32 * bar_ticks + e.tick as u32;
                    if at < limit && span.notes.len() < span.notes.capacity() {
                        let seq = span.notes.len() as u32;
                        span.notes.push(SpanNote { at, seq, ev: *e });
                    }
                }
                work.d = tmp;
            }
            if phrased {
                // The phrase starts `offset` beats into the letter's notes,
                // rotated; its notes end with it. A pitched phrase starts on
                // one of its notes at an even beat (user, 2026-10-21).
                // In clip mode the window starts anywhere in the clip: a
                // seeded part of it plus the offset (D50).
                let len = limit;
                let shift = if base.user.is_some() {
                    // The user's window starts where they set it.
                    0
                } else if self.role == Role::Drums {
                    (pos.offset as u32 * beat_ticks) % ticks
                } else {
                    let want = pos.offset as u32 + material.map_or(0, |b| clip_window(base.seed, c.lane, b));
                    onset_shift(&mut span.notes, want, len, beat_ticks)
                };
                for n in span.notes.iter_mut() {
                    n.at = (n.at + len - shift) % len;
                    n.ev.gate = n.ev.gate.min((len - n.at).min(u16::MAX as u32) as u16);
                }
                // A clip shorter than its phrase repeats; a longer one plays
                // the phrase's window of it.
                let written = span.notes.len();
                let mut lap = len;
                while lap < ticks {
                    for i in 0..written {
                        let mut n = span.notes[i];
                        n.at += lap;
                        if n.at < ticks && span.notes.len() < span.notes.capacity() {
                            n.seq = span.notes.len() as u32;
                            span.notes.push(n);
                        }
                    }
                    lap += len;
                }
                span.notes.retain(|n| n.at < ticks);
                for n in span.notes.iter_mut() {
                    n.ev.gate = n.ev.gate.min((ticks - n.at).min(u16::MAX as u32) as u16);
                }
            }
            ops::sort(&mut span.notes);
            let ctx = OpCtx { ticks, amount, seed: base.seed, lane: c.lane, drums: self.role == Role::Drums, harmony: base.harmony };
            ops::apply(chain, &mut span.notes, &ctx);
            span.ticks = ticks;
            span.key = key;
            span.valid = true;
        }
        // This bar's ticks in the span: the part from `from` starts at the
        // segment beat it is at; a segment longer than the span repeats it.
        let at_from = ((pos.beat_in_segment as u32 + if phrased { pos.skip as u32 } else { 0 }) * beat_ticks) % span.ticks;
        // A pitched phrase moves with the bar's chord, by scale steps, the
        // nearer way (the riff sequenced over the progression).
        // (Against a pitched host it was walked on the bar's chord already.)
        let chord_shift = match (phrased && self.role != Role::Drums && !base.host.on, base.harmony.chord_degree as i32) {
            (true, d) if d > 3 => d - 7,
            (true, d) => d,
            _ => 0,
        };

        out.clear();
        let mut lap = 0u32;
        while lap < at_from + bar_ticks {
            for n in &span.notes {
                let at = n.at + lap;
                if at < at_from || at >= at_from + bar_ticks - from as u32 {
                    continue;
                }
                let tick = from as u32 + (at - at_from);
                if tick < bar_ticks {
                    let step = if phrased { (tick / crate::TICKS_PER_STEP as u32) as u8 } else { n.ev.step };
                    let mut e = LiveEvent { tick: tick as u16, step, ..n.ev };
                    if chord_shift != 0 && e.kind == EventKind::NoteOn {
                        let (d, off) = ops::degree(e.key, &base.harmony);
                        e.key = ops::key_of(d + chord_shift, off, &base.harmony);
                    }
                    out.push(e);
                }
            }
            lap += span.ticks;
        }
        out.sort();
    }
}

/// A host's form position a follower plays (Splice, D46, D48): what the
/// host's form is built from, its pattern bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormFollow {
    pub spec: FormSpec,
    pub bar: u64,

    /// The relation (the Splice amount, 2a − 1) in hundredths: −100 layer ..
    /// 100 counter. On the counter side the host's questions are answered
    /// and its answers asked (D47).
    pub relation: i8,
}

/// Buffers [`PatternLane::render`] works in, allocated once.
#[derive(Clone, Default)]
pub struct Scratch {
    part: BarEvents,
    work: Work,
    span: [SpanCache; SPAN_SLOTS],
}

/// The chain's work buffers.
#[derive(Clone, Default)]
struct Work {
    a: BarEvents,
    b: BarEvents,
    c: BarEvents,
    d: BarEvents,
}

/// Spans [`Scratch`] keeps: a bar's parts (a question and its answer) each
/// keep theirs.
const SPAN_SLOTS: usize = 2;

/// A segment's transformed span (D47), kept while the bars of the segment
/// render one after the other: worked out once per span and inputs.
#[derive(Clone)]
pub struct SpanCache {
    key: u64,
    valid: bool,
    /// When it was last used (the oldest goes first).
    used: u64,
    ticks: u32,
    notes: Vec<SpanNote>,
}

impl Default for SpanCache {
    fn default() -> Self {
        Self {
            key: 0,
            valid: false,
            used: 0,
            ticks: 0,

            notes: Vec::with_capacity(MAX_SPAN_BARS * crate::MAX_EVENTS_PER_BAR),

        }
    }
}

/// Mix `v` into the running fingerprint `h`.
fn mix(h: u64, v: u64) -> u64 {
    rng::derive(h ^ v.wrapping_mul(0x9E37_79B9_7F4A_7C15), 7)
}

#[derive(Debug, Clone, Copy)]
struct PartCtx {
    lane: u8,
    bar: u64,
    bar_in_phrase: u8,
    phrase: u32,
    harmony: HarmonyCtx,
}

// MUSICAL: tune by ear (user, 2026-10-21: at 0 % Variation a varied block
// still differs a little from its original, at 100 % it plays the full
// variation). How far the operators go at Variation 0.
const OP_AMOUNT_FLOOR: f32 = 0.15;
/// How much further the operators go in the form's second pass.
const SECOND_PASS_OPS: f32 = 0.15;
const S_SPAN: u32 = 0xF60;
const S_SEEDED_OP: u32 = 0xF61;
const S_SWAP_QA: u32 = 0xF62;

// MUSICAL: tune by ear (2026-10-21: the operators move a follower's notes
// after its walk weighed them against the host). How much a neighbour in
// the scale (per degree) must fit better to replace a note.
const FIT_CLOSE: f32 = 0.35;

/// Move each note of a pitched follower (but the downbeat roots) to the
/// degree, at most two scale steps away, that fits the host's line best
/// (the walk's own weights, [`crate::splice::motion_weight`]); a note the
/// walk already fitted stays.
fn fit_to_host(out: &mut BarEvents, host: &crate::splice::HostLine, harmony: &HarmonyCtx) {
    let mut prev: Option<(usize, i32)> = None;
    for e in out.as_mut_slice().iter_mut().filter(|e| e.kind == EventKind::NoteOn) {
        let s = (e.tick / crate::TICKS_PER_STEP) as usize % STEPS_PER_BAR;
        if e.tick % TICKS_PER_BAR != 0 {
            let (d, off) = ops::degree(e.key, harmony);
            let mut best = (e.key, f32::MIN);
            let mut weigh = |key: u8, steps: i32| {
                let w = crate::splice::motion_weight(host, s, prev, key as i32) * FIT_CLOSE.powi(steps.min(2));
                if w > best.1 {
                    best = (key, w);
                }
            };
            for delta in -2i32..=2 {
                weigh(ops::key_of(d + delta, off, harmony), delta.abs());
            }
            // Layered, the same note an octave off may follow the host's way.
            if host.relation < 0.0 {
                for o in [-12i32, 12] {
                    let k = e.key as i32 + o;
                    if (1..=127).contains(&k) {
                        weigh(k as u8, 1);
                    }
                }
            }
            // The host's move from the previous note, followed or mirrored.
            if let Some((ps, pk)) = prev {
                let (h_prev, h_now) = (host.keys[ps] as i32, host.keys[s] as i32);
                if h_prev > 0 && h_now > 0 {
                    for to in [pk + (h_now - h_prev), pk - (h_now - h_prev)] {
                        let (td, _) = ops::degree(to.clamp(1, 127) as u8, harmony);
                        weigh(ops::key_of(td, 0, harmony), (td - d).abs());
                    }
                }
            }
            e.key = best.0;
        }
        prev = Some((s, e.key as i32));
    }
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
    // A note of the part held over its start comes in on its first tick
    // (user, 2026-10-21: it was lost, a silence up to half a bar long).
    let held = |e: &LiveEvent| e.kind == EventKind::NoteOn && e.gate > 0 && e.tick < from && e.tick + e.gate > from;
    let carried = part.as_slice().iter().find(|e| held(e)).copied();
    let enters = carried.is_some() || part.as_slice().iter().any(|e| e.kind == EventKind::NoteOn && e.tick >= from && e.tick < from + crate::TICKS_PER_STEP);
    for e in out.as_slice().iter().filter(|e| e.tick < from || e.tick >= to) {
        let mut e = *e;
        // The bar's notes end where the part's start.
        if enters && held(&e) {
            e.gate = from - e.tick;
        }
        scratch.push(e);
    }
    if let Some(mut e) = carried {
        e.gate = (e.tick + e.gate).min(to) - from;
        e.tick = from;
        e.step = (from / crate::TICKS_PER_STEP) as u8;
        e.flags &= !(flags::TIE | flags::REVERSE);
        e.loop_len = 0;
        e.note_id = e.note_id.wrapping_add(0x1000);
        scratch.push(e);
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

// MUSICAL: tune by ear (user: Variation widens the range of the sound
// knobs, the right-hand ones, not of the pattern knobs; 2026-10-20: far
// too wide, little at low Variation, less than 1.7 at full).
/// Scale of the letter and note moves of [`REACHED`] at Variation 0 and 1,
/// and the curve between (about 0.4 in the middle).
const VARIATION_REACH: (f32, f32) = (0.05, 1.0);
const VARIATION_REACH_CURVE: f32 = 1.5;
/// The parameters of the right-hand knobs (Shape, Morph, Spread, Slide):
/// Variation sets how far they move. The pattern knobs' parameters move
/// as the form says.
const REACHED: [&str; 4] = ["length", "chop", "spread", "slide"];
// MUSICAL: first draft, tune by ear.
/// Per-note moves of parameters the letters leave alone, at reach 1.
const NOTE_ONLY_MOVES: [(&str, f32); 1] = [("length", 0.12)];
// MUSICAL: tune by ear (user, 2026-10-20: Shape per hit the one parameter
// that moves much, by the seed; 2026-10-21: but at low Variation it still
// moved far too much). The per-note Length move, its scale at Variation 0
// and 1 (on a curve, so it stays small up to about half Variation) and the
// pattern seed's scale.
const SHAPE_NOTE: f32 = 0.3;
const SHAPE_NOTE_REACH: (f32, f32) = (0.1, 1.3);
const SHAPE_NOTE_CURVE: f32 = 2.5;
const SHAPE_NOTE_SEED: (f32, f32) = (0.6, 1.4);
const S_SHAPE_NOTE: u32 = 0xF54;

/// The per-note Length move of a lane (see [`SHAPE_NOTE`]).
fn shape_note(names: &[&str], params: &[f32; MAX_GEN_PARAMS], pattern_seed: u32, lane: u8) -> f32 {
    let v = names.iter().position(|n| *n == "variation").and_then(|i| params.get(i)).copied().unwrap_or(0.5).clamp(0.0, 1.0);
    let u = rng::unit_f32(rng::hash(pattern_seed, lane, 0, 0, S_SHAPE_NOTE));
    SHAPE_NOTE * (SHAPE_NOTE_REACH.0 + (SHAPE_NOTE_REACH.1 - SHAPE_NOTE_REACH.0) * v.powf(SHAPE_NOTE_CURVE)) * (SHAPE_NOTE_SEED.0 + (SHAPE_NOTE_SEED.1 - SHAPE_NOTE_SEED.0) * u)
}

/// The reach of parameter `name`.
fn reach_of(name: &str, reach: f32) -> f32 {
    if REACHED.contains(&name) { reach } else { 1.0 }
}

/// How far letters and notes move the parameters, from the lane's Variation.
fn variation_reach(names: &[&str], params: &[f32; MAX_GEN_PARAMS]) -> f32 {
    let v = names.iter().position(|n| *n == "variation").and_then(|i| params.get(i)).copied().unwrap_or(0.5);
    VARIATION_REACH.0 + (VARIATION_REACH.1 - VARIATION_REACH.0) * v.clamp(0.0, 1.0).powf(VARIATION_REACH_CURVE)
}

/// Per-note jitter amplitude per parameter ([`NOTE_JITTER_SHARE`] of the
/// letter offsets). Only the right-hand knobs' parameters ([`REACHED`])
/// move per note (user, 2026-10-20); the pattern knobs' stay per bar.
fn note_jitter(names: &[&str], reach: f32) -> [f32; MAX_GEN_PARAMS] {
    let mut out = [0.0; MAX_GEN_PARAMS];
    let moves = LETTER_OFFSETS.iter().map(|(n, m)| (*n, NOTE_JITTER_SHARE * m)).chain(NOTE_ONLY_MOVES).filter(|(n, _)| REACHED.contains(n));
    for (name, amp) in moves {
        if let Some(i) = names.iter().position(|n| *n == name) {
            if let Some(j) = out.get_mut(i) {
                *j = reach_of(name, reach) * amp;
            }
        }
    }
    out
}

// MUSICAL: tune by ear (D50). A clip's window: a seeded block of this many
// beats in the clip, the phrase's offset on top.
const CLIP_WINDOW_BEATS: u32 = 16;
const S_CLIP_WINDOW: u32 = 0xF63;

/// The seeded start (beats) of a letter's window in its clip of `beats`.
fn clip_window(seed: u32, lane: u8, beats: u32) -> u32 {
    let blocks = (beats / CLIP_WINDOW_BEATS).max(1);
    CLIP_WINDOW_BEATS * rng::below(rng::hash(seed, lane, 0, 0, S_CLIP_WINDOW), blocks)
}

/// Where a pitched phrase starts in its letter's notes (ticks): the even
/// beat at or after `want` (beats) where a note starts, or starts at most a
/// 16th earlier (that note is clipped to start on the beat); the letter's own
/// start when no beat has one. Even beats keep the kick's beats with the
/// kick, a note on the beat keeps the phrase from sounding late.
/// How late after a beat a note still counts as on it (the letter's feel).
const ONSET_FEEL_TICKS: u32 = crate::form::MAX_SWING_TICKS as u32;

fn onset_shift(notes: &mut [SpanNote], want: u32, ticks: u32, beat: u32) -> u32 {
    let points = ticks / (2 * beat);
    let first = (want / 2) % points.max(1);
    for k in 0..points {
        let p = (first + k) % points;
        if p == 0 {
            return 0;
        }
        let t = p * 2 * beat;
        // A note from a 16th before the beat (clipped to it) to a few ticks
        // after it (the letter's feel moved it, D57): the nearest one.
        let sounds = |n: &SpanNote| {
            matches!(n.ev.kind, EventKind::NoteOn | EventKind::Trigger)
                && n.ev.vel > 0
                && n.at + crate::TICKS_PER_STEP as u32 >= t
                && n.at <= t + ONSET_FEEL_TICKS
                && (n.ev.gate == 0 || n.at + n.ev.gate as u32 > t)
        };
        if let Some(n) = notes.iter_mut().filter(|n| sounds(n)).min_by_key(|n| n.at.abs_diff(t)) {
            if n.at < t {
                if n.ev.gate > 0 {
                    n.ev.gate -= (t - n.at) as u16;
                }
                n.at = t;
            }
            return t;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_softens_hits_near_its_centre_and_drops_them_far_out() {
        // User, 2026-10-27: off the centre a little the hits against the
        // setting play softer instead of going.
        let mut occ = [0.0f32; STEPS_PER_BAR];
        for s in (0..STEPS_PER_BAR).step_by(4) {
            occ[s] = 1.0;
        }
        let thinned = |amount: f32| {
            let mut bar = BarEvents::default();
            for s in 0..STEPS_PER_BAR {
                // Every step taken, so a hit has nowhere to move.
                bar.push(crate::event::LiveEvent { tick: s as u16 * crate::TICKS_PER_STEP, kind: EventKind::Trigger, vel: 100, target: 1, ..Default::default() });
            }
            splice_thin(&mut bar, &crate::splice::focus(amount, &occ), 7, 1, 0.0);
            let vels: Vec<u8> = bar.as_slice().iter().map(|e| e.vel).collect();
            (vels.len(), vels.iter().filter(|v| **v < 100).count())
        };
        let (near, far) = (thinned(0.7), thinned(1.0));
        assert_eq!(near.0, STEPS_PER_BAR, "near the centre none goes: {near:?}");
        assert!(near.1 > 0, "but some play softer: {near:?}");
        assert!(far.0 < STEPS_PER_BAR, "far out some go: {far:?}");
    }

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
    fn normal_plays_16ths_and_half_speed_8ths() {
        // D57: Normal is the generator's own pace; half speed keeps 8ths
        // (at Groove 0: no recursion and no moves). A pushed 8th keeps its
        // onset (the speed's slack).
        for (rate, min) in [(0i8, TICKS_PER_BAR / 16 * 3 / 4), (-1, TICKS_PER_BAR / 8 * 3 / 4)] {
            let mut fastest = u16::MAX;
            for seed in 0..40 {
                let mut lane = PatternLane::bass(seed, seed);
                lane.rate = rate;
                lane.set_param("syncopation", 0.0);
                for bar in render_all(&lane, 8) {
                    let on: Vec<u16> = bar.iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| e.tick).collect();
                    fastest = on.windows(2).map(|w| w[1] - w[0]).fold(fastest, u16::min);
                }
            }
            assert!(fastest >= min && fastest < u16::MAX, "rate {rate}: fastest {fastest} ticks");
        }
    }

    #[test]
    fn splice_lengths_follow_or_answer_the_host() {
        use crate::splice::HostLine;
        // Mean gate of the notes after each bar's first, over seeds and bars.
        let gates = |host: HostLine| {
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for seed in 0..12 {
                let mut lane = PatternLane::bass(seed, seed);
                lane.set_param("density", 0.8);
                lane.host_line = host;
                for bar in render_all(&lane, 8) {
                    for e in bar.iter().filter(|e| e.kind == EventKind::NoteOn).skip(1) {
                        sum += e.gate as f32;
                        n += 1.0;
                    }
                }
            }
            sum / n
        };
        let long = HostLine { on: true, keys: [40; 16], lens: [8; 16], onsets: 1 | 1 << 8, ..Default::default() };
        let short = HostLine { on: true, keys: [40; 16], lens: [1; 16], onsets: 0xFFFF, ..Default::default() };
        let at = |l: HostLine, relation: f32| gates(HostLine { relation, ..l });
        let (ll, lc) = (at(long, -1.0), at(long, 1.0));
        let (sl, sc) = (at(short, -1.0), at(short, 1.0));
        assert!(ll > 1.3 * lc, "a long host: layered {ll} vs countered {lc}");
        assert!(sc > 1.3 * sl, "a short host: countered {sc} vs layered {sl}");
        // The middle sits in between, nearer the counter end.
        let (lm, sm) = (at(long, 0.0), at(short, 0.0));
        assert!(lm < ll && lm > lc * 0.9 && sm > sl && sm < sc * 1.1, "middle: long {lm}, short {sm}");
    }

    #[test]
    fn permute_moves_chunks_and_keeps_the_downbeat() {
        let chunk = TICKS_PER_BAR / PERMUTE_CHUNKS as u16;
        for drums in [true, false] {
            let lane = |amount: f32| {
                let mut l = if drums { PatternLane::drums(DrumLayout::standard(), 7, 3) } else { PatternLane::bass(7, 3) };
                l.permute = amount;
                // No stabs: the Length phase splits no held note (their
                // lengths differ once the chunks move).
                l.set_param("stabs", 0.0);
                l
            };
            let plain = render_all(&lane(0.0), 8);
            let full = render_all(&lane(1.0), 8);
            let mut moved = 0;
            for (a, b) in plain.iter().zip(&full) {
                assert_eq!(a.len(), b.len(), "every hit stays");
                // (The melody walks the moved line anew: its pitches move.)
                let first = |v: &[crate::event::LiveEvent]| v.iter().filter(|e| e.tick < chunk).map(|e| (e.tick, e.target)).collect::<Vec<_>>();
                assert_eq!(first(a), first(b), "the downbeat chunk stays");
                let ticks = |v: &[crate::event::LiveEvent]| v.iter().map(|e| (e.tick, e.target)).collect::<Vec<_>>();
                moved += (ticks(a) != ticks(b)) as usize;
            }
            assert!(moved >= 6, "most bars change: {moved} of 8");
        }
    }

    #[test]
    fn a_bar_is_a_pure_function_of_its_inputs() {
        let phrased = |mut l: PatternLane| {
            l.set_form(Form::parse("A1.5:8+2 B.5:2 A2:4 C.5:2 B1.5r:16^4 A26.5:8").unwrap());
            l
        };
        for lane in [PatternLane::drums(DrumLayout::standard(), 7, 3), PatternLane::bass(7, 3), phrased(PatternLane::drums(DrumLayout::standard(), 7, 3)), phrased(PatternLane::bass(7, 3))] {

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
    fn generated_forms_place_the_fills_at_the_seams() {
        let mut fills = 0;
        for seed in 0..40 {
            let mut lane = PatternLane::drums(DrumLayout::standard(), 5, 5);
            lane.set_form_spec(crate::macros::FormSettings::seeded(seed).spec(true, 0.5, 0.5, 0.5));
            let f = lane.form();
            // The seams at bars 32 and 64 (the ones at 16 and 48 are rarer).
            fills += [127u64, 255].iter().filter(|&&b| f.pos_at(b).letter.class() == SeedClass::Filler).count();
            // A bar of a section's seam plays its fill.
            let key = MusicalKey::new(0, true);
            let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
            lane.render(15, key, &mut out, &mut scratch);
            assert!(!out.events.is_empty());
        }
        assert!(fills > 60, "{fills} of 80 seams at bars 32 and 64 end on a fill");
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

    /// The events of `bar` of `lane` (fresh scratch: no cache in between).
    fn bar_alone(lane: &PatternLane, bar: u64) -> Vec<crate::event::LiveEvent> {
        let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
        lane.render(bar, MusicalKey::new(5, true), &mut out, &mut scratch);
        out.events.as_slice().to_vec()
    }

    fn user_note(tick: u32, row: i16) -> crate::slots::UserNote {
        crate::slots::UserNote { tick, len: 48, row, vel: 100, start: 0, choke: 0 }
    }

    #[test]
    fn a_users_pattern_is_the_skeleton() {
        // A seed whose A plays at its own pace (no speed step).
        let seed = (0..400).find(|&s| PatternLane::bass(s, s).letter_rate(lane_index(Role::Bass), Letter::A) == 0).unwrap();
        let mut lane = PatternLane::bass(seed, seed);
        lane.set_param("syncopation", 0.0);
        lane.set_param("density", 0.5);
        let mut book = PatternBook::default();
        book.slots[0] = Some(UserPattern { notes: vec![user_note(0, 0), user_note(192, 7)], ticks: 384, start: 0, len: 384 });
        let before = bar_alone(&lane, 0);
        lane.set_patterns(std::sync::Arc::new(book));
        let ons: Vec<_> = bar_alone(&lane, 0).into_iter().filter(|e| e.kind == EventKind::NoteOn).collect();
        assert_eq!(ons.len(), 2, "{ons:?}");
        assert!(ons[0].tick < 12 && (180..204).contains(&ons[1].tick), "{ons:?}");
        assert_eq!(ons[1].key as i32 - ons[0].key as i32, 7);
        assert_ne!(before.iter().filter(|e| e.kind == EventKind::NoteOn).count(), 0);
        // B keeps its seeded pattern.
        let mut b = Form::generated();
        b.set_one(Segment::new(Letter::B, 4, 0));
        lane.set_form(b);
        assert!(!bar_alone(&lane, 0).is_empty());
    }

    #[test]
    fn a_window_of_the_users_pattern_loops() {
        let seed = (0..400).find(|&s| PatternLane::bass(s, s).letter_rate(lane_index(Role::Bass), Letter::A) == 0).unwrap();
        let mut lane = PatternLane::bass(seed, seed);
        lane.set_param("syncopation", 0.0);
        lane.set_param("density", 0.5);
        // Two bars of source, the window the second bar's first half.
        let mut book = PatternBook::default();
        book.slots[0] = Some(UserPattern { notes: vec![user_note(0, 0), user_note(384, 3), user_note(480, 5)], ticks: 768, start: 384, len: 192 });
        lane.set_patterns(std::sync::Arc::new(book));
        lane.play_slot(Some((Slot::from_index(0), VelOp::Plain)));
        let ons: Vec<_> = bar_alone(&lane, 0).into_iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| e.key as i32).collect();
        // Twice the window: rows 3 and 5, twice.
        assert_eq!(ons.len(), 4, "{ons:?}");
        assert_eq!((ons[1] - ons[0], ons[2] - ons[0], ons[3] - ons[0]), (2, 0, 2));
    }

    #[test]
    fn a_slots_source_holds_its_window() {
        for lane in [PatternLane::bass(7, 7), PatternLane::drums(DrumLayout::standard(), 7, 7)] {
            for slot in Slot::ALL {
                let src = lane.slot_source(slot, MusicalKey::new(5, true));
                assert!(!src.notes.is_empty(), "{slot:?}");
                assert!(src.start + src.len <= src.ticks && src.len >= crate::slots::MIN_WINDOW);
                assert!(src.notes.windows(2).all(|w| w[0].tick <= w[1].tick));
                if lane.role() == Role::Drums {
                    assert!(src.notes.iter().all(|n| crate::generator::layout::DrumVoice::of_target(n.row as u8).is_some()));
                } else {
                    assert!(src.notes.iter().all(|n| (-24..=36).contains(&n.row)), "{:?}", src.notes);
                }
            }
            // The two patterns of a letter differ.
            let (a1, a2) = (lane.slot_source(Slot::from_index(0), MusicalKey::new(5, true)), lane.slot_source(Slot::from_index(1), MusicalKey::new(5, true)));
            assert_ne!(a1.notes, a2.notes);
        }
    }

    #[test]
    fn a_variant_plays_its_letters_first_bars_and_repeats_exactly() {
        for drums in [true, false] {
            let mut lane = if drums { PatternLane::drums(DrumLayout::standard(), 7, 3) } else { PatternLane::bass(7, 3) };
            lane.set_form(Form::parse("A3 Ar2 A11").unwrap());
            lane.rate = 0;
            lane.set_param("density", 0.75);
            // Rendered in order (the span cached) or alone: the same bars.
            let ordered = render_all(&lane, 8);
            for b in [4u64, 3, 6, 4] {
                assert_eq!(bar_alone(&lane, b), ordered[b as usize], "bar {b}");
            }
            if !drums {
                // `A2r` after `A3`: A's first two bars, their pitches reordered.
                let keys = |bars: &[Vec<crate::event::LiveEvent>]| {
                    let mut k: Vec<(u8, u16)> = bars.iter().flatten().filter(|e| e.kind == EventKind::NoteOn).map(|e| (e.key, e.gate)).collect();
                    let order: Vec<u8> = k.iter().map(|x| x.0).collect();
                    k.sort_unstable();
                    (k.into_iter().map(|x| x.0).collect::<Vec<_>>(), order)
                };
                let (a, a_order) = keys(&ordered[0..2]);
                let (v, v_order) = keys(&ordered[3..5]);
                assert_eq!(a, v, "the same notes");
                assert_ne!(a_order, v_order, "in another order");
            }
        }
    }

    #[test]
    fn contrast_moves_b_from_a_and_keeps_the_knobs_character() {
        let key = MusicalKey::new(5, true);
        let (mut out, mut scratch) = (RenderedBar::default(), Scratch::default());
        let mut equal = 0;
        // Per Shape knob end, the notes A and B play at full Contrast.
        let mut notes = [[0usize; 2]; 2];
        let mut gates = [[0f32; 2]; 2];
        for seed in 0..40 {
            for drums in [false, true] {
                let mut lane = if drums { PatternLane::drums(DrumLayout::standard(), seed, 3) } else { PatternLane::bass(seed, 3) };
                lane.set_form(Form::parse("A1 B1 A62").unwrap());
                lane.contrast = 0.0;
                lane.render(0, key, &mut out, &mut scratch);
                let a = out.params.values;
                lane.render(1, key, &mut out, &mut scratch);
                // But the drums' style: B plays another at any Contrast (user,
                // 2026-10-26).
                let style = lane.param_names().iter().position(|n| *n == "style");
                let other = |v: [f32; MAX_GEN_PARAMS]| { let mut v = v; if let Some(i) = style { v[i] = 0.0; } v };
                assert_eq!(other(out.params.values), other(a), "seed {seed}: at Contrast 0 B plays A's settings");
                if let Some(i) = style {
                    assert_ne!(out.params.values[i], a[i], "seed {seed}: B's style");
                }
                lane.contrast = 1.0;
                lane.render(1, key, &mut out, &mut scratch);
                equal += (out.params.values == a) as u32;
                if drums {
                    continue;
                }
                for (end, shape) in [0.0f32, 1.0].into_iter().enumerate() {
                    lane.set_param("length", shape);
                    for (l, bar) in [0u64, 1].into_iter().enumerate() {
                        lane.render(bar, key, &mut out, &mut scratch);
                        for e in out.events.as_slice().iter().filter(|e| e.kind == EventKind::NoteOn) {
                            notes[end][l] += 1;
                            gates[end][l] += e.gate as f32;
                        }
                    }
                }
            }
        }
        assert_eq!(equal, 0, "at Contrast 1 B moves away from A");
        // B mirrors the Shape (user, 2026-10-26): at Shape 0 its notes far
        // longer than A's, at Shape 1 far shorter.
        let mean = |end: usize, l: usize| gates[end][l] / notes[end][l].max(1) as f32;
        assert!(mean(0, 1) > 2.0 * mean(0, 0), "Shape 0: B holds: {gates:?} {notes:?}");
        assert!(mean(1, 1) < 0.6 * mean(1, 0), "Shape 1: B stabs: {gates:?} {notes:?}");
    }

    #[test]
    fn a_follower_shares_its_hosts_operators_and_answers_its_questions() {
        let mut lane = PatternLane::bass(4, 4);
        let mut q = OpChain::default();
        q.push(Op::Retrograde);
        q.push(Op::Question);
        let swapped = |lane: &PatternLane| (0..100u8).filter(|&seg| lane.resolve_ops(&FormPos { ops: q, segment: seg, ..FormPos::default() }, 9, 1).contains(Op::Answer)).count();
        assert_eq!(swapped(&lane), 0, "alone: the form's own operators");
        for (relation, lo, hi) in [(-100i8, 0, 0), (0, 0, 0), (50, 30, 70), (100, 100, 100)] {
            lane.follow = Some(FormFollow { spec: FormSpec::default(), bar: 0, relation });
            let n = swapped(&lane);
            assert!((lo..=hi).contains(&n), "relation {relation}: {n} of 100 questions answered");
            // The other operators stay the host's.
            assert!(lane.resolve_ops(&FormPos { ops: q, ..FormPos::default() }, 9, 1).contains(Op::Retrograde));
        }
    }

    #[test]
    fn variation_scales_how_far_a_variant_moves() {
        // Notes of a variant bar that are not where A's bar has them.
        let moved = |variation: f32, form: &str| {
            let mut total = 0;
            for seed in 1..=6u32 {
                let mut lane = PatternLane::bass(seed, seed);
                lane.rate = 0;
                lane.set_param("variation", variation);
                lane.set_form(Form::parse(form).unwrap());
                let bars = render_all(&lane, 8);
                for b in 0..4 {
                    let plain: std::collections::BTreeSet<(u16, u8)> = bars[b].iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| (e.tick, e.key)).collect();
                    total += bars[b + 4].iter().filter(|e| e.kind == EventKind::NoteOn && !plain.contains(&(e.tick, e.key))).count();
                }
            }
            total
        };
        for form in ["A4 Am4 A8", "A4 Ar4 A8", "A4 Az4 A8", "A4 A?4 A8"] {
            let (little, full) = (moved(0.0, form), moved(1.0, form));
            assert!(little > 0 && full > little, "{form}: {little} at Variation 0, {full} at 1");
        }
    }


    #[test]
    fn a_clip_phrase_is_a_window_of_its_clip_from_a_note_on_an_even_beat() {
        let clips = crate::clips::ClipSet::builtin();
        let beat = TICKS_PER_BAR as u32 / 4;
        let (mut from_note, mut tried) = (0, 0);
        for seed in 1..=40u32 {
            let mut lane = PatternLane::bass(seed, seed);
            lane.set_param("source", 1.0);
            lane.set_form(Form::parse("A16:8+2").unwrap());
            lane.rate = 0;
            lane.phrase_bars = 64;
            // Blocks at their own speed play their window faster or slower.
            if lane.letter_rate(lane_index(Role::Bass), Letter::A) != 0 {
                continue;
            }
            tried += 1;
            let bars = render_all(&lane, 2);
            let got: Vec<u32> = bars
                .iter()
                .enumerate()
                .flat_map(|(b, bar)| bar.iter().filter(|e| e.kind == EventKind::NoteOn).map(move |e| b as u32 * TICKS_PER_BAR as u32 + e.tick as u32))
                .collect();
            // Some clip of A, read from an even beat, plays these onsets
            // (the letter's feel moves them a little).
            let fits = clips.letters[0].iter().any(|c| {
                (0..c.beats as u32).step_by(2).any(|start| {
                    let mut want: Vec<u32> = c
                        .notes
                        .iter()
                        .map(|n| (n.tick + c.ticks() - start * beat) % c.ticks())
                        .filter(|&t| t < 8 * beat)
                        .collect();
                    want.sort_unstable();
                    want.len() == got.len() && want.iter().zip(&got).all(|(w, g)| w.abs_diff(*g) <= 8)
                })
            });
            assert!(fits, "seed {seed}: a window of an A clip: {got:?}");
            from_note += (got.first().is_some_and(|&t| t <= 8)) as u32;
        }
        assert!(tried >= 12, "{tried} seeds at the lane's speed");
        assert!(from_note * 5 >= tried * 4, "the window starts on a note: {from_note} of {tried}");
    }

    #[test]
    fn a_phrase_loops_from_its_own_start() {
        for drums in [true, false] {
            let lane = |form: &str| {
                let mut l = if drums { PatternLane::drums(DrumLayout::standard(), 7, 3) } else { PatternLane::bass(7, 3) };
                l.set_form(Form::parse(form).unwrap());
                l.rate = 0;
                l.phrase_bars = 64;
                l
            };
            // Pitched phrases follow the song's chords: compare the bass's
            // rhythm, and its notes as steps from the bar's first.
            let onsets = |bars: &[Vec<crate::event::LiveEvent>]| {
                let h = HarmonyCtx { key: MusicalKey::new(5, true), chord_degree: 0, progression: 0, mode: 0 };
                let mut v = Vec::new();
                for (b, bar) in bars.iter().enumerate() {
                    let first = bar.iter().find(|e| e.kind == EventKind::NoteOn).map_or(0, |e| ops::degree(e.key, &h).0);
                    for e in bar.iter().filter(|e| matches!(e.kind, EventKind::NoteOn | EventKind::Trigger)) {
                        let id = if drums { e.key as i32 } else { ops::degree(e.key, &h).0 - first };
                        v.push((b as u32 * TICKS_PER_BAR as u32 + e.tick as u32, id));
                    }
                }
                v.sort_unstable();
                v
            };
            // Two bars of phrase: bars 2 and 3 play bars 0 and 1 again.
            let two = render_all(&lane("A16:8"), 4);
            assert_eq!(onsets(&two[2..4]), onsets(&two[0..2]), "drums {drums}: an 8-beat phrase repeats every 2 bars");
            // Half a bar: each bar plays it twice (the timing drift of the
            // letter's feel aside).
            let half = render_all(&lane("A16:2"), 2);
            let near = |a: &[(u32, i32)], b: &[(u32, i32)]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.1 == y.1 && x.0.abs_diff(y.0) <= 3);
            let halves = |bar: &[crate::event::LiveEvent], h: u32| {
                let mid = TICKS_PER_BAR as u32 / 2;
                onsets(std::slice::from_ref(&bar.to_vec())).into_iter().filter(|o| (o.0 >= mid) == (h == 1)).map(|o| (o.0 % mid, o.1)).collect::<Vec<_>>()
            };
            assert!(!halves(&half[0], 0).is_empty());
            assert!(near(&halves(&half[0], 0), &halves(&half[0], 1)), "drums {drums}: a 2-beat phrase twice per bar");
            assert!(near(&halves(&half[1], 0), &halves(&half[0], 0)));
            // An offset rotates where it starts in the letter's notes: the
            // same notes over the phrase, in another order.
            let rotated = render_all(&lane("A16:8+2"), 2);
            let ticks = |o: Vec<(u32, i32)>| o.into_iter().map(|x| x.0 % (2 * TICKS_PER_BAR as u32)).collect::<Vec<_>>();
            let beat = TICKS_PER_BAR as u32 / 4;
            let got = ticks(onsets(&rotated));
            // Drums by the written 2 beats; a pitched phrase from the even
            // beat at or after it where a note starts, or the letter's start.
            let shifts: &[u32] = if drums { &[2] } else { &[2, 4, 6, 0] };
            let fits = shifts.iter().any(|&sh| {
                let mut moved: Vec<u32> = ticks(onsets(&two[0..2])).into_iter().map(|t| (t + (8 - sh) * beat) % (8 * beat)).collect();
                moved.sort_unstable();
                got.len() == moved.len() && got.iter().zip(&moved).all(|(a, b)| a.abs_diff(*b) <= crate::TICKS_PER_STEP as u32)
            });
            assert!(fits, "drums {drums}: the same notes from another beat: {got:?}");
            if !drums {
                assert!(got.iter().any(|&t| t <= 3), "a pitched phrase starts on a note: {got:?}");
            }
            if drums {
                let ids = |o: Vec<(u32, i32)>| {
                    let mut k: Vec<i32> = o.into_iter().map(|x| x.1).collect();
                    k.sort_unstable();
                    k
                };
                assert_eq!(ids(onsets(&rotated)), ids(onsets(&two[0..2])));
            }
            // A skip enters the phrase later: one bar in.
            let skipped = render_all(&lane("A1:8^4 A15:8"), 1);
            assert_eq!(ticks(onsets(&skipped[0..1])), ticks(onsets(&two[1..2])), "drums {drums}: skipped one bar in");


        }
    }

    #[test]
    fn a_new_form_waits_for_the_next_16_bar_section() {
        // D61: a Contrast (or Variation, Shape) band moved mid-section plays
        // from the next section on.
        let mut lane = PatternLane::bass(3, 3);
        lane.defer_forms = true;
        let seeded = crate::macros::FormSettings::seeded(3);
        let (a, b) = (seeded.spec(false, 0.5, 0.5, 0.0), seeded.spec(false, 0.5, 0.5, 1.0));
        lane.set_form_spec(a);
        let first = lane.form().clone();
        lane.set_form_spec(b);
        assert_eq!((lane.form_spec(), lane.form()), (Some(a), &first), "the first form plays at once, a new one waits");
        assert!(!lane.advance_form(5, false));
        assert!(lane.advance_form(16, false));
        assert_eq!(lane.form_spec(), Some(b));
        assert_ne!(lane.form(), &first);
        // Back before it plays: nothing waits.
        lane.set_form_spec(a);
        lane.set_form_spec(b);
        assert!(!lane.advance_form(32, false));
        assert_eq!(lane.form_spec(), Some(b));
    }

    #[test]
    fn rendering_does_not_allocate() {
        let with_ops = |mut l: PatternLane| {
            l.set_form(Form::parse("A3 Ar2 A2zq B?1 Bix4 C4").unwrap());
            l
        };
        let phrased = |mut l: PatternLane| {
            l.set_form(Form::parse("C.5:2 A1.5:8^2 B.5:2 A.5r:2+1 B1:4 A12:16+4").unwrap());
            l
        };
        for lane in [
            PatternLane::drums(DrumLayout::standard(), 11, 4),
            PatternLane::bass(11, 4),
            with_ops(PatternLane::drums(DrumLayout::standard(), 11, 4)),
            with_ops(PatternLane::bass(11, 4)),
            phrased(PatternLane::drums(DrumLayout::standard(), 11, 4)),
            phrased(PatternLane::bass(11, 4)),
            {
                let mut l = phrased(PatternLane::bass(11, 4));
                l.set_param("source", 1.0);
                l
            },
            {
                let mut l = with_ops(PatternLane::bass(11, 4));
                l.set_param("source", 1.0);
                l
            },
        ] {

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
