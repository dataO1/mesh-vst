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
use crate::form::{
    form_chord, letter_offset, letter_seed, prime_seed, splice_second_half, Feel, Form, FormPos, Letter, Play, SeedClass,
    DEFAULT_FORMS, FILLER_BOOSTS, LETTER_OFFSETS, NOTE_JITTER_SHARE,
};
use crate::generator::bass::{self, BassGenerator, BassRules};
use crate::generator::blocks::{defaults_from_spec, patterned, NoteShape};
use crate::generator::drums::{self, DrumGenerator, DrumRules};
use crate::generator::layout::DrumVoice;
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
// MUSICAL: user, 2026-10-20: the centre speed was twice too fast. Speed
// exponent under the Rate setting: Normal plays what half speed played,
// Double what Normal played, Half half of that.
const BASE_RATE: i8 = -1;
/// RNG stream of the sound seed a filler letter derives.
const S_SOUND: u32 = 0xF52;
const S_PERMUTE: u32 = 0xF53;

// MUSICAL: first draft, tune by ear (user: Groove, the drums' feel).
/// Ticks each voice plays off the grid at full Groove: the snare laid back,
/// the hats and shakers pushed, the kick on the grid.
const GROOVE_PUSH: [(DrumVoice, i32); 6] = [
    (DrumVoice::Snare, 4),
    (DrumVoice::Hats, -2),
    (DrumVoice::Shaker, -2),
    (DrumVoice::Ride, -1),
    (DrumVoice::Perc, 2),
    (DrumVoice::Top, 1),
];
/// Velocity scale of the cymbals' off 16ths and on-beat 8ths at full Groove.
const GROOVE_OFF_VEL: f32 = 0.55;
const GROOVE_ON_VEL: f32 = 1.15;
/// Velocity of snare ghosts (hits below [`GHOST_VEL`]) at full Groove.
const GROOVE_GHOST_VEL: f32 = 0.7;
const GHOST_VEL: u8 = 70;

/// The drums' Groove beyond the swing: each voice pushed or laid back, the
/// cymbals' accent shape (strong 8ths, soft off 16ths), softer ghosts.
/// `amount` 0 plays the bar as rendered.
fn groove(out: &mut BarEvents, amount: f32) {
    let g = amount.clamp(0.0, 1.0);
    if g <= 0.0 {
        return;
    }
    for e in out.as_mut_slice() {
        let Some(voice) = DrumVoice::of_target(e.target) else { continue };
        if let Some((_, push)) = GROOVE_PUSH.iter().find(|(v, _)| *v == voice) {
            let to = e.tick as i32 + (*push as f32 * g).round() as i32;
            e.tick = to.clamp(0, TICKS_PER_BAR as i32 - 1) as u16;
            e.gate = e.gate.min(TICKS_PER_BAR - e.tick);
        }
        let scale = match voice {
            DrumVoice::Hats | DrumVoice::Ride | DrumVoice::Shaker => {
                let off = e.step % 2 == 1;
                1.0 + (if off { GROOVE_OFF_VEL } else { GROOVE_ON_VEL } - 1.0) * g
            }
            DrumVoice::Snare if e.vel < GHOST_VEL => 1.0 + (GROOVE_GHOST_VEL - 1.0) * g,
            _ => 1.0,
        };
        if e.vel > 0 && scale != 1.0 {
            e.vel = (e.vel as f32 * scale).round().clamp(1.0, 127.0) as u8;
        }
    }
    out.sort();
}

