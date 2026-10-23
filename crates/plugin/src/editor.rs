//! The editor (design D33, D35): a fixed top bar, the pattern knobs on the
//! left, the mode's knobs on the right, the lane view in the middle and the
//! sound slots at the bottom. Files dropped anywhere load: on a slot card
//! into that slot, elsewhere into the slot of the pool the library sorts
//! them into (Q17). The look follows Mesh live's screen.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use mesh_vst_core::library::{Library, Pool, Tag};
use mesh_vst_core::loader::Slot;
use mesh_vst_core::macros::DRUM_STYLES;
use mesh_vst_core::STEPS_PER_BAR;
use nice_plug::editor::dpi::LogicalSize;
use nice_plug::prelude::*;
use nice_plug_iced::iced::widget::canvas::{self, Frame, Geometry};
use nice_plug_iced::iced::widget::{button, canvas as canvas_widget, column, container, pick_list, row, scrollable, slider, text, Space};
use nice_plug_iced::iced::{
    self, border, event, mouse, window, Alignment, Background, Border, Color, Element, Font, Length, Point, Rectangle, Renderer, Size,
    Subscription, Theme,
};
use nice_plug_iced::{application, create_iced_editor, IcedEditor, IcedEditorState, IcedNiceContext, IcedNiceSettings, PersistentState};
use serde::{Deserialize, Serialize};

use crate::params::{BassSource, DrumStyle, MeshParams, Mode, NewRootMode, Phrase, PhaseMode, Rate, Scale, Tail, UnisonSpread, BEND_RANGES};
use mesh_vst_core::modout::{Source, CHANNELS as MOD_CHANNELS};
use crate::ui::{UiShared, UiState, ROWS};
use crate::worker::{self, FileRef, KitState};

pub const WINDOW: LogicalSize<f32> = LogicalSize::new(1120.0, 680.0);
const MIN_WINDOW: LogicalSize<f32> = LogicalSize::new(960.0, 600.0);

// MUSICAL: Mesh live's screen palette.
const BG: Color = Color::from_rgb(0.043, 0.051, 0.059);
const INK: Color = Color::from_rgb(0.84, 0.87, 0.85);
const DIM: Color = Color::from_rgb(0.60, 0.64, 0.62);
const LINE: Color = Color::from_rgb(0.12, 0.14, 0.15);
const EDGE: Color = Color::from_rgb(0.18, 0.21, 0.22);
const ACCENT: Color = Color::from_rgb(0.50, 0.72, 0.79);
const DRUM_INK: Color = Color::from_rgb(0.92, 0.86, 0.76);
const BASS_INK: Color = Color::from_rgb(0.74, 0.88, 0.92);

pub const DATA_FONT: Font = Font::with_name("Share Tech Mono");
pub const DISPLAY_FONT: Font = Font::with_name("Major Mono Display");

/// Drum slots in the footer, left to right.
const DRUM_SLOTS: [(Slot, &str); 8] = [
    (Slot::Kick, "KICK"),
    (Slot::Snare, "SNARE"),
    (Slot::HatClosed, "HAT"),
    (Slot::HatOpen, "OPEN HAT"),
    (Slot::CymbalA, "CYM A"),
    (Slot::CymbalB, "CYM B"),
    (Slot::Perc, "PERC"),
    (Slot::Break, "BREAK"),
];
/// Footer height, for finding the slot a file is dropped on.
const FOOTER_H: f32 = 118.0;

/// A saved pair of seeds (D22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favorite {
    pub name: String,
    pub drums: bool,
    pub sound_seed: i32,
    pub pattern_seed: i32,
}

impl std::fmt::Display for Favorite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

fn favorites_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mesh-vst").join("favorites.json"))
}

fn load_favorites() -> Vec<Favorite> {
    favorites_file().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_favorites(f: &[Favorite]) {
    if let Some(p) = favorites_file() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(b) = serde_json::to_vec_pretty(f) {
            let _ = std::fs::write(p, b);
        }
    }
}

/// What stays alive between editor opens.
pub struct EditorData {
    pub params: Arc<MeshParams>,
    pub shared: Arc<UiShared>,
}

/// The knob parameters, for one generic slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Knob {
    Density,
    Permute,
    Variation,
    Groove,
    Shape,
    Morph,
    Spread,
    Slide,
    Tone,
    Heat,
    Gain,
    Splice,
    Squash,
    /// D49: replaces Permute on the panel (Permute stays a parameter).
    Contrast,
    /// D53: the bass's neuro chain.
    Neuro,
}

impl Knob {
    fn param(self, p: &MeshParams) -> &FloatParam {
        match self {
            Knob::Density => &p.density,
            Knob::Permute => &p.permute,
            Knob::Variation => &p.variation,
            Knob::Groove => &p.swing,
            Knob::Shape => &p.shape,
            Knob::Morph => &p.morph,
            Knob::Spread => &p.spread,
            Knob::Slide => &p.slide,
            Knob::Tone => &p.tone,
            Knob::Heat => &p.heat,
            Knob::Gain => &p.gain,
            Knob::Splice => &p.splice,
            Knob::Squash => &p.squash,
            Knob::Contrast => &p.contrast,
            Knob::Neuro => &p.neuro,
        }
    }
}

/// One undo step: every parameter's value and the pinned files (D26).
#[derive(Clone)]
struct Snapshot {
    values: Vec<(ParamPtr, f32)>,
    pins: (std::collections::BTreeMap<Slot, FileRef>, Option<FileRef>, std::collections::BTreeMap<Slot, FileRef>),
}

#[derive(Debug, Clone)]
pub enum Message {
    Poll,
    Resized,
    Event(event::Event),
    Knob(Knob, f32),
    KnobReleased(Knob),
    Mode(Mode),
    Style(DrumStyle),
    Scale(Scale),
    Rate(Rate),
    /// Where the bass line comes from (D50).
    BassSource(BassSource),
    Tail(Tail),
    Phase(PhaseMode),
    RollSound,
    RollPattern,
    /// The form (D48): a new phrase seed (pinned phrases stay), pin or
    /// unpin a phrase (A..D), a new fill plan, a new second pass.
    RollPhrases,
    PinPhrase(usize),
    RollFill,
    RollSecond,
    LockSound,
    LockPattern,
    SaveFavorite,
    Favorite(Favorite),
    Undo,
    Redo,
    ToggleLibrary,
    LowPower,
    RollSlot(Slot),
    PinSlot(Slot),
    ClearSlot(Slot),
    RollMudpie,
    ShowPool(Pool),
    ToggleSettings,
    Phrase(Phrase),
    NewRoot(NewRootMode),
    Bend(BendItem),
    ModSource(usize, SourceItem),
    FolderInput(String),
    AddFolder,
    RemoveFolder(usize),
    Rescan,
    UnisonVoices(i32),
    UnisonDetune(f32),
    UnisonDetuneReleased,
    UnisonWidth(f32),
    MonoLows,
    UnisonMode(UnisonSpread),
    Host(HostItem),
    MoveFile(PathBuf, Pool),
    Audition(PathBuf),
}

pub struct Gui {
    data: PersistentState<EditorData>,
    ctx: IcedNiceContext,
    library: Option<Library>,
    show_library: bool,
    show_settings: bool,
    folder_input: String,
    pool: Pool,
    favorites: Vec<Favorite>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    grabbed: Option<Knob>,
    cursor: Point,
    size: Size,
    dragging: bool,
    toast: String,
    rng: u64,
    analyser: Option<crate::visual::Analyser>,
    slot_env: [f32; crate::visual::SLOTS],
    last_step: u32,
    /// The last hit's values (`UiShared::hit`), the hit count seen, and how
    /// bright its markers still are.
    hit: [f32; crate::ui::HIT_VALUES],
    hit_seen: u32,
    hit_glow: f32,
    started: std::time::Instant,
    samples: Vec<f32>,
    /// Kit generation the editor's library copy belongs to.
    seen_generation: u32,
    /// Splice: the shared table (read only here) and the other instances.
    link: Option<crate::link::Link>,
    peers: Vec<crate::link::Peer>,
    peers_at: std::time::Instant,
}

fn now_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1
}

/// A new Splice link id: never 0 (none) or the sidechain.
pub fn fresh_id() -> u64 {
    let mut x = now_seed() ^ (std::process::id() as u64).rotate_left(32);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    if x == 0 || x == crate::link::SIDECHAIN { 1 } else { x }
}

