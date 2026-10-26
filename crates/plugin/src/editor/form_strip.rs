//! The form strip (D60) at the top of the footer: what plays. The lane's
//! own form as blocks over both passes with the playhead, its cycles
//! ticked (D61); in the external
//! mode the eight patterns, the one the DAW plays lit with its operator.
//! A click on a block or a pattern opens it in the editor.

use nice_plug_iced::iced::alignment::Vertical;
use nice_plug_iced::iced::widget::canvas::{self, Frame, Geometry, Stroke};
use nice_plug_iced::iced::widget::{text::Alignment as TextAlign, Action};
use nice_plug_iced::iced::{mouse, Color, Point, Rectangle, Renderer, Size, Theme};

use mesh_vst_core::form::Letter;
use mesh_vst_core::midi::FormNote;
use mesh_vst_core::slots::{Slot, VelOp, SLOTS};

use super::widgets::chamfer;
use super::{Message, ACCENT, BG, DATA_FONT, DIM, EDGE, INK};

pub struct FormStrip {
    /// The lane's form (internal), and its length in beats.
    pub notes: Vec<FormNote>,
    pub beats: u32,
    /// Where its cycles start (beats, D61): marked under the blocks.
    pub cycles: Vec<u32>,
    /// The beat playing, if any.
    pub at: Option<f32>,
    /// The DAW plays the form: the slot and operator it plays.
    pub external: bool,
    pub playing: Option<(Slot, VelOp)>,
    /// The slot the editor shows.
    pub selected: usize,
}

const GAP: f32 = 2.0;
/// Bars of the form the strip shows: the page playing.
const PAGE_BARS: u32 = 16;
const LABEL_W: f32 = 44.0;

impl FormStrip {
    fn area(bounds: Rectangle) -> Rectangle {
        Rectangle { x: LABEL_W, y: 0.0, width: (bounds.width - LABEL_W).max(1.0), height: bounds.height }
    }

    /// The page of the form playing (its first beat and its beats).
    fn page(&self) -> (u32, u32) {
        let beats = (PAGE_BARS * 4).min(self.beats.max(1));
        let at = self.at.unwrap_or(0.0).max(0.0) as u32;
        (at / beats * beats, beats)
    }

    /// The slot under `x` (local).
    fn slot_at(&self, bounds: Rectangle, x: f32) -> Option<Slot> {
        let a = Self::area(bounds);
        if x < a.x {
            return None;
        }
        if self.external {
            let i = ((x - a.x) / a.width * SLOTS as f32).floor() as usize;
            return (i < SLOTS).then(|| Slot::from_index(i));
        }
        let (from, beats) = self.page();
        let beat = from as f32 + (x - a.x) / a.width * beats as f32;
        self.notes.iter().find(|n| beat >= n.beat as f32 && beat < (n.beat + n.beats) as f32).map(|n| n.slot)
    }
}

fn ink_of(letter: Letter) -> Color {
    match letter {
        Letter::A | Letter::B => INK,
        // The fills dimmer.
        Letter::C | Letter::D => DIM,
    }
}

impl canvas::Program<Message> for FormStrip {
    type State = ();

    fn update(&self, _: &mut (), event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            let p = cursor.position_in(bounds)?;
            let slot = self.slot_at(bounds, p.x)?;
            return Some(Action::publish(Message::SelectSlot(slot.index())).and_capture());
        }
        None
    }

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let a = Self::area(bounds);
        frame.fill_text(canvas::Text {
            content: if self.external { "FORM\nEXT".to_string() } else { let (from, beats) = self.page(); format!("INT\n{}–{}", from / 4 + 1, (from + beats) / 4) },
            position: Point::new(0.0, bounds.height / 2.0),
            color: DIM,
            size: 10.0.into(),
            font: DATA_FONT,
            align_y: Vertical::Center,
            ..Default::default()
        });
        // Drawn directly (a nested frame per block would be costly).
        let mut draw_block = |x: f32, w: f32, label: String, color: Color, lit: bool, selected: bool| {
            if w < 1.0 {
                return;
            }
            let size = Size::new(w, a.height);
            let path = chamfer(size, 4.0_f32.min(w / 2.0));
            frame.with_save(|f| {
                f.translate(nice_plug_iced::iced::Vector::new(x, a.y));
                if lit {
                    f.fill(&path, Color { a: 0.85, ..ACCENT });
                } else if selected {
                    f.fill(&path, Color { a: 0.12, ..ACCENT });
                }
                f.stroke(&path, Stroke::default().with_color(if selected { ACCENT } else { EDGE }).with_width(1.0));
                if w > 16.0 {
                    f.fill_text(canvas::Text {
                        content: label,
                        position: Point::new(w / 2.0, a.height / 2.0),
                        color: if lit { BG } else { color },
                        size: 10.0.into(),
                        font: DATA_FONT,
                        align_x: TextAlign::Center,
                        align_y: Vertical::Center,
                        ..Default::default()
                    });
                }
            });
        };
        if self.external {
            let w = a.width / SLOTS as f32;
            for s in Slot::ALL {
                let playing = self.playing.filter(|p| p.0 == s);
                let label = match playing {
                    Some((_, op)) if !op.mark().is_empty() => format!("{} {}", s.name(), op.mark()),
                    _ => s.name().to_string(),
                };
                draw_block(a.x + s.index() as f32 * w, w - GAP, label, ink_of(s.letter), playing.is_some(), s.index() == self.selected);
            }
        } else {
            // The page of the form playing.
            let (from, beats) = self.page();
            let per = a.width / beats as f32;
            for n in &self.notes {
                let (lo, hi) = (n.beat.max(from), (n.beat + n.beats).min(from + beats));
                if lo >= hi {
                    continue;
                }
                let lit = self.at.is_some_and(|b| b >= n.beat as f32 && b < (n.beat + n.beats) as f32);
                draw_block(a.x + (lo - from) as f32 * per, (hi - lo) as f32 * per - GAP, n.label(), ink_of(n.slot.letter), lit, n.slot.index() == self.selected);
            }
            // The cycles: a tick under each one's start.
            for &c in self.cycles.iter().filter(|c| **c > from && **c < from + beats) {
                let x = a.x + (c - from) as f32 * per - GAP / 2.0;
                frame.fill_rectangle(Point::new(x - 0.5, a.height - 5.0), Size::new(1.0, 5.0), ACCENT);
            }
            if let Some(b) = self.at {
                let x = a.x + (b - from as f32) * per;
                frame.fill_rectangle(Point::new(x, 0.0), Size::new(1.5, bounds.height), INK);
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, _: &(), bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match cursor.position_in(bounds) {
            Some(p) if self.slot_at(bounds, p.x).is_some() => mouse::Interaction::Pointer,
            _ => mouse::Interaction::default(),
        }
    }
}
