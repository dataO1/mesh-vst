//! The unison widget (D60): one square with three drag areas. The voices
//! on top (a number), the detune under it, the width at the bottom. Behind
//! them the voices as lines bending apart: as many as there are voices,
//! more jagged and blurred with the detune, further apart with the width.

use nice_plug_iced::iced::alignment::Vertical;
use nice_plug_iced::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use nice_plug_iced::iced::widget::{text::Alignment as TextAlign, Action};
use nice_plug_iced::iced::{mouse, Color, Point, Rectangle, Renderer, Theme};

use super::widgets::chamfer;
use super::{Message, ACCENT, DATA_FONT, DIM, EDGE, INK};

/// The three drag areas, top down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Voices,
    Detune,
    Width,
}

const ZONES: [Zone; 3] = [Zone::Voices, Zone::Detune, Zone::Width];
/// Highest detune (cents) and most voices, as the parameters allow.
pub const DETUNE_MAX: f32 = 60.0;
pub const VOICES_MAX: i32 = 8;
/// Pixels of drag over the whole of a range.
const DRAG_RANGE: f32 = 160.0;

pub struct UnisonPad {
    pub voices: i32,
    pub detune: f32,
    pub width: f32,
}

#[derive(Default)]
pub struct PadState {
    /// The area dragged, where it started and its value then (0..1).
    drag: Option<(Zone, Point, f32)>,
    hover: Option<Zone>,
}

impl UnisonPad {
    fn zone_at(bounds: Rectangle, y: f32) -> Zone {
        ZONES[(((y - bounds.y) / bounds.height) * 3.0).floor().clamp(0.0, 2.0) as usize]
    }

    /// Zone `z`'s value on 0..1.
    fn share(&self, z: Zone) -> f32 {
        match z {
            Zone::Voices => (self.voices - 1) as f32 / (VOICES_MAX - 1) as f32,
            Zone::Detune => self.detune / DETUNE_MAX,
            Zone::Width => self.width,
        }
    }

    fn message(z: Zone, v: f32) -> Message {
        let v = v.clamp(0.0, 1.0);
        match z {
            Zone::Voices => Message::UnisonVoices(1 + (v * (VOICES_MAX - 1) as f32).round() as i32),
            Zone::Detune => Message::UnisonDetune((v * DETUNE_MAX * 2.0).round() / 2.0),
            Zone::Width => Message::UnisonWidth((v * 100.0).round() / 100.0),
        }
    }
}

/// A seeded jitter in −1..1 for line `i`, point `k`.
fn jitter(i: usize, k: usize) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x9E37_79B9) ^ (k as u32).wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x % 2001) as f32 / 1000.0 - 1.0
}

impl canvas::Program<Message> for UnisonPad {
    type State = PadState;

    fn update(&self, state: &mut PadState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        let over = cursor.position_over(bounds);
        let hover = over.map(|p| Self::zone_at(bounds, p.y));
        if hover != state.hover && state.drag.is_none() {
            state.hover = hover;
            return Some(Action::request_redraw());
        }
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = over?;
                let z = Self::zone_at(bounds, p.y);
                state.drag = Some((z, p, self.share(z)));
                // The state before the drag goes on the undo stack.
                Some(Action::publish(Message::Grab).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let (z, from, v0) = state.drag?;
                // Up or right raises it.
                let d = (position.x - from.x) - (position.y - from.y);
                Some(Action::publish(Self::message(z, v0 + d / DRAG_RANGE)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.drag.is_some() => {
                state.drag = None;
                Some(Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let z = hover?;
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 20.0,
                };
                let step = if z == Zone::Voices { 1.0 / (VOICES_MAX - 1) as f32 } else { 0.02 };
                Some(Action::publish(Self::message(z, self.share(z) + dy.signum() * step)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &PadState, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        frame.stroke(&chamfer(bounds.size(), 9.0), Stroke::default().with_color(EDGE).with_width(1.0));

        // The voices as upright lines: straight with no detune, bending apart
        // in the middle, twisting and jagged the more detune.
        let n = self.voices.clamp(1, VOICES_MAX) as usize;
        let rough = (self.detune / DETUNE_MAX).clamp(0.0, 1.0);
        let x0 = w * 0.42;
        let (left, right) = (x0 + 6.0, w - 10.0);
        let cx = (left + right) / 2.0;
        // The width spreads them, inside the pad at the widest bend.
        let reach = (right - left) / 2.0 / 1.45;
        let spacing = ((3.0 + 11.0 * self.width.clamp(0.0, 1.0)).min(2.0 * reach / (n as f32 - 1.0).max(1.0))).max(1.5);
        let curve = rough.powf(0.7);
        let jag = 4.0 * rough.powf(1.5);
        let steps = 48usize;
        for i in 0..n {
            let off = i as f32 - (n as f32 - 1.0) / 2.0;
            let line = |blur: f32| {
                Path::new(|b| {
                    for k in 0..=steps {
                        let t = k as f32 / steps as f32;
                        let y = 8.0 + t * (h - 16.0);
                        let bend = 1.0 + curve * (0.35 + 1.1 * (std::f32::consts::PI * t).sin() - 1.0);
                        let twist = curve * 0.25 * off * (3.0 * std::f32::consts::PI * t + i as f32).sin();
                        let x = cx + (off * bend + twist) * spacing + jag * jitter(i, k) + blur;
                        if k == 0 {
                            b.move_to(Point::new(x, y));
                        } else {
                            b.line_to(Point::new(x, y));
                        }
                    }
                })
            };
            // The detune blurs: faint copies either side.
            for (blur, a) in [(-2.5 * rough, 0.18 * rough), (2.5 * rough, 0.18 * rough), (0.0, 0.85)] {
                if a > 0.01 {
                    frame.stroke(&line(blur), Stroke::default().with_color(Color { a, ..ACCENT }).with_width(1.2 + rough));
                }
            }
        }

        // The three areas and their values.
        let zh = h / 3.0;
        for (k, z) in ZONES.iter().enumerate() {
            let y = k as f32 * zh;
            let active = state.drag.map(|d| d.0) == Some(*z) || state.hover == Some(*z);
            if k > 0 {
                frame.fill_rectangle(Point::new(10.0, y), nice_plug_iced::iced::Size::new(x0 - 20.0, 1.0), Color { a: 0.5, ..EDGE });
            }
            let (value, label) = match z {
                Zone::Voices => (self.voices.to_string(), "VOICES"),
                Zone::Detune => (format!("{:.0} ct", self.detune), "DETUNE"),
                Zone::Width => (format!("{:.0} %", self.width * 100.0), "WIDTH"),
            };
            let big = *z == Zone::Voices;
            frame.fill_text(canvas::Text {
                content: value,
                position: Point::new(12.0, y + zh * 0.42),
                color: if active { INK } else { Color { a: 0.9, ..INK } },
                size: (if big { 22.0 } else { 12.0 }).into(),
                font: DATA_FONT,
                align_x: TextAlign::Left,
                align_y: Vertical::Center,
                ..Default::default()
            });
            frame.fill_text(canvas::Text {
                content: label.into(),
                position: Point::new(12.0, y + zh * 0.82),
                color: if active { ACCENT } else { DIM },
                size: 9.0.into(),
                font: DATA_FONT,
                align_x: TextAlign::Left,
                align_y: Vertical::Center,
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &PadState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if state.drag.is_some() || cursor.is_over(bounds) { mouse::Interaction::ResizingVertically } else { mouse::Interaction::default() }
    }
}