impl Gui {
    fn new(data: PersistentState<EditorData>, ctx: IcedNiceContext) -> Self {
        // Screenshot demos (`nix run .#screenshot -- out.png bass drag`).
        if std::env::var("MESH_VST_DEMO_MODE").is_ok_and(|m| m == "bass") {
            let setter = ctx.nice_context.param_setter();
            setter.begin_set_parameter(&data.params.mode);
            setter.set_parameter(&data.params.mode, Mode::Bass);
            setter.end_set_parameter(&data.params.mode);
        }
        let view = std::env::var("MESH_VST_DEMO_VIEW").unwrap_or_default();
        let demo_drag = view == "drag";
        let link = crate::link::Link::open(data.shared.link_id.load(Ordering::Relaxed)).ok();
        Self {
            data,
            ctx,
            library: None,
            show_library: view == "library",
            show_settings: view == "settings",
            folder_input: String::new(),
            pool: Pool::Kick,
            favorites: load_favorites(),
            undo: Vec::new(),
            redo: Vec::new(),
            grabbed: None,
            cursor: Point::ORIGIN,
            size: Size::new(WINDOW.width, WINDOW.height),
            dragging: demo_drag,
            toast: String::new(),
            rng: now_seed(),
            analyser: None,
            slot_env: [0.0; crate::visual::SLOTS],
            last_step: u32::MAX,
            hit: [f32::NAN; crate::ui::HIT_VALUES],
            hit_seen: 0,
            hit_glow: 0.0,
            started: std::time::Instant::now(),
            samples: Vec::new(),
            seen_generation: 0,
            link,
            peers: Vec::new(),
            peers_at: std::time::Instant::now(),
        }
    }

    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn params(&self) -> &MeshParams {
        &self.data.params
    }

    fn kit(&self) -> Arc<Mutex<KitState>> {
        self.data.params.kit.clone()
    }

    fn library(&mut self) -> &mut Library {
        self.library.get_or_insert_with(|| {
            // The loader scans; the editor reads its cache and never blocks on a scan.
            let mut lib = worker::cache_file().as_deref().map(Library::load).unwrap_or_default();
            lib.folders = worker::library_folders();
            lib
        })
    }

    fn save_library(&mut self) {
        if let (Some(lib), Some(path)) = (self.library.as_ref(), worker::cache_file()) {
            let _ = lib.save(&path);
        }
    }

    fn snapshot(&self) -> Snapshot {
        let values = self
            .params()
            .param_map()
            .into_iter()
            // SAFETY: the pointers come from the live params object.
            .map(|(_, ptr, _)| (ptr, unsafe { ptr.unmodulated_normalized_value() }))
            .collect();
        let kit = self.kit();
        let pins = match kit.lock() {
            Ok(k) => (k.pins.clone(), k.mudpie_pin.clone(), k.rolled.clone()),
            Err(_) => Default::default(),
        };
        Snapshot { values, pins }
    }

    /// Remember the state before a change (mouse release, a button).
    fn record(&mut self) {
        let s = self.snapshot();
        self.undo.push(s);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, s: &Snapshot) {
        let ctx = self.ctx.nice_context.clone();
        for (ptr, v) in &s.values {
            // SAFETY: as above.
            unsafe {
                if (ptr.unmodulated_normalized_value() - v).abs() > f32::EPSILON {
                    ctx.raw_begin_set_parameter(*ptr);
                    ctx.raw_set_parameter_normalized(*ptr, *v);
                    ctx.raw_end_set_parameter(*ptr);
                }
            }
        }
        let kit = self.kit();
        let mut changed = false;
        if let Ok(mut k) = kit.lock() {
            if (k.pins.clone(), k.mudpie_pin.clone(), k.rolled.clone()) != s.pins {
                k.pins = s.pins.0.clone();
                k.mudpie_pin = s.pins.1.clone();
                k.rolled = s.pins.2.clone();
                changed = true;
            }
        }
        if changed {
            self.data.shared.bump_kit();
        }
    }

    fn set_int(&self, p: &IntParam, v: i32) {
        let setter = self.ctx.nice_context.param_setter();
        setter.begin_set_parameter(p);
        setter.set_parameter(p, v);
        setter.end_set_parameter(p);
    }

    fn set_float(&self, p: &FloatParam, v: f32) {
        let setter = self.ctx.nice_context.param_setter();
        setter.begin_set_parameter(p);
        setter.set_parameter(p, v);
        setter.end_set_parameter(p);
    }

