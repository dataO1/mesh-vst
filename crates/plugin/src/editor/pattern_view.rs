//! The pattern editor (D60): a slot's whole source as a piano roll (bass)
//! or a hit grid (drums), the window that plays bracketed in it. Notes snap
//! to the grid: a straight, dotted or triplet division of the bar.
//!
//! Click an empty cell to add a note (drag to set a bass note's length),
//! drag a note to move it, its right edge to lengthen it, right-click to
//! delete it; drag a bracket to move the window's start or end. A change
//! is sent when the mouse lets go.

use nice_plug_iced::iced::alignment::Vertical;
use nice_plug_iced::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use nice_plug_iced::iced::widget::{text::Alignment as TextAlign, Action};
use nice_plug_iced::iced::{mouse, Color, Point, Rectangle, Renderer, Size, Theme};

use mesh_vst_core::generator::layout::DrumVoice;
use mesh_vst_core::slots::{UserNote, UserPattern, MIN_WINDOW};
use mesh_vst_core::TICKS_PER_BAR;

use super::{Message, ACCENT, BG, DATA_FONT, DIM, EDGE, INK, LINE};

const BAR: f32 = TICKS_PER_BAR as f32;
const BEAT: f32 = BAR / 4.0;
/// The label column and the ruler, in px.
const KEY_W: f32 = 54.0;
const RULER_H: f32 = 18.0;
/// How near (px) the pointer must be to grab an edge or a bracket.
const GRAB: f32 = 5.0;
/// Rows a bass roll shows at least (two octaves).
const MIN_ROWS: i16 = 25;

/// The grid's division: the snap's, dotted or as a triplet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Feel {
    #[default]
    Straight,
    Dotted,
    Triplet,
}

/// Snap `1/snap` of a bar in `feel`, in ticks.
pub fn unit(snap: u8, feel: Feel) -> f32 {
    let u = BAR / snap.max(1) as f32;
    match feel {
        Feel::Straight => u,
        Feel::Dotted => u * 1.5,
        Feel::Triplet => u * 2.0 / 3.0,
    }
}

/// A change to the pattern, by the index of a note in its sorted notes.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Add(UserNote),
    Move { index: usize, tick: u32, row: i16 },
    Resize { index: usize, len: u32 },
    Delete { index: usize },
    Window { start: u32, len: u32 },
}

impl Edit {
    /// Apply to `p` (normalized after).
    pub fn apply(&self, p: &mut UserPattern) {
        match *self {
            Edit::Add(n) => p.notes.push(n),
            Edit::Move { index, tick, row } => {
                if let Some(n) = p.notes.get_mut(index) {
                    n.tick = tick;
                    n.row = row;
                }
            }
            Edit::Resize { index, len } => {
                if let Some(n) = p.notes.get_mut(index) {
                    n.len = len;
                }
            }
            Edit::Delete { index } => {
                if index < p.notes.len() {
                    p.notes.remove(index);
                }
            }
            Edit::Window { start, len } => {
                p.start = start;
                p.len = len;
            }
        }
        p.normalize();
    }
}

