//! The editor's own controls (D60): chamfered chips for buttons, the
//! segmented knob slider, a value box for the transpose. Drawn on canvases
//! in the editor's palette.

use nice_plug_iced::iced::alignment::Vertical;
use nice_plug_iced::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use nice_plug_iced::iced::widget::{canvas as canvas_widget, text::Alignment as TextAlign, Action};
use nice_plug_iced::iced::{mouse, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

use super::{Knob, Message, ACCENT, BG, DATA_FONT, DIM, EDGE, INK, LINE};

/// Corner cut of a chip, in px.
const CUT: f32 = 5.0;
/// Width of one character of the data font at size 1.
const CHAR_W: f32 = 0.56;

/// A chamfered outline in `size`, its top-left and bottom-right corners cut.
pub fn chamfer(size: Size, cut: f32) -> Path {
    Path::new(|b| {
        b.move_to(Point::new(cut, 0.5));
        b.line_to(Point::new(size.width - 0.5, 0.5));
        b.line_to(Point::new(size.width - 0.5, size.height - cut));
        b.line_to(Point::new(size.width - cut, size.height - 0.5));
        b.line_to(Point::new(0.5, size.height - 0.5));
        b.line_to(Point::new(0.5, cut));
        b.close();
    })
}

/// A button: label, lit when `on`, sending `msg` on a click (none: off).
pub struct Chip {
    label: String,
    on: bool,
    msg: Option<Message>,
    size: f32,
}

#[derive(Default)]
pub struct ChipState {
    hover: bool,
    pressed: bool,
}

impl canvas::Program<Message> for Chip {
    type State = ChipState;

    fn update(&self, state: &mut ChipState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        let over = cursor.is_over(bounds);
        if over != state.hover {
            state.hover = over;
            return Some(Action::request_redraw());
        }
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
                state.pressed = true;
                Some(Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.pressed => {
                state.pressed = false;
                match (&self.msg, over) {
                    (Some(m), true) => Some(Action::publish(m.clone()).and_capture()),
                    _ => Some(Action::request_redraw()),
                }
            }
            _ => None,
        }
    }

    fn draw(&self, state: &ChipState, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let path = chamfer(bounds.size(), CUT);
        let enabled = self.msg.is_some();
        if self.on {
            frame.fill(&path, INK);
        } else if state.hover && enabled {
            frame.fill(&path, Color { a: 0.06, ..INK });
        }
        let edge = if self.on { INK } else if state.hover && enabled { DIM } else { EDGE };
        frame.stroke(&path, Stroke::default().with_color(edge).with_width(1.0));
        frame.fill_text(canvas::Text {
            content: self.label.clone(),
            position: Point::new(bounds.width / 2.0, bounds.height / 2.0),
            color: if self.on { BG } else if enabled { INK } else { EDGE },
            size: self.size.into(),
            font: DATA_FONT,
            align_x: TextAlign::Center,
            align_y: Vertical::Center,
            ..Default::default()
        });
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &ChipState, _: Rectangle, _: mouse::Cursor) -> mouse::Interaction {
        if state.hover && self.msg.is_some() { mouse::Interaction::Pointer } else { mouse::Interaction::default() }
    }
}


/// A small chip.
pub fn chip_sm<'a>(label: &str, on: bool, msg: Option<Message>) -> Element<'a, Message> {
    chip_sized(label, on, msg, 11.0, None)
}

/// A chip that fills its share of a row.
pub fn chip_fill<'a>(label: &str, on: bool, msg: Option<Message>) -> Element<'a, Message> {
    chip_sized(label, on, msg, 11.0, Some(Length::FillPortion(1)))
}

fn chip_sized<'a>(label: &str, on: bool, msg: Option<Message>, size: f32, width: Option<Length>) -> Element<'a, Message> {
    let label = label.to_uppercase();
    let w = width.unwrap_or(Length::Fixed((label.chars().count() as f32 * size * CHAR_W + 2.0 * size).round()));
    let h = (size * 2.1).round();
    canvas_widget(Chip { label, on, msg, size }).width(w).height(h).into()
}

/// A knob as a segmented bar: drag (or click) sets it, `hit` (position,
/// brightness) marks where the last hit played it.
pub struct SegSlider {
    pub knob: Knob,
    pub value: f32,
    pub hit: Option<(f32, f32)>,
}

#[derive(Default)]
pub struct DragState {
    dragging: bool,
}

/// Segment pitch and gap of the bar, in px.
const SEG: f32 = 10.0;
const SEG_GAP: f32 = 1.0;
const TRACK_H: f32 = 6.0;

fn value_at(bounds: Rectangle, x: f32) -> f32 {
    ((x - bounds.x) / bounds.width).clamp(0.0, 1.0)
}

impl canvas::Program<Message> for SegSlider {
    type State = DragState;

    fn update(&self, state: &mut DragState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_over(bounds)?;
                state.dragging = true;
                Some(Action::publish(Message::Knob(self.knob, value_at(bounds, p.x))).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging => {
                Some(Action::publish(Message::Knob(self.knob, value_at(bounds, position.x))).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
                Some(Action::publish(Message::KnobReleased(self.knob)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, _: &DragState, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let y = (bounds.height - TRACK_H) / 2.0;
        let fill = self.value.clamp(0.0, 1.0) * w;
        let mut x = 0.0;
        while x < w {
            let seg = (SEG - SEG_GAP).min(w - x);
            // The segment under the value's edge is lit up to it.
            let lit = (fill - x).clamp(0.0, seg);
            frame.fill_rectangle(Point::new(x, y), Size::new(seg, TRACK_H), LINE);
            if lit > 0.0 {
                frame.fill_rectangle(Point::new(x, y), Size::new(lit, TRACK_H), ACCENT);
            }
            x += SEG;
        }
        if let Some((at, glow)) = self.hit {
            let hx = at.clamp(0.0, 1.0) * w;
            for (bw, k) in [(7.0, 0.15), (4.0, 0.3), (1.5, 0.6)] {
                frame.fill_rectangle(Point::new(hx - bw / 2.0, 1.0), Size::new(bw, bounds.height - 2.0), Color { a: (0.1 + 0.25 * glow) * k, ..INK });
            }
        }
        frame.fill_rectangle(Point::new((fill - 1.5).clamp(0.0, w - 3.0), 0.0), Size::new(3.0, bounds.height), INK);
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &DragState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if state.dragging || cursor.is_over(bounds) { mouse::Interaction::ResizingHorizontally } else { mouse::Interaction::default() }
    }
}

/// A number box: the wheel or a vertical drag steps it within `range`,
/// a double click goes back to `reset`.
pub struct ValueBox {
    pub value: i32,
    pub range: (i32, i32),
    pub reset: i32,
    pub unit: &'static str,
    pub on_change: fn(i32) -> Message,
    pub on_release: Message,
}

#[derive(Default)]
pub struct BoxState {
    drag: Option<(f32, i32)>,
    last_click: Option<std::time::Instant>,
    hover: bool,
}

/// Pixels of vertical drag per step.
const DRAG_STEP: f32 = 8.0;

impl canvas::Program<Message> for ValueBox {
    type State = BoxState;

    fn update(&self, state: &mut BoxState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        let over = cursor.is_over(bounds);
        if over != state.hover {
            state.hover = over;
        }
        let clamp = |v: i32| v.clamp(self.range.0, self.range.1);
        match event {
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) if over => {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 20.0,
                };
                let step = if dy > 0.0 { 1 } else if dy < 0.0 { -1 } else { 0 };
                (step != 0).then(|| Action::publish((self.on_change)(clamp(self.value + step))).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
                let now = std::time::Instant::now();
                if state.last_click.is_some_and(|t| now.duration_since(t).as_millis() < 350) {
                    state.last_click = None;
                    return Some(Action::publish((self.on_change)(self.reset)).and_capture());
                }
                state.last_click = Some(now);
                state.drag = cursor.position().map(|p| (p.y, self.value));
                Some(Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let (y0, v0) = state.drag?;
                let v = clamp(v0 + ((y0 - position.y) / DRAG_STEP).round() as i32);
                (v != self.value).then(|| Action::publish((self.on_change)(v)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.drag.is_some() => {
                state.drag = None;
                Some(Action::publish(self.on_release.clone()).and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &BoxState, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let path = chamfer(bounds.size(), CUT);
        frame.stroke(&path, Stroke::default().with_color(if state.hover || state.drag.is_some() { DIM } else { EDGE }).with_width(1.0));
        let sign = if self.value > 0 { "+" } else { "" };
        frame.fill_text(canvas::Text {
            content: format!("{sign}{}{}", self.value, self.unit),
            position: Point::new(bounds.width / 2.0 - 4.0, bounds.height / 2.0),
            color: if self.value == self.reset { INK } else { ACCENT },
            size: 14.0.into(),
            font: DATA_FONT,
            align_x: TextAlign::Center,
            align_y: Vertical::Center,
            ..Default::default()
        });
        // The arrows: it scrolls.
        let ax = bounds.width - 9.0;
        let my = bounds.height / 2.0;
        for (dy, up) in [(-4.0, true), (4.0, false)] {
            let tri = Path::new(|b| {
                let y = my + dy;
                if up {
                    b.move_to(Point::new(ax - 3.0, y + 1.5));
                    b.line_to(Point::new(ax + 3.0, y + 1.5));
                    b.line_to(Point::new(ax, y - 1.5));
                } else {
                    b.move_to(Point::new(ax - 3.0, y - 1.5));
                    b.line_to(Point::new(ax + 3.0, y - 1.5));
                    b.line_to(Point::new(ax, y + 1.5));
                }
                b.close();
            });
            frame.fill(&tri, DIM);
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &BoxState, _: Rectangle, _: mouse::Cursor) -> mouse::Interaction {
        if state.hover || state.drag.is_some() { mouse::Interaction::ResizingVertically } else { mouse::Interaction::default() }
    }
}