    fn set_enum<T: Enum + PartialEq + 'static>(&self, p: &EnumParam<T>, v: T) {
        let setter = self.ctx.nice_context.param_setter();
        setter.begin_set_parameter(p);
        setter.set_parameter(p, v);
        setter.end_set_parameter(p);
    }

    fn roll_seed(&mut self) -> i32 {
        (self.next_rand() % (crate::params::MAX_SEED as u64 + 1)) as i32
    }

    /// A random file of `pool` other than `current` (when the pool has another).
    fn roll_pick(&mut self, pool: Pool, prefer: Option<Tag>, current: Option<FileRef>) -> Option<PathBuf> {
        let mut first = None;
        for _ in 0..32 {
            let seed = self.next_rand() as u32;
            let lib = self.library();
            let Some(e) = lib.pick(pool, seed, 0, prefer) else { break };
            if current.as_ref().is_none_or(|c| c.path != lib.relative(&e.path)) {
                return Some(e.path.clone());
            }
            first.get_or_insert_with(|| e.path.clone());
        }
        first
    }

    fn pin(&mut self, slot: Option<Slot>, path: &std::path::Path) {
        let lib = self.library();
        let r = FileRef { path: lib.relative(path), hash: lib.entry(path).map_or(0, |e| e.hash) };
        let kit = self.kit();
        if let Ok(mut k) = kit.lock() {
            match slot {
                Some(s) => {
                    k.rolled.remove(&s);
                    k.pins.insert(s, r);
                }
                None => k.mudpie_pin = Some(r),
            }
        }
        self.data.shared.bump_kit();
    }

    /// The middle area the visual fills, in window coordinates (from the
    /// layout: 24 px padding, 260 px columns, 28 px gaps, 60 px top bar).
    fn field_rect(&self) -> Option<Rectangle> {
        let x = 24.0 + 260.0 + 28.0;
        let y = 60.0 + 18.0 + 8.0;
        let w = self.size.width - 2.0 * x;
        let h = self.size.height - y - FOOTER_H - 18.0 - 8.0;
        (w > 0.0 && h > 0.0).then_some(Rectangle { x, y, width: w, height: h })
    }

    /// A file dropped at the cursor.
    fn dropped(&mut self, file: PathBuf) {
        self.record();
        let drums = self.params().mode.value() == Mode::Drums;
        let in_footer = self.cursor.y > self.size.height - FOOTER_H;
        let lib = self.library();
        let path = match lib.import(&file) {
            Ok(p) => p,
            Err(e) => {
                self.toast = format!("could not add {}: {e}", file.display());
                return;
            }
        };
        let entry = lib.entry(&path).cloned();
        self.save_library();
        if !drums {
            self.pin(None, &path);
            self.toast = format!("mudpie: {}", file_name(&path));
            return;
        }
        let in_field = self.field_rect().and_then(|r| subgrid_at(field_square(r.size()), Point::new(self.cursor.x - r.x, self.cursor.y - r.y)));
        let slot = if let Some(i) = in_field {
            Some(SUBGRIDS[i].1)
        } else if in_footer {
            let i = ((self.cursor.x / self.size.width) * DRUM_SLOTS.len() as f32).floor() as usize;
            DRUM_SLOTS.get(i).map(|s| s.0)
        } else {
            entry.as_ref().and_then(|e| {
                let pool = self.library.as_ref().map_or(e.detected, |l| l.pool_of(e));
                slot_for(pool, e.tag)
            })
        };
        match slot {
            Some(s) => {
                self.pin(Some(s), &path);
                self.toast = format!("{}: {}", slot_name(s), file_name(&path));
            }
            None => self.toast = format!("{}: unsure, left in the unsorted bin; drop it on a slot", file_name(&path)),
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Poll => self.animate(),
            Message::LowPower => {
                if let Ok(mut u) = self.data.params.ui.lock() {
                    u.low_power = !u.low_power;
                }
            }
            Message::Resized => self.ctx.sync_window_size(),
            Message::Event(e) => match e {
                iced::Event::Mouse(mouse::Event::CursorMoved { position }) => self.cursor = position,
                iced::Event::Window(window::Event::Resized(size)) => self.size = size,
                iced::Event::Window(window::Event::FileHovered(_)) => self.dragging = true,
                iced::Event::Window(window::Event::FilesHoveredLeft) => self.dragging = false,
                iced::Event::Window(window::Event::FileDropped(path)) => {
                    self.dragging = false;
                    self.dropped(path);
                }
                _ => {}
            },
            Message::Knob(k, v) => {
                let first = self.grabbed != Some(k);
                if first {
                    self.record();
                    self.grabbed = Some(k);
                }
                let setter = self.ctx.nice_context.param_setter();
                let p = k.param(&self.data.params);
                if first {
                    setter.begin_set_parameter(p);
                }
                setter.set_parameter_normalized(p, v);
            }
            Message::KnobReleased(k) => {
                let setter = self.ctx.nice_context.param_setter();
                setter.end_set_parameter(k.param(&self.data.params));
                self.grabbed = None;
            }
            Message::Mode(m) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.mode, m);
            }
            Message::Style(s) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.style, s);
            }
            Message::Scale(s) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.scale, s);
            }
            Message::Rate(r) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.rate, r);
            }
            Message::BassSource(s) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.bass_source, s);
            }
            Message::Tail(t) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.tail, t);
            }
            Message::Phase(p) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.phase, p);
            }
            Message::RollSound => {
                if !self.locks().sound_locked {
                    self.record();
                    let v = self.roll_seed();
                    let params = self.data.params.clone();
                    let bass = params.mode.value() == Mode::Bass;
                    if bass {
                        // Bass: the roll keeps the mudpie (its own roll picks
                        // another) and draws a new voice.
                        let kit = self.kit();
                        let keep = kit.lock().ok().and_then(|k| if k.mudpie_pin.is_none() { k.mudpie.clone() } else { None });
                        if let Some(f) = keep {
                            if let Ok(mut k) = kit.lock() {
                                k.mudpie_pin = Some(f);
                            }
                            self.data.shared.bump_kit();
                        }
                    }
                    self.set_int(&params.sound_seed, v);
                    // Drums: the new seed brings its own tail mode.
                    if !bass {
                        self.set_enum(&params.tail, Tail::seeded(v as u32));
                    }
                    // Bass: the new voice shows on the knobs (all but
                    // Permute), to shape by hand.
                    if bass {
                        for p in [&params.density, &params.variation, &params.swing, &params.shape, &params.morph, &params.spread, &params.slide] {
                            // MUSICAL: away from the extremes.
                            let x = 0.1 + 0.8 * (self.next_rand() % 1000) as f32 / 1000.0;
                            self.set_float(p, x);
                        }
                        // Heat mostly warm, rarely crushed.
                        let x = 0.1 + 0.4 * (self.next_rand() % 1000) as f32 / 1000.0;
                        self.set_float(&params.heat, x);
                        // And the Rate: mostly Normal, a quarter each half
                        // and double (user, 2026-10-20).
                        let rate = match self.next_rand() % 4 {
                            0 => Rate::Half,
                            1 => Rate::Double,
                            _ => Rate::Normal,
                        };
                        self.set_enum(&params.rate, rate);
                    }
                }
            }
            Message::RollPattern => {
                if !self.locks().pattern_locked {
                    self.record();
                    let v = self.roll_seed();
                    let params = self.data.params.clone();
                    self.set_int(&params.pattern_seed, v);
                    // And the form: phrases (pinned ones stay), fills and
                    // the second pass (user, 2026-10-21).
                    for p in [&params.phrase_seed, &params.fill_seed, &params.second_seed] {
                        let v = self.roll_seed();
                        self.set_int(p, v);
                    }
                }
            }
            Message::RollPhrases | Message::RollFill | Message::RollSecond => {
                self.record();
                let v = self.roll_seed();
                let params = self.data.params.clone();
                let p = match message {
                    Message::RollPhrases => &params.phrase_seed,
                    Message::RollFill => &params.fill_seed,
                    _ => &params.second_seed,
                };
                self.set_int(p, v);
            }
            Message::PinPhrase(i) => {
                let letter = mesh_vst_core::form::Letter::ALL[i.min(3)];
                let seed = self.form_settings().phrase_of(letter);
                if let Ok(mut u) = self.data.params.ui.lock() {
                    u.phrase_pins[i.min(3)] = if u.phrase_pins[i.min(3)].is_some() { None } else { Some(seed) };
                    self.data.shared.load_phrase_pins(&u);
                }
            }
            Message::LockSound => self.toggle_lock(true),
            Message::LockPattern => self.toggle_lock(false),
            Message::SaveFavorite => {
                let p = self.params();
                let drums = p.mode.value() == Mode::Drums;
                let fav = Favorite {
                    name: format!("{} {:04}/{:04}", if drums { "drums" } else { "bass" }, p.sound_seed.value(), p.pattern_seed.value()),
                    drums,
                    sound_seed: p.sound_seed.value(),
                    pattern_seed: p.pattern_seed.value(),
                };
                if !self.favorites.contains(&fav) {
                    self.favorites.push(fav);
                    save_favorites(&self.favorites);
                    self.toast = "favorite saved".into();
                }
            }
            Message::Favorite(f) => {
                self.record();
                let params = self.data.params.clone();
                self.set_int(&params.sound_seed, f.sound_seed);
                self.set_int(&params.pattern_seed, f.pattern_seed);
            }
            Message::Undo => {
                if let Some(s) = self.undo.pop() {
                    let now = self.snapshot();
                    self.redo.push(now);
                    self.restore(&s);
                }
            }
            Message::Redo => {
                if let Some(s) = self.redo.pop() {
                    let now = self.snapshot();
                    self.undo.push(now);
                    self.restore(&s);
                }
            }
            Message::ToggleLibrary => {
                self.show_library = !self.show_library;
                if self.show_library {
                    self.library();
                }
            }
            Message::RollSlot(slot) => {
                self.record();
                let (pool, prefer) = slot.source();
                let current = self.kit().lock().ok().and_then(|k| k.pins.get(&slot).or_else(|| k.rolled.get(&slot)).or_else(|| k.drums.get(&slot)).cloned());
                let pick = self.roll_pick(pool, prefer, current);
                match pick {
                    // A pinned slot keeps its pin on the new file; any other
                    // plays it until the next sound roll.
                    Some(p) => {
                        let lib = self.library();
                        let r = FileRef { path: lib.relative(&p), hash: lib.entry(&p).map_or(0, |e| e.hash) };
                        let kit = self.kit();
                        if let Ok(mut k) = kit.lock() {
                            if k.pins.contains_key(&slot) {
                                k.pins.insert(slot, r);
                            } else {
                                k.rolled.insert(slot, r);
                            }
                        }
                        self.data.shared.bump_kit();
                    }
                    None => self.toast = format!("no {} files in the library", pool.name()),
                }
            }
            Message::PinSlot(slot) => {
                self.record();
                let kit = self.kit();
                if let Ok(mut k) = kit.lock() {
                    if let Some(f) = k.pins.remove(&slot) {
                        // Unpinned: it plays on until the next sound roll.
                        k.rolled.insert(slot, f);
                    } else if let Some(f) = k.rolled.remove(&slot).or_else(|| k.drums.get(&slot).cloned()) {
                        k.pins.insert(slot, f);
                    }
                }
                self.data.shared.bump_kit();
            }
            Message::ClearSlot(slot) => {
                self.record();
                let kit = self.kit();
                if let Ok(mut k) = kit.lock() {
                    k.pins.remove(&slot);
                    k.rolled.remove(&slot);
                }
                self.data.shared.bump_kit();
            }
            Message::RollMudpie => {
                self.record();
                let current = self.kit().lock().ok().and_then(|k| k.mudpie_pin.clone().or_else(|| k.mudpie.clone()));
                let pick = self.roll_pick(Pool::Mudpie, None, current);
                match pick {
                    Some(p) => self.pin(None, &p),
                    None => self.toast = "no mudpies in the library".into(),
                }
            }
            Message::ToggleSettings => {
                self.show_settings = !self.show_settings;
                self.show_library = false;
            }
            Message::Phrase(v) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.phrase, v);
            }
            Message::NewRoot(v) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.new_root, v);
            }
            Message::Bend(b) => {
                self.record();
                let params = self.data.params.clone();
                self.set_int(&params.bend_range, b.0);
            }
            Message::ModSource(ch, s) => {
                let drums = self.params().mode.value() == Mode::Drums;
                if let Ok(mut u) = self.data.params.ui.lock() {
                    let (map, default) = if drums {
                        (&mut u.mod_drums, mesh_vst_core::modout::DEFAULT_DRUMS)
                    } else {
                        (&mut u.mod_bass, mesh_vst_core::modout::DEFAULT_BASS)
                    };
                    if map.len() != MOD_CHANNELS {
                        *map = default.to_vec();
                    }
                    map[ch] = s.0;
                    self.data.shared.load_mod_map(&u);
                }
            }
            Message::FolderInput(s) => self.folder_input = s,
            Message::AddFolder => {
                let p = PathBuf::from(self.folder_input.trim());
                if p.is_dir() {
                    let mut folders = worker::library_folders();
                    if !folders.contains(&p) {
                        folders.push(p);
                        self.save_folders(folders);
                    }
                    self.folder_input.clear();
                } else {
                    self.toast = format!("not a folder: {}", p.display());
                }
            }
            Message::RemoveFolder(i) => {
                let mut folders = worker::library_folders();
                if i < folders.len() {
                    folders.remove(i);
                    self.save_folders(folders);
                }
            }
            Message::Rescan => {
                self.scan_in_background();
                self.toast = "scanning the library…".into();
            }
            Message::UnisonVoices(v) => {
                let params = self.data.params.clone();
                self.set_int(&params.unison_voices, v);
            }
            Message::UnisonDetune(v) => {
                let params = self.data.params.clone();
                let setter = self.ctx.nice_context.param_setter();
                setter.begin_set_parameter(&params.unison_detune);
                setter.set_parameter(&params.unison_detune, v);
                setter.end_set_parameter(&params.unison_detune);
            }
            Message::UnisonDetuneReleased => self.record(),
            Message::UnisonWidth(v) => {
                let params = self.data.params.clone();
                self.set_float(&params.unison_width, v);
            }
            Message::MonoLows => {
                self.record();
                let params = self.data.params.clone();
                let on = params.mono_lows.value();
                let setter = self.ctx.nice_context.param_setter();
                setter.begin_set_parameter(&params.mono_lows);
                setter.set_parameter(&params.mono_lows, !on);
                setter.end_set_parameter(&params.mono_lows);
            }
            Message::Host(h) => {
                let id = match h {
                    HostItem::None => 0,
                    HostItem::Sidechain => crate::link::SIDECHAIN,
                    HostItem::Peer(id, _) => id,
                };
                if let Ok(mut u) = self.data.params.ui.lock() {
                    u.host = (id != 0).then_some(id);
                }
                self.data.shared.host.store(id, Ordering::Relaxed);
            }
            Message::UnisonMode(m) => {
                self.record();
                let params = self.data.params.clone();
                self.set_enum(&params.unison_mode, m);
            }
            Message::ShowPool(p) => self.pool = p,
            Message::Audition(path) => {
                if let Ok(mut a) = self.data.shared.audition.lock() {
                    *a = Some(path);
                }
            }
            Message::MoveFile(path, pool) => {
                self.library().set_override(&path, Some(pool));
                self.save_library();
                self.data.shared.bump_kit();
            }
        }
    }

    /// Advance the visual: read the output, decay the slot envelopes, light
    /// the slots that hit on a new step.
    fn animate(&mut self) {
        // Splice: the other instances, once a second.
        if self.peers_at.elapsed().as_millis() > 1000 {
            self.peers_at = std::time::Instant::now();
            if let Some(l) = self.link.as_ref() {
                self.peers = l.peers();
            }
        }
        // Read the cache again when a scan finished (the kit generation moved)
        // or while the panel waits for the loader's first scan.
        let generation = self.data.shared.kit_generation.load(Ordering::Relaxed);
        let waiting = self.library.as_ref().is_some_and(|l| l.is_empty() && !l.folders.is_empty()) && self.started.elapsed().as_millis() % 1000 < 40;
        if generation != self.seen_generation || waiting {
            self.seen_generation = generation;
            self.library = None;
        }
        if (self.show_library || self.show_settings) && self.library.is_none() {
            self.library();
        }
        let shared = self.data.shared.clone();
        let sr = shared.sample_rate.load(Ordering::Relaxed) as f32;
        if self.analyser.as_ref().is_none_or(|a| a.sample_rate() != sr) {
            self.analyser = Some(crate::visual::Analyser::new(sr));
        }
        self.samples.clear();
        if let Ok(mut rx) = shared.audio.lock() {
            if let Some(rx) = rx.as_mut() {
                while let Ok(s) = rx.pop() {
                    self.samples.push(s);
                }
            }
        }
        if let Some(a) = self.analyser.as_mut() {
            a.push(&self.samples);
        }
        for e in &mut self.slot_env {
            *e *= 0.82;
        }
        let step = shared.step.load(Ordering::Relaxed);
        if step != self.last_step && (step as usize) < STEPS_PER_BAR {
            let hits = shared.hits();
            for (e, (_, _, row)) in self.slot_env.iter_mut().zip(SUBGRIDS) {
                let v = hits[row][step as usize];
                if v > 0 {
                    *e = e.max(v as f32 / 127.0);
                }
            }
        }
        let (count, hit) = shared.hit();
        if count != self.hit_seen {
            self.hit_seen = count;
            self.hit = hit;
            self.hit_glow = 1.0;
        } else {
            self.hit_glow *= HIT_FADE;
        }
        self.last_step = step;
    }

    /// Where the last hit puts knob `k`'s marker and how bright it is; `None`
    /// once it faded or while the hit sits on the knob.
    fn hit_marker(&self, k: Knob) -> Option<(f32, f32)> {
        let i = k as usize;
        let v = *self.hit.get(i)?;
        if !v.is_finite() || self.hit_glow < 0.05 {
            return None;
        }
        let param = k.param(&self.data.params);
        // Gain: the knob's gain times the hit's level.
        let pos = if k == Knob::Gain { param.preview_normalized(param.value() * v) } else { v };
        ((pos - param.unmodulated_normalized_value()).abs() > 0.004).then_some((pos.clamp(0.0, 1.0), self.hit_glow))
    }

    /// Splice: per bin, how much this lane and its host are both loud there
    /// (the host's band levels spread over the bins). Zero without a host.
    fn overlap(&self, own: &[f32; crate::visual::BINS]) -> [f32; crate::visual::BINS] {
        let host = self.data.shared.host.load(Ordering::Relaxed);
        if host == 0 || host == crate::link::SIDECHAIN {
            return [0.0; crate::visual::BINS];
        }
        let bands: [f32; crate::link::BANDS] = std::array::from_fn(|b| self.data.shared.host_bands[b].load(Ordering::Relaxed) as f32 / 255.0);
        std::array::from_fn(|i| {
            let x = (i as f32 + 0.5) / crate::visual::BINS as f32 * crate::link::BANDS as f32 - 0.5;
            let lo = (x.floor().max(0.0) as usize).min(crate::link::BANDS - 1);
            let hi = (lo + 1).min(crate::link::BANDS - 1);
            let t = (x - lo as f32).clamp(0.0, 1.0);
            let theirs = bands[lo] * (1.0 - t) + bands[hi] * t;
            // MUSICAL: both above about half their range counts as a clash.
            let both = own[i].min(theirs);
            ((both - 0.35) / 0.35).clamp(0.0, 1.0)
        })
    }

    fn visual_frame(&self, drums: bool) -> crate::visual::Frame {
        let a = self.analyser.as_ref();
        let levels = a.map_or([0.0; crate::visual::BINS], |a| a.levels());
        crate::visual::Frame {
            overlap: self.overlap(&levels),
            levels,
            slots: self.slot_env,
            env: a.map_or(0.0, |a| a.env()),
            punch: a.map_or(0.0, |a| a.punch()),
            time: self.started.elapsed().as_secs_f32(),
            drums,
            aberration: 0.6,
        }
    }

    /// Write the library folders to the settings file and reload the library.
    fn save_folders(&mut self, folders: Vec<PathBuf>) {
        if let Err(e) = worker::save_settings(&worker::Settings { library_folders: folders }) {
            self.toast = format!("could not save the settings: {e}");
            return;
        }
        self.scan_in_background();
    }

    /// Scan the library folders on a thread, save the cache, then make the
    /// loader and the editor read it again.
    fn scan_in_background(&mut self) {
        self.library = None;
        let shared = self.data.shared.clone();
        let _ = std::thread::Builder::new().name("mesh-vst-scan".into()).spawn(move || {
            let cache = worker::cache_file();
            let mut lib = cache.as_deref().map(Library::load).unwrap_or_default();
            lib.folders = worker::library_folders();
            lib.scan();
            if let Some(c) = &cache {
                let _ = lib.save(c);
            }
            shared.bump_kit();
        });
    }

    /// The form's seeds as the lane plays them.
    fn form_settings(&self) -> mesh_vst_core::macros::FormSettings {
        let p = self.params();
        mesh_vst_core::macros::FormSettings {
            phrase_seed: p.phrase_seed.value() as u32,
            fill_seed: p.fill_seed.value() as u32,
            second_seed: p.second_seed.value() as u32,
            pins: self.data.shared.phrase_pins(),
        }
    }

    fn locks(&self) -> UiState {
        self.data.params.ui.lock().map(|u| u.clone()).unwrap_or_default()
    }

    fn toggle_lock(&mut self, sound: bool) {
        if let Ok(mut u) = self.data.params.ui.lock() {
            if sound {
                u.sound_locked = !u.sound_locked;
            } else {
                u.pattern_locked = !u.pattern_locked;
            }
        }
    }

    fn theme(&self) -> Option<Theme> {
        Some(Theme::Dark)
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            iced::poll_events().map(|_| Message::Poll),
            iced::window_resized().map(|_| Message::Resized),
            event::listen().map(Message::Event),
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let p = self.params();
        let drums = p.mode.value() == Mode::Drums;
        let locks = self.locks();
        let shared = &self.data.shared;

        // Top bar.
        let split = self.analyser.as_ref().map_or(0.0, |a| 1.5 * a.env() + 6.0 * a.punch());
        let root = shared.root.load(Ordering::Relaxed);
        let key = if root == 255 { "—".to_string() } else { note_name(root) };
        let bpm = f32::from_bits(shared.bpm.load(Ordering::Relaxed));
        let mode_button = |m: Mode, label: &'static str| {
            let on = p.mode.value() == m;
            button(text(label).font(DATA_FONT).size(13)).on_press(Message::Mode(m)).padding([6, 10]).style(move |_, s| btn(on, s))
        };
        let seed_box = |label: &'static str, v: i32, roll: Message, lock: Message, locked: bool| {
            row![
                column![small(label), ghost(format!("{v:04}"), 16.0, DATA_FONT, INK, split)].spacing(2),
                button(text("roll").font(DATA_FONT).size(12)).on_press(roll).padding([6, 8]).style(|_, s| btn(false, s)),
                button(text(if locked { "locked" } else { "lock" }).font(DATA_FONT).size(12))
                    .on_press(lock)
                    .padding([6, 8])
                    .style(move |_, s| btn(locked, s)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        };
        let top = row![
            ghost("mesh//vst".into(), 15.0, DISPLAY_FONT, DIM, split),
            row![mode_button(Mode::Drums, "DRUMS"), mode_button(Mode::Bass, "BASS")],
            column![small("KEY · from note"), row![ghost(key, 16.0, DATA_FONT, INK, split), pick_w(Scale::variants_list(), p.scale.value(), Message::Scale, 104.0)].spacing(6).align_y(Alignment::Center)].spacing(2),
            column![small("HOST"), text(format!("{bpm:.1} bpm")).font(DATA_FONT).size(16).color(INK)].spacing(2),
            seed_box("SOUND", p.sound_seed.value(), Message::RollSound, Message::LockSound, locks.sound_locked),
            seed_box("PATTERN", p.pattern_seed.value(), Message::RollPattern, Message::LockPattern, locks.pattern_locked),
            button(text("★").font(DATA_FONT).size(14)).on_press(Message::SaveFavorite).padding([6, 8]).style(|_, s| btn(false, s)),
            pick_list(self.favorites.clone(), None::<Favorite>, Message::Favorite).placeholder("favs").text_size(12).font(DATA_FONT).width(70).style(|_, _| pick_style()),
            Space::new().width(Length::Fill),
            button(text("undo").font(DATA_FONT).size(12)).on_press_maybe((!self.undo.is_empty()).then_some(Message::Undo)).padding([6, 8]).style(|_, s| btn(false, s)),
            button(text("redo").font(DATA_FONT).size(12)).on_press_maybe((!self.redo.is_empty()).then_some(Message::Redo)).padding([6, 8]).style(|_, s| btn(false, s)),
            button(text("LIBRARY").font(DATA_FONT).size(12)).on_press(Message::ToggleLibrary).padding([6, 10]).style(move |_, s| btn(self.show_library, s)),
            button(text("SET").font(DATA_FONT).size(12)).on_press(Message::ToggleSettings).padding([6, 10]).style(move |_, s| btn(self.show_settings, s)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([0, 12]);
        let top = container(top).height(60).center_y(60).width(Length::Fill).style(|_| bar_style());

        // Left: the pattern knobs and settings.
        let mut left = column![small("PATTERN")].spacing(12);
        for k in [Knob::Density, Knob::Contrast, Knob::Variation, Knob::Groove] {
            left = left.push(knob(p, k, self.hit_marker(k)));
        }
        // The lane's last stage.
        left = left.push(small("OUTPUT"));
        left = left.push(knob(p, Knob::Squash, None));
        left = left.push(column![
            small("PHASE"),
            row![
                toggle("CONTINUOUS", p.phase.value() == PhaseMode::Continuous, Message::Phase(PhaseMode::Continuous)),
                toggle("RETRIGGER", p.phase.value() == PhaseMode::Retrigger, Message::Phase(PhaseMode::Retrigger)),
            ]
            .spacing(6)
        ].spacing(6));
        // The form (D48): its seeds, the phrase pins, what it plays.
        let form = self.form_settings();
        let seed = |label: &'static str, v: i32, roll: Message| {
            row![
                text(label).font(DATA_FONT).size(11).color(DIM).width(34),
                text(format!("{v:04}")).font(DATA_FONT).size(13).color(INK).width(38),
                button(text("roll").font(DATA_FONT).size(11)).on_press(roll).padding([3, 6]).style(|_, s| btn(false, s)),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
        };
        // The phrases with their pins, then the fill plan and the second pass.
        let mut phrases = row![seed("phr", p.phrase_seed.value(), Message::RollPhrases), Space::new().width(Length::Fill)].spacing(4).align_y(Alignment::Center);
        for (i, l) in ["A", "B", "C", "D"].into_iter().enumerate() {
            let on = form.pins[i].is_some();
            phrases = phrases.push(button(text(l).font(DATA_FONT).size(11)).on_press(Message::PinPhrase(i)).padding([3, 6]).style(move |_, s| btn(on, s)));
        }
        let (plan, recipes) = form_summary(&form, drums, [p.variation.value(), p.shape.value(), p.contrast.value()]);
        left = left.push(
            column![
                text(format!("FORM · {plan}, then {recipes}")).font(DATA_FONT).size(11).color(DIM),
                phrases,
                row![seed("fill", p.fill_seed.value(), Message::RollFill), Space::new().width(Length::Fill), seed("2nd", p.second_seed.value(), Message::RollSecond)].align_y(Alignment::Center),
            ]
            .spacing(6),
        );


        let left = container(left).width(260);

        // Right: the mode's knobs.
        let mut right = column![small(if drums { "KIT" } else { "MUDPIE VOICE" })].spacing(12);
        if drums {
            right = right.push(column![small("STYLE"), pick(DrumStyle::variants_list(), p.style.value(), Message::Style)].spacing(6));
            right = right.push(knob(p, Knob::Shape, self.hit_marker(Knob::Shape)));
            // The release tail's method (Shape above its middle).
            let tail = p.tail.value();
            right = right.push(
                row![
                    toggle("VOC", tail == Tail::Vocoder, Message::Tail(Tail::Vocoder)),
                    toggle("SPEC", tail == Tail::Spectral, Message::Tail(Tail::Spectral)),
                    toggle("NOISE", tail == Tail::Noise, Message::Tail(Tail::Noise)),
                ]
                .spacing(6),
            );
            for k in [Knob::Morph, Knob::Tone, Knob::Heat] {
                right = right.push(knob(p, k, self.hit_marker(k)));
            }
        } else {
            for k in [Knob::Shape, Knob::Morph, Knob::Spread, Knob::Slide, Knob::Heat, Knob::Neuro] {
                right = right.push(knob(p, k, self.hit_marker(k)));
            }
            let source = p.bass_source.value();
            right = right.push(
                row![
                    column![small("RATE"), pick(Rate::variants_list(), p.rate.value(), Message::Rate)].spacing(6),
                    column![
                        small("SOURCE"),
                        row![
                            toggle("CLIPS", source == BassSource::Clips, Message::BassSource(BassSource::Clips)),
                            toggle("GEN", source == BassSource::Generated, Message::BassSource(BassSource::Generated)),
                        ]
                        .spacing(6),
                    ]
                    .spacing(6),
                ]
                .spacing(16),
            );
        }
        right = right.push(Space::new().height(Length::Fill));
        right = right.push(self.splice_view(p));
        right = right.push(column![small("VISUAL"), row![toggle("FIELD", !locks.low_power, Message::LowPower), toggle("STEPS", locks.low_power, Message::LowPower)].spacing(6)].spacing(6));
        let right = container(right).width(260).height(Length::Fill);

        // Middle: the point field, or the step view in low-power mode.
        let view: Element<Message> = if locks.low_power {
            canvas_widget(StepView {
                hits: shared.hits(),
                step: shared.step.load(Ordering::Relaxed),
                drums,
                letter: shared.letter.load(Ordering::Relaxed),
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            iced_widget::shader(crate::visual::Field { frame: self.visual_frame(drums) }).width(Length::Fill).height(Length::Fill).into()
        };
        let view: Element<Message> = if self.dragging && drums {
            iced::widget::stack![view, canvas_widget(SubgridLabels { hover: None }).width(Length::Fill).height(Length::Fill)].into()
        } else {
            view
        };
        let middle = container(view).width(Length::Fill).height(Length::Fill).padding([8, 0]);

        let body = row![left, middle, right].spacing(28).padding([18, 24]).height(Length::Fill);

        // Footer: the sound slots.
        let kit_lock = self.kit();
        let kit = match kit_lock.lock() {
            Ok(k) => k.clone(),
            Err(_) => KitState::default(),
        };
        let footer: Element<Message> = if drums {
            let mut r = row![].spacing(8);
            for (slot, name) in DRUM_SLOTS {
                let pinned = kit.pins.get(&slot);
                let is_pinned = pinned.is_some();
                let own = pinned.or_else(|| kit.rolled.get(&slot));
                let file = own.or_else(|| kit.drums.get(&slot)).map_or(if kit.drums.is_empty() { "built-in".to_string() } else { "—".to_string() }, |f| file_name(std::path::Path::new(&f.path)));
                let mut buttons = row![button(text("roll").font(DATA_FONT).size(11)).on_press(Message::RollSlot(slot)).padding([3, 6]).style(|_, s| btn(false, s))].spacing(4);
                if own.is_some() {
                    buttons = buttons.push(button(text("seed").font(DATA_FONT).size(11)).on_press(Message::ClearSlot(slot)).padding([3, 6]).style(|_, s| btn(false, s)));
                }
                let card = column![
                    row![
                        small(name),
                        Space::new().width(Length::Fill),
                        // Pin: the sound roll leaves this slot alone.
                        button(text("pin").font(DATA_FONT).size(10)).on_press(Message::PinSlot(slot)).padding([1, 4]).style(move |_, s| btn(is_pinned, s)),
                    ]
                    .align_y(Alignment::Center),
                    text(file).font(DATA_FONT).size(12).color(INK).wrapping(iced::widget::text::Wrapping::None),
                    Space::new().height(Length::Fill),
                    buttons,
                ]
                .spacing(4);
                r = r.push(container(card).padding(8).width(Length::Fill).height(Length::Fill).style(|_| card_style()));
            }
            r.into()
        } else {
            let file = kit.mudpie_pin.as_ref().or(kit.mudpie.as_ref()).map_or("built-in mudpie".to_string(), |f| file_name(std::path::Path::new(&f.path)));
            let card = column![
                row![small("MUDPIE · drop a file anywhere"), Space::new().width(Length::Fill), small(if kit.mudpie_pin.is_some() { "pinned" } else { "seed" })],
                text(file).font(DATA_FONT).size(14).color(INK),
                button(text("roll").font(DATA_FONT).size(11)).on_press(Message::RollMudpie).padding([3, 6]).style(|_, s| btn(false, s)),
            ]
            .spacing(6);
            // Unison limits and mode (D38), bottom right.
            let voices = p.unison_voices.value();
            let detune = p.unison_detune.value();
            let width = p.unison_width.value();
            let unison = column![
                small("UNISON · the seed picks each note within these"),
                row![
                    text(format!("voices {voices}")).font(DATA_FONT).size(12).color(INK).width(80),
                    slider(1..=8, voices, Message::UnisonVoices).height(16).style(|_, _| slider_style()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text(format!("detune {detune:.0} ct")).font(DATA_FONT).size(12).color(INK).width(80),
                    slider(0.0..=60.0, detune, Message::UnisonDetune).on_release(Message::UnisonDetuneReleased).step(0.5_f32).height(16).style(|_, _| slider_style()),
                    pick(UnisonSpread::variants_list(), p.unison_mode.value(), Message::UnisonMode),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text(format!("width {:.0} %", width * 100.0)).font(DATA_FONT).size(12).color(INK).width(80),
                    slider(0.0..=1.0, width, Message::UnisonWidth).on_release(Message::UnisonDetuneReleased).step(0.01_f32).height(16).style(|_, _| slider_style()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(4);
            row![
                container(card).padding(10).width(360).height(Length::Fill).style(|_| card_style()),
                Space::new().width(Length::Fill),
                container(unison).padding(10).width(420).height(Length::Fill).style(|_| card_style()),
            ]
            .into()
        };
        let toast = text(self.toast.clone()).font(DATA_FONT).size(12).color(DIM);
        let footer = container(column![container(footer).height(Length::Fill), toast].spacing(4)).height(FOOTER_H).padding([8, 24]).width(Length::Fill).style(|_| bar_style());

        let mut screen = column![top, body, footer];
        if self.show_library {
            screen = column![top_spacer(), self.library_panel()];
        } else if self.show_settings {
            screen = column![top_spacer(), self.settings_panel()];
        }
        let base = container(screen).width(Length::Fill).height(Length::Fill).style(|_| container::Style {
            background: Some(Background::Color(BG)),
            text_color: Some(INK),
            ..Default::default()
        });
        if self.dragging && !drums {
            let hint = "drop mudpie";
            iced::widget::stack![
                base,
                container(column![text(hint).font(DISPLAY_FONT).size(24).color(INK), small("files outside the library are copied to mesh vst imports")].spacing(8).align_x(Alignment::Center))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(Background::Color(Color { a: 0.82, ..BG })),
                        border: Border { color: INK, width: 1.0, radius: 0.0.into() },
                        ..Default::default()
                    })
            ]
            .into()
        } else {
            base.into()
        }
    }

    fn settings_panel(&self) -> Element<'_, Message> {
        let p = self.params();
        let drums = p.mode.value() == Mode::Drums;
        let bends: Vec<BendItem> = (0..=BEND_RANGES.len() as i32).map(BendItem).collect();
        let pattern = column![
            small("PATTERN"),
            row![text("Phrase").font(DATA_FONT).size(13).color(DIM).width(120), pick(Phrase::variants_list(), p.phrase.value(), Message::Phrase)].align_y(Alignment::Center),
            row![text("New root").font(DATA_FONT).size(13).color(DIM).width(120), pick(NewRootMode::variants_list(), p.new_root.value(), Message::NewRoot)].align_y(Alignment::Center),
            row![
                text("Bend range").font(DATA_FONT).size(13).color(DIM).width(120),
                pick_list(bends, Some(BendItem(p.bend_range.value())), Message::Bend).font(DATA_FONT).text_size(12).style(|_, _| pick_style()),
            ]
            .align_y(Alignment::Center),
            row![
                text("Mono lows").font(DATA_FONT).size(13).color(DIM).width(120),
                toggle(if p.mono_lows.value() { "ON · below 80 Hz" } else { "OFF" }, p.mono_lows.value(), Message::MonoLows),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(10);

        let map = |c: usize| self.data.shared.mod_map[if drums { 1 } else { 0 }][c].load(Ordering::Relaxed) as usize;
        let mut mods = column![small(if drums { "MOD OUT · drums" } else { "MOD OUT · bass" })].spacing(4);
        for c in 0..MOD_CHANNELS {
            let current = Source::ALL[map(c) % Source::ALL.len()];
            mods = mods.push(
                row![
                    text(format!("Mod {}", c + 1)).font(DATA_FONT).size(12).color(DIM).width(70),
                    pick_list(Source::ALL.map(SourceItem).to_vec(), Some(SourceItem(current)), move |s| Message::ModSource(c, s))
                        .font(DATA_FONT)
                        .text_size(12)
                        .width(150)
                        .style(|_, _| pick_style()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }

        let mut folders = column![small("LIBRARY FOLDERS")].spacing(6);
        if std::env::var_os(worker::LIBRARY_ENV).is_some() {
            folders = folders.push(small("set by MESH_VST_LIBRARY: edits here apply without it"));
        }
        for (i, f) in worker::library_folders().iter().enumerate() {
            folders = folders.push(
                row![
                    text(f.display().to_string()).font(DATA_FONT).size(12).color(INK).width(Length::Fill),
                    button(text("remove").font(DATA_FONT).size(11)).on_press(Message::RemoveFolder(i)).padding([3, 6]).style(|_, s| btn(false, s)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        folders = folders.push(
            row![
                iced::widget::text_input("/path/to/samples", &self.folder_input)
                    .on_input(Message::FolderInput)
                    .on_submit(Message::AddFolder)
                    .font(DATA_FONT)
                    .size(12),
                button(text("add").font(DATA_FONT).size(11)).on_press(Message::AddFolder).padding([3, 6]).style(|_, s| btn(false, s)),
            ]
            .spacing(8),
        );
        folders = folders.push(button(text("rescan").font(DATA_FONT).size(11)).on_press(Message::Rescan).padding([3, 6]).style(|_, s| btn(false, s)));

        column![
            row![text("SETTINGS").font(DISPLAY_FONT).size(18).color(DIM), Space::new().width(Length::Fill), button(text("close").font(DATA_FONT).size(12)).on_press(Message::ToggleSettings).style(|_, s| btn(false, s))].padding([12, 24]),
            row![container(pattern).width(300), scrollable(mods).width(260).height(Length::Fill), container(folders).width(Length::Fill)].spacing(32).padding([12, 24]),
        ]
        .into()
    }

    fn library_panel(&self) -> Element<'_, Message> {
        let header = row![
            text("LIBRARY").font(DISPLAY_FONT).size(18).color(DIM),
            Space::new().width(Length::Fill),
            button(text("close").font(DATA_FONT).size(12)).on_press(Message::ToggleLibrary).style(|_, s| btn(false, s)),
        ]
        .padding([12, 24]);
        let Some(lib) = self.library.as_ref() else {
            return column![header, text("loading the library…").font(DATA_FONT).size(13).color(DIM)].into();
        };
        let counts = lib.counts();
        if lib.is_empty() && !lib.folders.is_empty() {
            return column![header, text("scanning the library folders… (the first scan reads every file once)").font(DATA_FONT).size(13).color(DIM)].padding([0, 24]).into();
        }
        let mut pools = column![small("POOLS")].spacing(6);
        for pool in Pool::ALL {
            let n = counts.get(&pool).copied().unwrap_or(0);
            let on = self.pool == pool;
            pools = pools.push(
                button(text(format!("{:9} {n}", pool.name())).font(DATA_FONT).size(13))
                    .on_press(Message::ShowPool(pool))
                    .width(Length::Fill)
                    .style(move |_, s| btn(on, s)),
            );
        }
        let folders = lib.folders.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join("\n");
        pools = pools.push(small("FOLDERS"));
        pools = pools.push(text(if folders.is_empty() { "none: set ~/.config/mesh-vst/settings.json".into() } else { folders }).font(DATA_FONT).size(11).color(DIM));
        let mut files = column![small("FILES · move a file to another pool for good")].spacing(4);
        for e in lib.pool(self.pool).into_iter().take(400) {
            let path = e.path.clone();
            let play = e.path.clone();
            files = files.push(
                row![
                    button(text("play").font(DATA_FONT).size(11)).on_press(Message::Audition(play)).padding([2, 6]).style(|_, s| btn(false, s)),
                    text(lib.relative(&e.path)).font(DATA_FONT).size(12).color(INK).width(Length::Fill),
                    pick_list(Pool::ALL.map(PoolItem).to_vec(), Some(PoolItem(lib.pool_of(e))), move |p: PoolItem| Message::MoveFile(path.clone(), p.0))
                        .text_size(11)
                        .font(DATA_FONT)
                        .width(110)
                        .style(|_, _| pick_style()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let panel = row![container(pools).width(240), scrollable(files).height(Length::Fill).width(Length::Fill)].spacing(24).padding([18, 24]);
        column![
            row![text("LIBRARY").font(DISPLAY_FONT).size(18).color(DIM), Space::new().width(Length::Fill), button(text("close").font(DATA_FONT).size(12)).on_press(Message::ToggleLibrary).style(|_, s| btn(false, s))].padding([12, 24]),
            panel
        ]
        .into()
    }
}

fn top_spacer() -> Element<'static, Message> {
    Space::new().height(0).into()
}

/// A pool in a pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolItem(Pool);

impl std::fmt::Display for PoolItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.name())
    }
}

/// The slot a pool's file goes to when it is dropped outside the slots.
fn slot_for(pool: Pool, tag: Tag) -> Option<Slot> {
    Some(match pool {
        Pool::Kick => Slot::Kick,
        Pool::Snare => Slot::Snare,
        Pool::Hats if tag == Tag::OpenHat => Slot::HatOpen,
        Pool::Hats => Slot::HatClosed,
        Pool::Cymbal if tag == Tag::Crash => Slot::CymbalB,
        Pool::Cymbal if tag == Tag::Shaker => Slot::Shaker,
        Pool::Cymbal => Slot::CymbalA,
        Pool::Perc => Slot::Perc,
        Pool::Break => Slot::Break,
        Pool::Mudpie | Pool::Unsorted => return None,
    })
}

fn slot_name(s: Slot) -> &'static str {
    DRUM_SLOTS.iter().find(|d| d.0 == s).map_or("shaker", |d| d.1)
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().to_string())
}

fn note_name(key: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[key as usize % 12], key as i32 / 12 - 1)
}

/// Text with the red/blue ghost split of Mesh live's screen, `split` px apart
/// (D33: the top bar moves with the hits).
fn ghost<'a>(s: String, size: f32, font: Font, color: Color, split: f32) -> Element<'a, Message> {
    if split < 0.2 {
        return text(s).font(font).size(size).color(color).into();
    }
    let t = |c: Color| text(s.clone()).font(font).size(size).color(c);
    iced::widget::stack![
        container(t(Color::from_rgba(0.95, 0.35, 0.32, 0.5))).padding(iced::Padding { left: split, ..Default::default() }),
        container(t(Color::from_rgba(0.35, 0.55, 0.98, 0.5))).padding(iced::Padding { right: split, ..Default::default() }),
        container(t(color)).padding(iced::Padding { left: split * 0.5, ..Default::default() }),
    ]
    .into()
}

fn small(s: &str) -> iced::widget::Text<'_> {
    text(s).font(DATA_FONT).size(11).color(DIM)
}

/// Brightness a hit marker keeps per redraw (about a third of a second).
const HIT_FADE: f32 = 0.9;
/// Width of the slider handle (see [`slider_style`]).
const HANDLE_W: f32 = 10.0;

/// A knob slider; `hit` (position, brightness) marks where the last hit
/// played it.
fn knob(p: &MeshParams, k: Knob, hit: Option<(f32, f32)>) -> Element<'_, Message> {
    let param = k.param(p);
    let v = param.unmodulated_normalized_value();
    let s = slider(0.0..=1.0, v, move |x| Message::Knob(k, x)).on_release(Message::KnobReleased(k)).step(0.001_f32).height(18).style(|_, _| slider_style());
    // Always the same widgets, the marker hidden when no hit moved the knob:
    // a slider that comes and goes in a stack loses its drag.
    let marker = match hit {
        Some((at, glow)) => HitMarker { at, glow, shown: true },
        None => HitMarker { at: 0.0, glow: 0.0, shown: false },
    };
    let s: Element<Message> = iced::widget::stack![s, canvas_widget(marker).width(Length::Fill).height(18)].into();
    column![
        row![text(param.name()).font(DATA_FONT).size(13).color(DIM), Space::new().width(Length::Fill), text(param.normalized_value_to_string(v, true)).font(DATA_FONT).size(13).color(INK)],
        s,
    ]
    .spacing(4)
    .into()
}

fn toggle(label: &'static str, on: bool, msg: Message) -> Element<'static, Message> {
    button(text(label).font(DATA_FONT).size(12)).on_press(msg).padding([6, 8]).style(move |_, s| btn(on, s)).into()
}

fn pick_w<T>(options: Vec<T>, selected: T, f: impl Fn(T) -> Message + 'static, width: f32) -> Element<'static, Message>
where
    T: ToString + PartialEq + Clone + 'static,
{
    pick_list(options.into_iter().map(Labeled).collect::<Vec<_>>(), Some(Labeled(selected)), move |l: Labeled<T>| f(l.0))
        .font(DATA_FONT)
        .text_size(12)
        .width(width)
        .style(|_, _| pick_style())
        .into()
}

fn pick<T>(options: Vec<T>, selected: T, f: impl Fn(T) -> Message + 'static) -> Element<'static, Message>
where
    T: ToString + PartialEq + Clone + 'static,
{
    pick_list(options.into_iter().map(Labeled).collect::<Vec<_>>(), Some(Labeled(selected)), move |l: Labeled<T>| f(l.0))
        .font(DATA_FONT)
        .text_size(12)
        .style(|_, _| pick_style())
        .into()
}

/// A param enum shown by its display name.
#[derive(Clone, PartialEq)]
struct Labeled<T>(T);

impl<T: ToString> std::fmt::Display for Labeled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.to_string())
    }
}

/// Every variant of a param enum, in order.
pub trait VariantsList: Sized {
    fn variants_list() -> Vec<Self>;
}

impl<T: Enum> VariantsList for T {
    fn variants_list() -> Vec<Self> {
        (0..T::variants().len()).map(T::from_index).collect()
    }
}

macro_rules! enum_display {
    ($($t:ty),*) => {$(
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(<$t as Enum>::variants()[self.clone().to_index()])
            }
        }
    )*};
}
enum_display!(Scale, Rate, DrumStyle, PhaseMode, Mode, Phrase, NewRootMode, UnisonSpread);

/// A mod-out source in a pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceItem(pub Source);

impl std::fmt::Display for SourceItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.name())
    }
}

/// A bend range choice: 0 = the seed's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BendItem(pub i32);

impl std::fmt::Display for BendItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 <= 0 {
            f.write_str("seed")
        } else {
            write!(f, "{} st", BEND_RANGES[(self.0 as usize - 1).min(BEND_RANGES.len() - 1)])
        }
    }
}