/// Parameters the Splice focus moves per step, the rest stay.
const FOCUS_PARAMS: [&str; 3] = ["density", "variation", "spread"];
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
/// it drops, up to [`crate::splice::DROP_MAX`] of them where the focus is
/// lowest. The bar stays sorted, ties and gates move with their notes.
fn splice_thin(out: &mut BarEvents, focus: &[f32; STEPS_PER_BAR], seed: u32, lane: u8, bar_in_phrase: u8) {
    if focus.iter().all(|f| *f >= 1.0) {
        return;
    }
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
        let h = rng::hash(seed, lane, bar_in_phrase, (e.tick / 4) as u8 ^ e.target, S_SPLICE);
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
            // No place to go: it drops (velocity 0, removed below).
            None if rng::unit_f32(h.rotate_left(17)) < low * crate::splice::DROP_MAX => e.vel = 0,
            None => {}
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
fn permute(out: &mut BarEvents, amount: f32, seed: u32, lane: u8, bar_in_phrase: u8) {
    let swaps = (amount.clamp(0.0, 1.0) * PERMUTE_CHUNKS as f32).round() as usize;
    if swaps == 0 {
        return;
    }
    // Where each chunk's content plays.
    let mut dest: [usize; PERMUTE_CHUNKS] = std::array::from_fn(|i| i);
    // The same swaps for the bars of a 4-bar block, so a block keeps its shape.
    let h = |k: u8, s: u32| rng::hash(seed, lane, bar_in_phrase % 4, k, s);
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
    bar_in_phrase: u8,
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
        patterned(*v, *amp, self.seed, self.lane, self.bar_in_phrase, index, shape)
    }

    /// The shape of a played note `ev` (its step, gate and height).
    pub fn shape_of(&self, ev: &LiveEvent) -> NoteShape {
        NoteShape::note(ev.step, ev.gate, ev.key as i32, self.root)
    }

    /// The Morph move of bass note `ev`, in knob values (see [`MORPH_NOTE`]).
    pub fn morph_move(&self, ev: &LiveEvent) -> f32 {
        patterned(0.5, MORPH_NOTE * self.reach, self.seed, self.lane, self.bar_in_phrase, MORPH_INDEX, self.shape_of(ev)) - 0.5
    }
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
    /// Rate setting on top of the seeded block speed (bass): −1 half, 1
    /// double, of a centre at half the rendered speed ([`BASE_RATE`]).
    pub rate: i8,
    /// Mode in the key's family (0 = plain minor or major).
    pub mode: u8,
    /// Bars of a phrase: the drums fill its last bar (D25).
    pub phrase_bars: u32,
    /// Permute 0..1: how many of the bar's chunks swap places (seeded).
    pub permute: f32,
    /// Splice: the focus per step the player sets for the next render
    /// ([`crate::splice::focus`]; all 1 = off).
    pub focus: [f32; STEPS_PER_BAR],
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
        Self { role, generator, layout, forms, form: 0, pattern_seed, sound_seed, rate: 0, mode: 0, phrase_bars: 8, permute: 0.0, focus: [1.0; STEPS_PER_BAR], params }
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
        out.params = BarParams::default();
        out.voices = self.render_part(&ctx, &pos, &mut out.events, &mut scratch.second, Some(&mut out.params));
        out.letter = pos.letter.index();
        for p in &parts[1..n_parts] {
            scratch.part.clear();
            self.render_part(&ctx, &p.pos, &mut scratch.part, &mut scratch.second, None);
            splice_range(&mut out.events, &scratch.part, p.from, p.to, &mut scratch.second);
        }
    }

    fn render_part(
        &self,
        c: &PartCtx,
        pos: &FormPos,
        out: &mut BarEvents,
        scratch: &mut BarEvents,
        played: Option<&mut BarParams>,
    ) -> u8 {
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
        // Variation widens how far letters and notes move the parameters.
        let reach = variation_reach(names, &params);
        let block_reach = letter_reach(names, &params);
        vary_letter(names, seed, c.lane, pos.letter, pos.second, block_reach, &mut params);
        // The block's own Permute and (bass) Morph. A Permute at 0 stays off.
        let permute_amount = if self.permute > 0.0 {
            (self.permute + block_move("permute", seed, c.lane, pos.letter, pos.second, 1.0)).clamp(LETTER_FLOOR.min(self.permute), 1.0)
        } else {
            0.0
        };
        let block_morph = block_move("morph", seed, c.lane, pos.letter, pos.second, block_reach);
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
            style_seed: self.pattern_seed,
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
            jitter: {
                let mut j = note_jitter(names, reach);
                if let Some(i) = names.iter().position(|n| *n == "length") {
                    j[i] = shape_note(names, &params, self.pattern_seed, c.lane);
                }
                j
            },
            bass_onsets: 0,
            bass_phrase: [0; 4],
            drum_fill: false,
            focus: self.focus,
            focus_shift: focus_shift(names, &self.focus),
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
        permute(out, permute_amount, seed, c.lane, c.bar_in_phrase);
        let mut feel = Feel::of(seed, c.lane, class);
        feel.swing = (feel.swing + knob_swing).min(1.0);
        if let Some(p) = played {
            *p = BarParams {
                values: params,
                jitter: bar_ctx.jitter,
                seed,
                lane: c.lane,
                bar_in_phrase: c.bar_in_phrase,
                root: c.harmony.key.root,
                reach,
                swing: feel.swing,
                morph: if self.role == Role::Drums { 0.0 } else { block_morph },
                permute: permute_amount,
            };
        }
        feel.apply(out, seed, c.lane);
        if self.role == Role::Drums {
            groove(out, knob_swing);
        }
        if self.role != Role::Drums {
            let block = if self.role == Role::Bass { block_rate(seed, c.lane, class) } else { 0 };
            let rate = self.rate + BASE_RATE;
            apply_speed(out, rate + block, c.bar, scratch);
            limit_speed(out, rate, rate + block);
            // Each note moves Morph a touch, as its shape says.
            let played = BarParams { seed, lane: c.lane, bar_in_phrase: c.bar_in_phrase, root: c.harmony.key.root, reach, ..BarParams::default() };
            // Spread is width too: notes above the home root spread out in
            // the stereo field, the sub stays centred.
            let spread = names.iter().position(|n| *n == "spread").map_or(0.0, |i| params[i]);
            let home = bass::home_root(c.harmony.key);
            for e in out.as_mut_slice().iter_mut().filter(|e| e.kind == EventKind::NoteOn) {
                let at = e.start as f64 / u16::MAX as f64 + (block_morph + played.morph_move(e)) as f64 * MORPH_REGION;
                e.start = (at.rem_euclid(1.0) * u16::MAX as f64) as u16;
                let above = ((e.key as i32 - home) as f32 / WIDTH_FULL_SEMIS).clamp(0.0, 1.0);
                let side = patterned(0.5, 0.5, seed, c.lane, c.bar_in_phrase, PAN_INDEX, played.shape_of(e)) * 2.0 - 1.0;
                e.pan = (side.signum() * WIDTH_PAN * spread * above * 127.0).round() as i8;
            }
        }
        splice_thin(out, &self.focus, seed, c.lane, c.bar_in_phrase);
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
// that moves much, at low Variation too, by the seed). The per-note Length
// move, its scale at Variation 0 and 1 and the pattern seed's scale.
const SHAPE_NOTE: f32 = 0.3;
const SHAPE_NOTE_REACH: (f32, f32) = (0.6, 1.3);
const SHAPE_NOTE_SEED: (f32, f32) = (0.6, 1.4);
const S_SHAPE_NOTE: u32 = 0xF54;

/// The per-note Length move of a lane (see [`SHAPE_NOTE`]).
fn shape_note(names: &[&str], params: &[f32; MAX_GEN_PARAMS], pattern_seed: u32, lane: u8) -> f32 {
    let v = names.iter().position(|n| *n == "variation").and_then(|i| params.get(i)).copied().unwrap_or(0.5).clamp(0.0, 1.0);
    let u = rng::unit_f32(rng::hash(pattern_seed, lane, 0, 0, S_SHAPE_NOTE));
    SHAPE_NOTE * (SHAPE_NOTE_REACH.0 + (SHAPE_NOTE_REACH.1 - SHAPE_NOTE_REACH.0) * v) * (SHAPE_NOTE_SEED.0 + (SHAPE_NOTE_SEED.1 - SHAPE_NOTE_SEED.0) * u)
}

// MUSICAL: tune by ear (user, 2026-10-20: blocks move the right-hand knobs
// more, notes only a little). Scale of the letters' moves of [`REACHED`] at
// Variation 0 and 1 (the notes' follow [`VARIATION_REACH`]).
const LETTER_REACH: (f32, f32) = (0.6, 1.4);

/// How far letters move the right-hand knobs' parameters, from Variation.
fn letter_reach(names: &[&str], params: &[f32; MAX_GEN_PARAMS]) -> f32 {
    let v = names.iter().position(|n| *n == "variation").and_then(|i| params.get(i)).copied().unwrap_or(0.5);
    LETTER_REACH.0 + (LETTER_REACH.1 - LETTER_REACH.0) * v.clamp(0.0, 1.0)
}

/// A letter's move of a knob the lane applies itself (`"morph"`,
/// `"permute"`, see [`LETTER_OFFSETS`]), in knob values.
fn block_move(name: &str, seed: u32, lane: u8, letter: Letter, second: bool, reach: f32) -> f32 {
    LETTER_OFFSETS.iter().position(|(n, _)| *n == name).map_or(0.0, |k| reach * letter_offset(seed, lane, letter, k, second))
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

/// Move a letter's parameters around their values ([`LETTER_OFFSETS`]).
fn vary_letter(names: &[&str], seed: u32, lane: u8, letter: Letter, second: bool, reach: f32, params: &mut [f32; MAX_GEN_PARAMS]) {
    for (k, (name, _)) in LETTER_OFFSETS.iter().enumerate() {
        if let Some(i) = names.iter().position(|n| n == name) {
            if let Some(p) = params.get_mut(i) {
                // A parameter above 0 stays above it, so a block never falls silent.
                let floor = if *p > 0.0 { LETTER_FLOOR.min(*p) } else { 0.0 };
                *p = (*p + reach_of(name, reach) * letter_offset(seed, lane, letter, k, second)).clamp(floor, 1.0);
            }
        }
    }
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

/// A note's loop section at `k` times its speed's length (none when it no
/// longer fits its tick bytes).
fn scale_loop(e: &mut LiveEvent, k: f32) {
    let (at, len) = (e.loop_at as f32 * k, e.loop_len as f32 * k);
    if e.loop_len == 0 || at + len > u8::MAX as f32 || len < 1.0 {
        e.loop_len = 0;
    } else {
        (e.loop_at, e.loop_len) = (at as u8, len as u8);
    }
}

/// Play the bar at `2^exponent` speed (clamped to −3..=1). Below 1 each bar
/// plays the next part of the rendered bar stretched over the whole bar, or
/// the busiest part when the next one is far quieter (a bar of near silence
/// otherwise); a note held over the part start comes back on its first
/// tick. 1 plays the bar compressed into each half.
fn apply_speed(out: &mut BarEvents, exponent: i8, bar: u64, scratch: &mut BarEvents) {
    let half = TICKS_PER_BAR / 2;
    match exponent.clamp(-3, 1) {
        0 => return,
        e if e < 0 => {
            let k = 1u16 << (-e) as u16;
            let part = TICKS_PER_BAR / k;
            let from = busy_part(out.as_slice(), k, (bar % k as u64) as u16) * part;
            scratch.clear();
            for e in out.as_slice().iter().filter(|e| {
                e.kind == EventKind::NoteOn && e.gate > 0 && e.tick < from && e.tick + e.gate > from
            }) {
                let mut e = *e;
                e.gate = (e.tick + e.gate - from).min(part) * k;
                e.tick = 0;
                e.flags &= !(flags::TIE | flags::ACCENT);
                // Picked up mid-note: its loop section is behind it.
                e.loop_len = 0;
                e.note_id = e.note_id.wrapping_add(0x2000);
                scratch.push(e);
            }
            for e in out.as_slice().iter().filter(|e| e.tick >= from && e.tick < from + part) {
                let mut e = *e;
                e.tick = (e.tick - from) * k;
                e.gate = e.gate.saturating_mul(k).min(TICKS_PER_BAR - e.tick);
                scale_loop(&mut e, k as f32);
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
                    scale_loop(&mut e, 0.5);
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

// MUSICAL: tune by ear (2026-10-20, the half speed centre: a seed's quiet
// half bar became a near silent bar).
/// A part this much quieter than the busiest one gives way to it.
const QUIET_PART: f32 = 0.35;
/// What a stab sounds for, whatever its gate (it decays).
const STAB_SOUNDS: u16 = crate::TICKS_PER_STEP;

/// Part `next` of the bar's `k` parts, or the busiest one when `next` plays
/// under [`QUIET_PART`] of it (sounding ticks times velocity).
fn busy_part(events: &[LiveEvent], k: u16, next: u16) -> u16 {
    let part = TICKS_PER_BAR / k;
    let score = |p: u16| -> f32 {
        let (lo, hi) = (p * part, (p + 1) * part);
        events
            .iter()
            .filter(|e| e.kind == EventKind::NoteOn && e.gate > 0)
            .map(|e| {
                let gate = if e.has(flags::STAB) { e.gate.min(STAB_SOUNDS) } else { e.gate };
                let covered = (e.tick + gate).min(hi).saturating_sub(e.tick.max(lo));
                covered as f32 * e.vel as f32
            })
            .sum()
    };
    let busiest = (0..k).map(|p| (p, score(p))).fold((next, score(next)), |a, b| if b.1 > a.1 { b } else { a });
    if score(next) < QUIET_PART * busiest.1 { busiest.0 } else { next }
}

/// The fastest notes a speed keeps (user, 2026-10-20): double speed stays
/// at 16ths and half speed at 8ths, instead of everything scaled (`rate`
/// and `exponent` with [`BASE_RATE`] in them: the Normal setting is half). A note
/// closer than that to the previous one goes; the previous one holds over it.
fn limit_speed(out: &mut BarEvents, rate: i8, exponent: i8) {
    // The Rate setting decides; a block at double speed stays at 16ths too.
    let min = if rate >= 1 || exponent >= 1 {
        TICKS_PER_BAR / 16
    } else if rate <= -1 {
        TICKS_PER_BAR / 8
    } else {
        return;
    };
    let events = out.as_mut_slice();
    let mut kept = 0usize;
    let mut last: Option<usize> = None;
    for i in 0..events.len() {
        let e = events[i];
        if e.kind == EventKind::NoteOn {
            if let Some(l) = last {
                if e.tick < events[l].tick + min {
                    let p = &mut events[l];
                    p.gate = p.gate.max((e.tick + e.gate).saturating_sub(p.tick));
                    continue;
                }
            }
            last = Some(kept);
        }
        events[kept] = e;
        kept += 1;
    }
    out.truncate(kept);
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
    fn normal_and_half_speed_stay_at_8ths() {
        for (rate, min) in [(0i8, TICKS_PER_BAR / 8), (-1, TICKS_PER_BAR / 8)] {
            let mut fastest = u16::MAX;
            for seed in 0..40 {
                let mut lane = PatternLane::bass(seed, seed);
                lane.rate = rate;
                for bar in render_all(&lane, 8) {
                    let on: Vec<u16> = bar.iter().filter(|e| e.kind == EventKind::NoteOn).map(|e| e.tick).collect();
                    fastest = on.windows(2).map(|w| w[1] - w[0]).fold(fastest, u16::min);
                }
            }
            assert!(fastest >= min && fastest < u16::MAX, "rate {rate}: fastest {fastest} ticks");
        }
    }

    #[test]
    fn permute_moves_chunks_and_keeps_the_downbeat() {
        let chunk = TICKS_PER_BAR / PERMUTE_CHUNKS as u16;
        for drums in [true, false] {
            let lane = |amount: f32| {
                let mut l = if drums { PatternLane::drums(DrumLayout::standard(), 7, 3) } else { PatternLane::bass(7, 3) };
                l.permute = amount;
                // The bar at its rendered speed: half speed's 8th limit
                // thins moved notes differently.
                l.rate = 1;
                l
            };
            let plain = render_all(&lane(0.0), 8);
            let full = render_all(&lane(1.0), 8);
            let mut moved = 0;
            for (a, b) in plain.iter().zip(&full) {
                assert_eq!(a.len(), b.len(), "every hit stays");
                let first = |v: &[crate::event::LiveEvent]| v.iter().filter(|e| e.tick < chunk).map(|e| (e.tick, e.target, e.key)).collect::<Vec<_>>();
                assert_eq!(first(a), first(b), "the downbeat chunk stays");
                let ticks = |v: &[crate::event::LiveEvent]| v.iter().map(|e| (e.tick, e.target)).collect::<Vec<_>>();
                moved += (ticks(a) != ticks(b)) as usize;
            }
            assert!(moved >= 6, "most bars change: {moved} of 8");
        }
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
