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

use crate::params::{DrumStyle, MeshParams, Mode, PhaseMode, Rate, Scale};
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
    Intensity,
    Variation,
    Swing,
    Shape,
    Morph,
    Spread,
    Slide,
    Gain,
}

impl Knob {
    fn param(self, p: &MeshParams) -> &FloatParam {
        match self {
            Knob::Density => &p.density,
            Knob::Intensity => &p.intensity,
            Knob::Variation => &p.variation,
            Knob::Swing => &p.swing,
            Knob::Shape => &p.shape,
            Knob::Morph => &p.morph,
            Knob::Spread => &p.spread,
            Knob::Slide => &p.slide,
            Knob::Gain => &p.gain,
        }
    }
}

/// One undo step: every parameter's value and the pinned files (D26).
#[derive(Clone)]
struct Snapshot {
    values: Vec<(ParamPtr, f32)>,
    pins: (std::collections::BTreeMap<Slot, FileRef>, Option<FileRef>),
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
    Phase(PhaseMode),
    RollSound,
    RollPattern,
    RollForm,
    LockSound,
    LockPattern,
    SaveFavorite,
    Favorite(Favorite),
    Undo,
    Redo,
    ToggleLibrary,
    LowPower,
    RollSlot(Slot),
    ClearSlot(Slot),
    RollMudpie,
    ShowPool(Pool),
    MoveFile(PathBuf, Pool),
}

pub struct Gui {
    data: PersistentState<EditorData>,
    ctx: IcedNiceContext,
    library: Option<Library>,
    show_library: bool,
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
    started: std::time::Instant,
    samples: Vec<f32>,
}