fn btn(on: bool, status: button::Status) -> button::Style {
    let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if on { INK } else { Color::TRANSPARENT })),
        text_color: if on { BG } else if status == button::Status::Disabled { EDGE } else { INK },
        border: Border { color: if on { INK } else if hover { DIM } else { EDGE }, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn pick_style() -> pick_list::Style {
    pick_list::Style {
        text_color: INK,
        placeholder_color: DIM,
        handle_color: DIM,
        background: Background::Color(BG),
        border: Border { color: EDGE, width: 1.0, radius: 0.0.into() },
    }
}

fn slider_style() -> slider::Style {
    slider::Style {
        rail: slider::Rail { backgrounds: (Background::Color(ACCENT), Background::Color(LINE)), width: 4.0, border: border::rounded(0) },
        handle: slider::Handle {
            shape: slider::HandleShape::Rectangle { width: 10, border_radius: 0.0.into() },
            background: Background::Color(INK),
            border_width: 0.0,
            border_color: INK,
        },
    }
}

/// The last hit's mark on a knob slider: a soft ink line where the hit
/// played it, split red and blue at its edges like the field's points.
struct HitMarker {
    at: f32,
    glow: f32,
    shown: bool,
}

impl canvas::Program<Message> for HitMarker {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if !self.shown {
            return vec![frame.into_geometry()];
        }
        // The handle's centre travels HANDLE_W / 2 .. width − HANDLE_W / 2.
        let x = HANDLE_W / 2.0 + self.at * (bounds.width - HANDLE_W);
        let a = 0.1 + 0.25 * self.glow;
        let h = bounds.height - 6.0;
        let bar = |frame: &mut Frame, dx: f32, w: f32, color: Color| {
            frame.fill_rectangle(Point::new(x + dx - w / 2.0, 3.0), Size::new(w, h), color);
        };
        // A blur: wide and faint to narrow and brighter.
        for (w, k) in [(7.0, 0.15), (4.0, 0.3), (1.5, 0.6)] {
            bar(&mut frame, 0.0, w, Color { a: a * k, ..INK });
        }
        bar(&mut frame, -1.5, 1.0, Color::from_rgba(0.95, 0.35, 0.32, a * 0.35));
        bar(&mut frame, 1.5, 1.0, Color::from_rgba(0.35, 0.55, 0.98, a * 0.35));
        vec![frame.into_geometry()]
    }
}