/// One row: its value (bass semitones over the home root, drum target) and
/// its label.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub value: i16,
    pub label: String,
    /// A root (bass) or a voice's first row (drums): drawn stronger.
    pub mark: bool,
    /// A black key's row.
    pub dark: bool,
}

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The rows of `p`: the bass two octaves and more around its notes, top
/// down, named in key `root` (a pitch class); the drums the targets its
/// hits play, by voice.
pub fn rows(p: &UserPattern, drums: bool, root: u8, home: i32) -> Vec<Row> {
    if drums {
        let mut targets: Vec<u8> = p.notes.iter().map(|n| n.row.clamp(0, 255) as u8).collect();
        targets.sort_unstable();
        targets.dedup();
        let mut out = Vec::new();
        for v in DrumVoice::ALL {
            let mine: Vec<u8> = targets.iter().copied().filter(|t| DrumVoice::of_target(*t) == Some(v)).collect();
            for (i, t) in mine.iter().enumerate() {
                let label = if mine.len() > 1 { format!("{} {}", short(v), i + 1) } else { short(v).to_string() };
                out.push(Row { value: *t as i16, label, mark: i == 0, dark: false });
            }
        }
        return out;
    }
    let lo = p.notes.iter().map(|n| n.row).min().unwrap_or(0).min(0) - 2;
    let hi = p.notes.iter().map(|n| n.row).max().unwrap_or(12).max(12) + 2;
    let hi = hi.max(lo + MIN_ROWS - 1);
    (lo..=hi)
        .rev()
        .map(|r| {
            let key = home + r as i32;
            let pc = key.rem_euclid(12) as usize;
            Row { value: r, label: format!("{}{}", NAMES[pc], key.div_euclid(12) - 1), mark: pc as u8 == root % 12, dark: NAMES[pc].len() > 1 }
        })
        .collect()
}

fn short(v: DrumVoice) -> &'static str {
    match v {
        DrumVoice::Kick => "KICK",
        DrumVoice::Snare => "SNARE",
        DrumVoice::Hats => "HATS",
        DrumVoice::Ride => "RIDE",
        DrumVoice::Shaker => "SHAKE",
        DrumVoice::Perc => "PERC",
        DrumVoice::Top => "TOP",
        DrumVoice::Break => "BREAK",
    }
}

/// The editor's view of one pattern.
pub struct PatternView {
    pub pattern: UserPattern,
    pub rows: Vec<Row>,
    pub drums: bool,
    /// Grid division in ticks.
    pub unit: f32,
    /// Where the playing note is in the source, if this pattern plays.
    pub playhead: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// A note moved: its index and the grab's offset (ticks, rows).
    Move { index: usize, tick: u32, row: usize, from: (f32, usize) },
    Resize { index: usize, len: u32 },
    /// A new note being drawn.
    New { tick: u32, row: usize, len: u32 },
    Start(u32),
    End(u32),
}

#[derive(Default)]
pub struct ViewState {
    drag: Option<Drag>,
    hover: Option<Point>,
}

struct Layout {
    grid: Rectangle,
    rh: f32,
    ticks: f32,
}

impl PatternView {
    fn layout(&self, bounds: Rectangle) -> Layout {
        let grid = Rectangle { x: KEY_W, y: RULER_H, width: (bounds.width - KEY_W).max(1.0), height: (bounds.height - RULER_H).max(1.0) };
        let rh = grid.height / self.rows.len().max(1) as f32;
        Layout { grid, rh, ticks: self.pattern.ticks.max(1) as f32 }
    }

    fn x_of(l: &Layout, tick: f32) -> f32 {
        l.grid.x + tick / l.ticks * l.grid.width
    }

    fn tick_at(l: &Layout, x: f32) -> f32 {
        ((x - l.grid.x) / l.grid.width * l.ticks).clamp(0.0, l.ticks)
    }

    fn row_at(&self, l: &Layout, y: f32) -> Option<usize> {
        let r = ((y - l.grid.y) / l.rh).floor();
        (r >= 0.0 && (r as usize) < self.rows.len()).then_some(r as usize)
    }

    fn row_index(&self, value: i16) -> Option<usize> {
        self.rows.iter().position(|r| r.value == value)
    }

    /// Snap `tick` to the grid (`floor`: the cell it is in).
    fn snap(&self, tick: f32, floor: bool) -> u32 {
        let cells = tick / self.unit;
        let c = if floor { cells.floor() } else { cells.round() };
        (c * self.unit).round().clamp(0.0, l_max(self.pattern.ticks)) as u32
    }

    /// The width a note draws: its length, or a cell for a one-shot hit.
    fn note_ticks(&self, n: &UserNote) -> f32 {
        if self.drums || n.len == 0 { (self.unit * 0.8).max(6.0) } else { n.len as f32 }
    }