fn now_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1
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
        let demo_drag = std::env::var_os("MESH_VST_DEMO_DRAG").is_some();
        Self {
            data,
            ctx,
            library: None,
            show_library: false,
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
            started: std::time::Instant::now(),
            samples: Vec::new(),
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
            let mut lib = worker::cache_file().as_deref().map(Library::load).unwrap_or_default();
            lib.folders = worker::library_folders();
            if lib.is_empty() && !lib.folders.is_empty() {
                lib.scan();
            }
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
            Ok(k) => (k.pins.clone(), k.mudpie_pin.clone()),
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
            if (k.pins.clone(), k.mudpie_pin.clone()) != s.pins {
                k.pins = s.pins.0.clone();
                k.mudpie_pin = s.pins.1.clone();
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

    fn set_enum<T: Enum + PartialEq + 'static>(&self, p: &EnumParam<T>, v: T) {
        let setter = self.ctx.nice_context.param_setter();
        setter.begin_set_parameter(p);
        setter.set_parameter(p, v);
        setter.end_set_parameter(p);
    }

    fn roll_seed(&mut self) -> i32 {
        (self.next_rand() % (crate::params::MAX_SEED as u64 + 1)) as i32
    }

    fn pin(&mut self, slot: Option<Slot>, path: &std::path::Path) {
        let lib = self.library();
        let r = FileRef { path: lib.relative(path), hash: lib.entry(path).map_or(0, |e| e.hash) };
        let kit = self.kit();
        if let Ok(mut k) = kit.lock() {
            match slot {
                Some(s) => {
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
                    self.set_int(&params.sound_seed, v);
                }
            }
            Message::RollPattern => {
                if !self.locks().pattern_locked {
                    self.record();
                    let v = self.roll_seed();
                    let params = self.data.params.clone();
                    self.set_int(&params.pattern_seed, v);
                    // A new pattern rolls the form too (D25).
                    let f = 1 + (self.next_rand() % 10) as i32;
                    self.set_int(&params.form, f);
                }
            }
            Message::RollForm => {
                self.record();
                let f = 1 + (self.next_rand() % 10) as i32;
                let params = self.data.params.clone();
                self.set_int(&params.form, f);
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
                let seed = self.next_rand() as u32;
                let pick = self.library().pick(pool, seed, 0, prefer).map(|e| e.path.clone());
                match pick {
                    Some(p) => self.pin(Some(slot), &p),
                    None => self.toast = format!("no {} files in the library", pool.name()),
                }
            }
            Message::ClearSlot(slot) => {
                self.record();
                let kit = self.kit();
                if let Ok(mut k) = kit.lock() {
                    k.pins.remove(&slot);
                }
                self.data.shared.bump_kit();
            }
            Message::RollMudpie => {
                self.record();
                let seed = self.next_rand() as u32;
                let pick = self.library().pick(Pool::Mudpie, seed, 0, None).map(|e| e.path.clone());
                match pick {
                    Some(p) => self.pin(None, &p),
                    None => self.toast = "no mudpies in the library".into(),
                }
            }
            Message::ShowPool(p) => self.pool = p,
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
            for (row, e) in self.slot_env.iter_mut().enumerate() {
                let v = hits[row][step as usize];
                if v > 0 {
                    *e = e.max(v as f32 / 127.0);
                }
            }
        }
        self.last_step = step;
    }

    fn visual_frame(&self, drums: bool) -> crate::visual::Frame {
        let a = self.analyser.as_ref();
        crate::visual::Frame {
            levels: a.map_or([0.0; crate::visual::BINS], |a| a.levels()),
            slots: self.slot_env,
            env: a.map_or(0.0, |a| a.env()),
            punch: a.map_or(0.0, |a| a.punch()),
            time: self.started.elapsed().as_secs_f32(),
            drums,
            aberration: 0.6,
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
        let root = shared.root.load(Ordering::Relaxed);
        let key = if root == 255 { "—".to_string() } else { note_name(root) };
        let bpm = f32::from_bits(shared.bpm.load(Ordering::Relaxed));
        let mode_button = |m: Mode, label: &'static str| {
            let on = p.mode.value() == m;
            button(text(label).font(DATA_FONT).size(13)).on_press(Message::Mode(m)).padding([6, 10]).style(move |_, s| btn(on, s))
        };
        let seed_box = |label: &'static str, v: i32, roll: Message, lock: Message, locked: bool| {
            row![
                column![small(label), text(format!("{v:04}")).font(DATA_FONT).size(16).color(INK)].spacing(2),
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
            text("mesh//vst").font(DISPLAY_FONT).size(17).color(DIM),
            row![mode_button(Mode::Drums, "DRUMS"), mode_button(Mode::Bass, "BASS")],
            column![small("KEY · from note"), row![text(key).font(DATA_FONT).size(16).color(INK), pick_w(Scale::variants_list(), p.scale.value(), Message::Scale, 118.0)].spacing(6).align_y(Alignment::Center)].spacing(2),
            column![small("HOST"), text(format!("{bpm:.1} bpm")).font(DATA_FONT).size(16).color(INK)].spacing(2),
            seed_box("SOUND", p.sound_seed.value(), Message::RollSound, Message::LockSound, locks.sound_locked),
            seed_box("PATTERN", p.pattern_seed.value(), Message::RollPattern, Message::LockPattern, locks.pattern_locked),
            button(text("★").font(DATA_FONT).size(14)).on_press(Message::SaveFavorite).padding([6, 8]).style(|_, s| btn(false, s)),
            pick_list(self.favorites.clone(), None::<Favorite>, Message::Favorite).placeholder("favs").text_size(12).font(DATA_FONT).width(80).style(|_, _| pick_style()),
            Space::new().width(Length::Fill),
            button(text("undo").font(DATA_FONT).size(12)).on_press_maybe((!self.undo.is_empty()).then_some(Message::Undo)).padding([6, 8]).style(|_, s| btn(false, s)),
            button(text("redo").font(DATA_FONT).size(12)).on_press_maybe((!self.redo.is_empty()).then_some(Message::Redo)).padding([6, 8]).style(|_, s| btn(false, s)),
            button(text("LIBRARY").font(DATA_FONT).size(12)).on_press(Message::ToggleLibrary).padding([6, 10]).style(move |_, s| btn(self.show_library, s)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .padding([0, 16]);
        let top = container(top).height(60).center_y(60).width(Length::Fill).style(|_| bar_style());

        // Left: the pattern knobs and settings.
        let mut left = column![small("PATTERN")].spacing(12);
        for k in [Knob::Density, Knob::Intensity, Knob::Variation, Knob::Swing] {
            left = left.push(knob(p, k));
        }
        left = left.push(column![
            small("PHASE"),
            row![
                toggle("CONTINUOUS", p.phase.value() == PhaseMode::Continuous, Message::Phase(PhaseMode::Continuous)),
                toggle("RETRIGGER", p.phase.value() == PhaseMode::Retrigger, Message::Phase(PhaseMode::Retrigger)),
            ]
            .spacing(6)
        ].spacing(6));
        left = left.push(
            column![
                small("FORM"),
                row![
                    text(if p.form.value() == 0 { "one bar".to_string() } else { format!("form {}", p.form.value()) }).font(DATA_FONT).size(13).color(INK).width(Length::Fill),
                    button(text("roll").font(DATA_FONT).size(12)).on_press(Message::RollForm).padding([6, 8]).style(|_, s| btn(false, s)),
                ]
                .align_y(Alignment::Center)
            ]
            .spacing(6),
        );
        left = left.push(knob(p, Knob::Gain));
        let left = container(left).width(260);

        // Right: the mode's knobs.
        let mut right = column![small(if drums { "KIT" } else { "MUDPIE VOICE" })].spacing(12);
        if drums {
            right = right.push(column![small("STYLE"), pick(DrumStyle::variants_list(), p.style.value(), Message::Style)].spacing(6));
            for k in [Knob::Shape, Knob::Morph] {
                right = right.push(knob(p, k));
            }
        } else {
            for k in [Knob::Shape, Knob::Morph, Knob::Spread, Knob::Slide] {
                right = right.push(knob(p, k));
            }
            right = right.push(column![small("RATE"), pick(Rate::variants_list(), p.rate.value(), Message::Rate)].spacing(6));
        }
        right = right.push(Space::new().height(Length::Fill));
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
                let file = pinned.or_else(|| kit.drums.get(&slot)).map_or(if kit.drums.is_empty() { "built-in".to_string() } else { "—".to_string() }, |f| file_name(std::path::Path::new(&f.path)));
                let mut buttons = row![button(text("roll").font(DATA_FONT).size(11)).on_press(Message::RollSlot(slot)).padding([3, 6]).style(|_, s| btn(false, s))].spacing(4);
                if pinned.is_some() {
                    buttons = buttons.push(button(text("seed").font(DATA_FONT).size(11)).on_press(Message::ClearSlot(slot)).padding([3, 6]).style(|_, s| btn(false, s)));
                }
                let card = column![
                    row![small(name), Space::new().width(Length::Fill), small(if pinned.is_some() { "pinned" } else { "seed" })],
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
            row![container(card).padding(10).width(360).height(Length::Fill).style(|_| card_style()), Space::new().width(Length::Fill)].into()
        };
        let toast = text(self.toast.clone()).font(DATA_FONT).size(12).color(DIM);
        let footer = container(column![container(footer).height(Length::Fill), toast].spacing(4)).height(FOOTER_H).padding([8, 24]).width(Length::Fill).style(|_| bar_style());

        let mut screen = column![top, body, footer];
        if self.show_library {
            screen = column![top_spacer(), self.library_panel()];
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

    fn library_panel(&self) -> Element<'_, Message> {
        let Some(lib) = self.library.as_ref() else {
            return text("no library").font(DATA_FONT).into();
        };
        let counts = lib.counts();
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
            files = files.push(
                row![
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

fn small(s: &str) -> iced::widget::Text<'_> {
    text(s).font(DATA_FONT).size(11).color(DIM)
}

fn knob(p: &MeshParams, k: Knob) -> Element<'_, Message> {
    let param = k.param(p);
    let v = param.unmodulated_normalized_value();
    column![
        row![text(param.name()).font(DATA_FONT).size(13).color(DIM), Space::new().width(Length::Fill), text(param.normalized_value_to_string(v, true)).font(DATA_FONT).size(13).color(INK)],
        slider(0.0..=1.0, v, move |x| Message::Knob(k, x)).on_release(Message::KnobReleased(k)).step(0.001_f32).height(18).style(|_, _| slider_style()),
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
enum_display!(Scale, Rate, DrumStyle, PhaseMode, Mode);

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

/// The drum field's subgrids, in the shader's order (top row first), and the
/// slot a file dropped on one loads.
const SUBGRIDS: [(&str, Slot); 8] = [
    ("KICK", Slot::Kick),
    ("SNARE", Slot::Snare),
    ("HATS", Slot::HatClosed),
    ("RIDE", Slot::CymbalA),
    ("SHAKER", Slot::Shaker),
    ("PERC", Slot::Perc),
    ("TOP", Slot::Break),
    ("BREAK", Slot::Break),
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
        for (i, (name, _)) in SUBGRIDS.iter().enumerate() {
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