fn bar_style() -> container::Style {
    container::Style { background: Some(Background::Color(BG)), border: Border { color: LINE, width: 1.0, radius: 0.0.into() }, ..Default::default() }
}

fn card_style() -> container::Style {
    container::Style { border: Border { color: EDGE, width: 1.0, radius: 0.0.into() }, ..Default::default() }
}

/// The playing bar as a step grid (the M5 visuals replace it).
struct StepView {
    hits: [[u8; STEPS_PER_BAR]; ROWS],
    step: u32,
    drums: bool,
    letter: u8,
}

/// The drum field's subgrids, in the shader's order (top row first): the
/// slot a file dropped on one loads, and the hit row (of [`DRUM_ROWS`]) that
/// moves it. Hats and the other tops above, kick, snare, break, perc below.
const SUBGRIDS: [(&str, Slot, usize); 8] = [
    ("HATS", Slot::HatClosed, 2),
    ("RIDE", Slot::CymbalA, 3),
    ("SHAKER", Slot::Shaker, 4),
    ("TOP", Slot::Break, 6),
    ("KICK", Slot::Kick, 0),
    ("SNARE", Slot::Snare, 1),
    ("BREAK", Slot::Break, 7),
    ("PERC", Slot::Perc, 5),
];

/// The field's square inside `bounds` (as the shader lays it out).
fn field_square(bounds: Size) -> Rectangle {
    let side = bounds.width.min(bounds.height) - 28.0;
    Rectangle { x: (bounds.width - side) / 2.0, y: (bounds.height - side) / 2.0, width: side, height: side }
}