    fn note_rect(&self, l: &Layout, n: &UserNote) -> Option<Rectangle> {
        let r = self.row_index(n.row)?;
        let x = Self::x_of(l, n.tick as f32);
        let w = (Self::x_of(l, n.tick as f32 + self.note_ticks(n)) - x).max(2.0);
        Some(Rectangle { x, y: l.grid.y + r as f32 * l.rh + 1.0, width: w - 1.0, height: (l.rh - 2.0).max(1.0) })
    }

    /// The notes as they draw with the drag in progress.
    fn shown(&self, drag: Option<Drag>) -> Vec<(UserNote, bool)> {
        let mut out: Vec<(UserNote, bool)> = self.pattern.notes.iter().map(|n| (*n, false)).collect();
        match drag {
            Some(Drag::Move { index, tick, row, .. }) => {
                if let (Some(n), Some(r)) = (out.get_mut(index), self.rows.get(row)) {
                    n.0.tick = tick;
                    n.0.row = r.value;
                    n.1 = true;
                }
            }
            Some(Drag::Resize { index, len }) => {
                if let Some(n) = out.get_mut(index) {
                    n.0.len = len;
                    n.1 = true;
                }
            }
            Some(Drag::New { tick, row, len }) => {
                if let Some(r) = self.rows.get(row) {
                    out.push((UserNote { tick, len, row: r.value, vel: 100, start: 0, choke: 0 }, true));
                }
            }
            _ => {}
        }
        out
    }

    fn window(&self, drag: Option<Drag>) -> (u32, u32) {
        let (s, l) = (self.pattern.start, self.pattern.len);
        match drag {
            Some(Drag::Start(start)) => (start, (s + l).saturating_sub(start)),
            Some(Drag::End(end)) => (s, end.saturating_sub(s)),
            _ => (s, l),
        }
    }

    /// A new hit's start and choke: as the row's other hits play.
    fn template(&self, row: i16) -> (u16, u8) {
        self.pattern.notes.iter().find(|n| n.row == row).map_or((0, 0), |n| (n.start, n.choke))
    }
}

fn l_max(ticks: u32) -> f32 {
    ticks.saturating_sub(1) as f32
}

impl canvas::Program<Message> for PatternView {
    type State = ViewState;