/// Subgrid index (0..8) of a point inside the square.
fn subgrid_at(sq: Rectangle, p: Point) -> Option<usize> {
    if !sq.contains(p) {
        return None;
    }
    let sx = (((p.x - sq.x) / sq.width) * 4.0).floor().clamp(0.0, 3.0) as usize;
    let sy = if p.y - sq.y < sq.height / 2.0 { 0 } else { 1 };
    Some(sx + 4 * sy)
}

/// Labels over the drum field while a file is dragged (Q17).
struct SubgridLabels {
    hover: Option<Point>,
}

impl canvas::Program<Message> for SubgridLabels {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, cursor: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let sq = field_square(bounds.size());
        let hover = cursor.position_in(bounds).or(self.hover).and_then(|p| subgrid_at(sq, p));
        let (w, h) = (sq.width / 4.0, sq.height / 2.0);
        for (i, (name, _, _)) in SUBGRIDS.iter().enumerate() {
            let x = sq.x + (i % 4) as f32 * w;
            let y = sq.y + (i / 4) as f32 * h;
            let on = hover == Some(i);
            frame.fill_rectangle(Point::new(x + 1.0, y + 1.0), Size::new(w - 2.0, h - 2.0), Color { a: if on { 0.55 } else { 0.35 }, ..BG });
            frame.stroke(
                &canvas::Path::rectangle(Point::new(x + 1.0, y + 1.0), Size::new(w - 2.0, h - 2.0)),
                canvas::Stroke::default().with_color(if on { INK } else { EDGE }).with_width(1.0),
            );
            frame.fill_text(canvas::Text {
                content: (*name).into(),
                position: Point::new(x + 8.0, y + 8.0),
                color: if on { INK } else { DIM },
                size: 13.0.into(),
                font: DATA_FONT,
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

const DRUM_ROWS: [&str; 8] = ["KICK", "SNARE", "HATS", "RIDE", "SHAKER", "PERC", "TOP", "BREAK"];

impl canvas::Program<Message> for StepView {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let rows = if self.drums { ROWS } else { 1 };
        let label_w = 54.0;
        let cw = (bounds.width - label_w) / STEPS_PER_BAR as f32;
        let rh = (bounds.height - 22.0) / rows as f32;
        let ink = if self.drums { DRUM_INK } else { BASS_INK };
        for r in 0..rows {
            let y = r as f32 * rh;
            if self.drums {
                frame.fill_text(canvas::Text {
                    content: DRUM_ROWS[r].into(),
                    position: Point::new(0.0, y + rh / 2.0 - 6.0),
                    color: DIM,
                    size: 11.0.into(),
                    font: DATA_FONT,
                    ..Default::default()
                });
            }
            for s in 0..STEPS_PER_BAR {
                let x = label_w + s as f32 * cw;
                let v = self.hits[r][s];
                let current = self.step as usize == s;
                if current {
                    frame.fill_rectangle(Point::new(x, y + 1.0), Size::new(cw - 2.0, rh - 2.0), Color { a: 0.35, ..LINE });
                }
                let (w, h) = ((cw - 6.0).max(2.0), (rh - 8.0).max(2.0));
                if v == 0 {
                    frame.fill_rectangle(Point::new(x + 2.0, y + rh / 2.0 - 1.0), Size::new(w, 2.0), LINE);
                } else {
                    let a = 0.35 + 0.65 * v as f32 / 127.0;
                    let hh = h * (0.3 + 0.7 * v as f32 / 127.0);
                    frame.fill_rectangle(Point::new(x + 2.0, y + 4.0 + (h - hh)), Size::new(w, hh), Color { a: if current { 1.0 } else { a }, ..ink });
                }
            }
        }
        let letter = ["A", "B", "C", "D"][self.letter as usize % 4];
        frame.fill_text(canvas::Text {
            content: format!("letter {letter} · step {}", if self.step == u32::MAX { "—".into() } else { (self.step + 1).to_string() }),
            position: Point::new(label_w, bounds.height - 16.0),
            color: DIM,
            size: 11.0.into(),
            font: DATA_FONT,
            ..Default::default()
        });
        vec![frame.into_geometry()]
    }
}

/// Build the editor.
pub fn create(editor_state: Arc<IcedEditorState>, data: EditorData) -> Option<IcedEditor> {
    let notifier = data.shared.notifier.clone();
    create_iced_editor(
        editor_state,
        data,
        notifier,
        IcedNiceSettings::new().with_resize_hint(ResizeHint::resizable().with_min_logical_size(MIN_WINDOW)),
        |state, ctx| {
            Ok(application(state, ctx, Gui::new, Gui::update, Gui::view)
                .font(include_bytes!("../assets/fonts/ShareTechMono-Regular.ttf").as_slice())
                .font(include_bytes!("../assets/fonts/MajorMonoDisplay-Regular.ttf").as_slice())
                .default_font(DATA_FONT)
                .subscription(Gui::subscription)
                .theme(Gui::theme)
                .run())
        },
    )
}

#[allow(dead_code)]
fn style_names() -> &'static [&'static str] {
    &DRUM_STYLES
}

/// A Splice host in the list: none, the sidechain input, or another instance.
#[derive(Debug, Clone, PartialEq)]
pub enum HostItem {
    None,
    Sidechain,
    Peer(u64, String),
}

impl std::fmt::Display for HostItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostItem::None => write!(f, "no host"),
            HostItem::Sidechain => write!(f, "sidechain"),
            HostItem::Peer(_, label) => write!(f, "{label}"),
        }
    }
}

impl Gui {
    /// Splice (D42): the host to follow and how (stacked .. gaps).
    fn splice_view<'a>(&'a self, p: &'a MeshParams) -> Element<'a, Message> {
        let host = self.data.shared.host.load(Ordering::Relaxed);
        let me = self.data.shared.link_id.load(Ordering::Relaxed);
        let mut items = vec![HostItem::None, HostItem::Sidechain];
        // Not the ones following this lane: two never follow each other.
        items.extend(self.peers.iter().filter(|q| q.follows != me).map(|q| HostItem::Peer(q.id, q.label.clone())));
        let selected = match host {
            0 => HostItem::None,
            crate::link::SIDECHAIN => HostItem::Sidechain,
            id => items.iter().find(|i| matches!(i, HostItem::Peer(p, _) if *p == id)).cloned().unwrap_or(HostItem::Peer(id, "host gone".into())),
        };
        let list = pick_list(items, Some(selected), Message::Host).font(DATA_FONT).text_size(12).width(Length::Fill).style(|_, _| pick_style());
        column![
            small("SPLICE · arrange around another lane"),
            list,
            knob(p, Knob::Splice, None),
            row![small("stack"), Space::new().width(Length::Fill), small("gaps")],
        ]
        .spacing(6)
        .into()
    }
}

/// The plan and the second pass's recipes of the form `form` builds at the
/// knob positions (the seeds' centres aside), for the form panel.
fn form_summary(form: &mesh_vst_core::macros::FormSettings, drums: bool, [variation, shape, contrast]: [f32; 3]) -> (&'static str, String) {
    let mut b = mesh_vst_core::arrange::FormBuilder::new();
    let mut f = mesh_vst_core::form::Form::generated();
    b.build(&form.spec(drums, variation, shape, contrast), &mut f);
    let recipes: Vec<&str> = b.recipe_names().into_iter().flatten().collect();
    (b.plan_name(), recipes.join(" + "))
}