    fn update(&self, state: &mut ViewState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        let l = self.layout(bounds);
        let local = cursor.position_in(bounds);
        state.hover = local;
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let p = local?;
                let (ws, wl) = (self.pattern.start as f32, self.pattern.len as f32);
                let near = |t: f32| (p.x - Self::x_of(&l, t)).abs() <= GRAB;
                let on_ruler = p.y < l.grid.y;
                // The window's brackets: on the ruler, or their lines.
                if *button == mouse::Button::Left && (on_ruler || near(ws) || near(ws + wl)) && (near(ws) || near(ws + wl)) {
                    state.drag = Some(if near(ws + wl) && !near(ws) { Drag::End(self.pattern.start + self.pattern.len) } else { Drag::Start(self.pattern.start) });
                    return Some(Action::capture());
                }
                if on_ruler || p.x < l.grid.x {
                    return None;
                }
                let hit = self.pattern.notes.iter().enumerate().rev().find(|(_, n)| self.note_rect(&l, n).is_some_and(|r| r.contains(p)));
                match (button, hit) {
                    (mouse::Button::Right, Some((index, _))) => return Some(Action::publish(Message::PatternEdit(Edit::Delete { index })).and_capture()),
                    (mouse::Button::Left, Some((index, n))) => {
                        let r = self.note_rect(&l, n)?;
                        if !self.drums && p.x >= r.x + r.width - GRAB {
                            state.drag = Some(Drag::Resize { index, len: n.len });
                        } else {
                            let row = self.row_index(n.row)?;
                            state.drag = Some(Drag::Move { index, tick: n.tick, row, from: (Self::tick_at(&l, p.x) - n.tick as f32, row) });
                        }
                    }
                    (mouse::Button::Left, None) => {
                        let row = self.row_at(&l, p.y)?;
                        let tick = self.snap(Self::tick_at(&l, p.x), true);
                        state.drag = Some(Drag::New { tick, row, len: if self.drums { 0 } else { self.unit.round() as u32 } });
                    }
                    _ => return None,
                }
                Some(Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let p = cursor.position_in(bounds).or_else(|| cursor.position().map(|q| Point::new(q.x - bounds.x, q.y - bounds.y)))?;
                let drag = state.drag?;
                let t = Self::tick_at(&l, p.x);
                state.drag = Some(match drag {
                    Drag::Move { index, from, .. } => {
                        let row = self.row_at(&l, p.y.clamp(l.grid.y, l.grid.y + l.grid.height - 1.0)).unwrap_or(from.1);
                        Drag::Move { index, tick: self.snap(t - from.0, false), row, from }
                    }
                    Drag::Resize { index, .. } => {
                        let start = self.pattern.notes.get(index).map_or(0, |n| n.tick);
                        Drag::Resize { index, len: (self.snap(t, false).saturating_sub(start)).max(self.unit.round() as u32).max(1) }
                    }
                    Drag::New { tick, row, len } if !self.drums => {
                        let end = self.snap(t, false).max(tick + self.unit.round() as u32);
                        let _ = len;
                        Drag::New { tick, row, len: end - tick }
                    }
                    Drag::Start(_) => {
                        let end = self.pattern.start + self.pattern.len;
                        Drag::Start(self.snap(t, false).min(end.saturating_sub(MIN_WINDOW)))
                    }
                    Drag::End(_) => Drag::End(self.snap(t, false).max(self.pattern.start + MIN_WINDOW).min(self.pattern.ticks)),
                    d => d,
                });
                Some(Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let drag = state.drag.take()?;
                let edit = match drag {
                    Drag::Move { index, tick, row, .. } => {
                        let n = self.pattern.notes.get(index)?;
                        let value = self.rows.get(row)?.value;
                        if (n.tick, n.row) == (tick, value) {
                            return Some(Action::request_redraw());
                        }
                        Edit::Move { index, tick, row: value }
                    }
                    Drag::Resize { index, len } => Edit::Resize { index, len },
                    Drag::New { tick, row, len } => {
                        let value = self.rows.get(row)?.value;
                        let (start, choke) = if self.drums { self.template(value) } else { (0, 0) };
                        Edit::Add(UserNote { tick, len, row: value, vel: 100, start, choke })
                    }
                    Drag::Start(_) | Drag::End(_) => {
                        let (start, len) = self.window(Some(drag));
                        Edit::Window { start, len: len.max(MIN_WINDOW) }
                    }
                };
                Some(Action::publish(Message::PatternEdit(edit)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &ViewState, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let l = self.layout(bounds);
        let g = l.grid;
        // Rows and their labels.
        for (i, r) in self.rows.iter().enumerate() {
            let y = g.y + i as f32 * l.rh;
            if r.dark {
                frame.fill_rectangle(Point::new(g.x, y), Size::new(g.width, l.rh), Color { a: 0.35, ..BG });
            }
            if r.mark {
                frame.fill_rectangle(Point::new(g.x, y + l.rh - 1.0), Size::new(g.width, 1.0), Color { a: 0.35, ..ACCENT });
            }
            if l.rh >= 7.0 || r.mark {
                frame.fill_text(canvas::Text {
                    content: r.label.clone(),
                    position: Point::new(KEY_W - 5.0, y + l.rh / 2.0),
                    color: if r.mark { ACCENT } else { DIM },
                    size: (l.rh * 0.75).clamp(7.0, 11.0).into(),
                    font: DATA_FONT,
                    align_x: TextAlign::Right,
                    align_y: Vertical::Center,
                    ..Default::default()
                });
            }
        }
        // The grid: the division faint, beats stronger, bars strongest.
        let mut t = 0.0;
        let mut k = 0usize;
        while t < l.ticks {
            let x = Self::x_of(&l, t);
            let bar = (t / BAR - (t / BAR).round()).abs() < 1e-3;
            let beat = (t / BEAT - (t / BEAT).round()).abs() < 1e-3;
            if k > 0 {
                let c = if bar { EDGE } else if beat { Color { a: 0.8, ..LINE } } else { Color { a: 0.45, ..LINE } };
                frame.fill_rectangle(Point::new(x, g.y), Size::new(1.0, g.height), c);
            }
            k += 1;
            t = k as f32 * self.unit;
        }
        // Beat lines the division does not cross (dotted, triplets).
        let mut b = BEAT;
        while b < l.ticks {
            if ((b / self.unit) - (b / self.unit).round()).abs() > 1e-3 {
                let bar = (b / BAR - (b / BAR).round()).abs() < 1e-3;
                frame.fill_rectangle(Point::new(Self::x_of(&l, b), g.y), Size::new(1.0, g.height), if bar { EDGE } else { Color { a: 0.8, ..LINE } });
            }
            b += BEAT;
        }
        // The ruler: bar numbers, beats.
        let bars = (l.ticks / BAR).ceil() as u32;
        let label_every = if g.width / bars.max(1) as f32 > 18.0 { 1 } else { 2 };
        for bar in 0..bars {
            let x = Self::x_of(&l, bar as f32 * BAR);
            if bar % label_every == 0 {
                frame.fill_text(canvas::Text { content: (bar + 1).to_string(), position: Point::new(x + 3.0, 2.0), color: INK, size: 11.0.into(), font: DATA_FONT, ..Default::default() });
            }
            for q in 1..4 {
                let bx = Self::x_of(&l, bar as f32 * BAR + q as f32 * BEAT);
                frame.fill_rectangle(Point::new(bx, RULER_H - 4.0), Size::new(1.0, 3.0), EDGE);
            }
        }
        frame.fill_rectangle(Point::new(g.x, RULER_H - 1.0), Size::new(g.width, 1.0), EDGE);
        // The window that plays.
        let (ws, wl) = self.window(state.drag);
        let (x0, x1) = (Self::x_of(&l, ws as f32), Self::x_of(&l, (ws + wl) as f32));
        frame.fill_rectangle(Point::new(x0, g.y), Size::new(x1 - x0, g.height), Color { a: 0.07, ..ACCENT });
        for (x, left) in [(x0, true), (x1, false)] {
            let path = Path::new(|p| {
                let dx = if left { 5.0 } else { -5.0 };
                p.move_to(Point::new(x + dx, 1.0));
                p.line_to(Point::new(x, 1.0));
                p.line_to(Point::new(x, bounds.height - 1.0));
                p.line_to(Point::new(x + dx, bounds.height - 1.0));
            });
            frame.stroke(&path, Stroke::default().with_color(ACCENT).with_width(2.0));
        }
        // The notes: inside the window lit, outside dim.
        for (n, active) in self.shown(state.drag) {
            let Some(r) = self.note_rect(&l, &n) else { continue };
            let inside = n.tick >= ws && n.tick < ws + wl;
            let c = if active { INK } else if inside { ACCENT } else { Color { a: 0.35, ..DIM } };
            let a = 0.55 + 0.45 * n.vel as f32 / 127.0;
            frame.fill_rectangle(r.position(), r.size(), Color { a: c.a * a, ..c });
        }
        // The playhead.
        if let Some(t) = self.playhead {
            let x = Self::x_of(&l, t as f32);
            frame.fill_rectangle(Point::new(x - 1.0, g.y), Size::new(3.0, g.height), Color { a: 0.2, ..INK });
            frame.fill_rectangle(Point::new(x, g.y), Size::new(1.0, g.height), INK);
        }
        if self.pattern.notes.is_empty() && self.rows.is_empty() {
            frame.fill_text(canvas::Text { content: "no hits".into(), position: Point::new(g.x + 8.0, g.y + 8.0), color: DIM, size: 12.0.into(), font: DATA_FONT, ..Default::default() });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &ViewState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match state.drag {
            Some(Drag::Resize { .. } | Drag::Start(_) | Drag::End(_)) => return mouse::Interaction::ResizingHorizontally,
            Some(_) => return mouse::Interaction::Grabbing,
            None => {}
        }
        let (Some(p), l) = (cursor.position_in(bounds), self.layout(bounds)) else { return mouse::Interaction::default() };
        let near = |t: u32| (p.x - Self::x_of(&l, t as f32)).abs() <= GRAB;
        if near(self.pattern.start) || near(self.pattern.start + self.pattern.len) {
            return mouse::Interaction::ResizingHorizontally;
        }
        match self.pattern.notes.iter().find_map(|n| self.note_rect(&l, n).filter(|r| r.contains(p))) {
            Some(r) if !self.drums && p.x >= r.x + r.width - GRAB => mouse::Interaction::ResizingHorizontally,
            Some(_) => mouse::Interaction::Grab,
            None if p.x >= l.grid.x && p.y >= l.grid.y => mouse::Interaction::Crosshair,
            None => mouse::Interaction::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern() -> UserPattern {
        let n = |tick, row| UserNote { tick, len: 48, row, vel: 100, start: 0, choke: 0 };
        UserPattern { notes: vec![n(0, 0), n(192, 7)], ticks: 768, start: 0, len: 384 }
    }

    #[test]
    fn units_follow_the_snap_and_its_feel() {
        assert_eq!(unit(16, Feel::Straight), 24.0);
        assert_eq!(unit(8, Feel::Dotted), 72.0);
        assert_eq!(unit(8, Feel::Triplet), 32.0);
    }

    #[test]
    fn edits_apply_and_keep_the_pattern_sound() {
        let mut p = pattern();
        Edit::Add(UserNote { tick: 96, len: 24, row: 3, vel: 100, start: 0, choke: 0 }).apply(&mut p);
        assert_eq!(p.notes.iter().map(|n| n.tick).collect::<Vec<_>>(), vec![0, 96, 192]);
        Edit::Move { index: 1, tick: 120, row: 5 }.apply(&mut p);
        assert_eq!((p.notes[1].tick, p.notes[1].row), (120, 5));
        Edit::Resize { index: 0, len: 2000 }.apply(&mut p);
        assert_eq!(p.notes[0].len, 768);
        Edit::Delete { index: 2 }.apply(&mut p);
        assert_eq!(p.notes.len(), 2);
        Edit::Window { start: 700, len: 10 }.apply(&mut p);
        assert!(p.start + p.len <= p.ticks && p.len >= MIN_WINDOW);
    }

    #[test]
    fn bass_rows_cover_the_notes_and_two_octaves() {
        let r = rows(&pattern(), false, 5, 29);
        assert!(r.len() >= MIN_ROWS as usize);
        assert!(r.iter().any(|x| x.value == 7) && r.iter().any(|x| x.value == 0));
        // Top down, the root marked.
        assert!(r.windows(2).all(|w| w[0].value > w[1].value));
        assert!(r.iter().find(|x| x.value == 0).unwrap().mark);
        assert_eq!(r.iter().find(|x| x.value == 0).unwrap().label, "F1");
    }

    #[test]
    fn drum_rows_are_the_targets_by_voice() {
        let n = |tick, row| UserNote { tick, len: 0, row, vel: 100, start: 0, choke: 0 };
        let hats = mesh_vst_core::generator::layout::HATS_BASE as i16;
        let p = UserPattern { notes: vec![n(0, hats + 1), n(0, 0), n(96, hats)], ticks: 384, start: 0, len: 384 };
        let r = rows(&p, true, 0, 0);
        assert_eq!(r.len(), 3);
        assert!(r.iter().filter(|x| x.label.starts_with("HATS")).count() == 2);
    }
}
