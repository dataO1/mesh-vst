use std::time::Instant;
use std::{cell::RefCell, pin::Pin, rc::Rc};

use baseview::dpi::LogicalSize;
use baseview::{EventStatus, HandlerError, WindowContext, WindowHandler};
use iced_core::{mouse, theme};
use iced_program::Program;
use iced_runtime::Task;
pub use iced_runtime::core::window::Id;
use iced_runtime::futures::futures::{
    self,
    channel::mpsc::{self, SendError},
};
pub use iced_runtime::window::{close_events, close_requests, events, open_events, resize_events};
use iced_widget::core::Size;

#[cfg(all(feature = "log", not(feature = "tracing")))]
use log::error;
#[cfg(feature = "tracing")]
use tracing::error;

use crate::graphics::Compositor;
use crate::shell::{RuntimeEvent, SharedWindowSize};

pub(super) mod state;

pub(super) struct InstanceWindow<P, C>
where
    P: Program,
    C: Compositor<Renderer = P::Renderer>,
    P::Theme: theme::Base,
{
    pub state: state::State<P>,
    pub mouse_interaction: mouse::Interaction,
    pub surface: C::Surface,
    pub surface_version: u64,
    pub compositor: C,
    pub renderer: P::Renderer,
    //pub preedit: Option<Preedit<P::Renderer>>,
    pub queue: WindowQueue,
    pub window: WindowContext,
    pub id: iced_core::window::Id,
    #[allow(unused)]
    pub always_redraw: bool,
    pub ignore_non_modifier_keys: bool,
    pub redraw_requested: bool,
    pub redraw_at: Option<Instant>,
    //ime_state: Option<(Rectangle, input_method::Purpose)>,
}

pub(super) struct IcedWindowHandler<Message: 'static + Send> {
    inner: RefCell<IcedWindowHandlerInner<Message>>,
    window_commands_rx: RefCell<mpsc::UnboundedReceiver<WindowCommand>>,
    runtime_state: RefCell<RuntimeState>,
    window: WindowContext,
    event_status: Rc<RefCell<EventStatus>>,
    shared_window_size: SharedWindowSize,
    poll_timer: baseview::TimerHandle,
}

struct IcedWindowHandlerInner<Message: 'static + Send> {
    runtime_event_tx: mpsc::UnboundedSender<RuntimeEvent<Message>>,
    action_rx: mpsc::UnboundedReceiver<iced_runtime::Action<Message>>,
    processed_close_signal: bool,
}

struct RuntimeState {
    instance: Pin<Box<dyn futures::Future<Output = ()>>>,
    runtime_context: futures::task::Context<'static>,
}

impl<Message: 'static + Send> IcedWindowHandler<Message> {
    pub fn new(
        window: WindowContext,
        instance: Pin<Box<dyn futures::Future<Output = ()>>>,
        runtime_context: futures::task::Context<'static>,
        event_status: Rc<RefCell<EventStatus>>,
        window_commands_rx: mpsc::UnboundedReceiver<WindowCommand>,
        runtime_event_tx: mpsc::UnboundedSender<RuntimeEvent<Message>>,
        action_rx: mpsc::UnboundedReceiver<iced_runtime::Action<Message>>,
        shared_window_size: SharedWindowSize,
        poll_timer: baseview::TimerHandle,
    ) -> Self {
        Self {
            inner: RefCell::new(IcedWindowHandlerInner {
                runtime_event_tx,
                action_rx,
                processed_close_signal: false,
            }),
            window_commands_rx: RefCell::new(window_commands_rx),
            runtime_state: RefCell::new(RuntimeState {
                instance,
                runtime_context,
            }),
            window,
            event_status,
            shared_window_size,
            poll_timer,
        }
    }

    fn drain_window_commands(&self) {
        if let Some(size) = self.shared_window_size.get_requested_size() {
            let zoom_factor = self.shared_window_size.user_scale_factor();

            let size = baseview::dpi::Size::Logical(LogicalSize {
                width: (size.width * zoom_factor) as f64,
                height: (size.height * zoom_factor) as f64,
            });

            if let Err(e) = self.window.resize(size) {
                error!("Failed to resize window to {:?}: {}", size, e);
            }
        }

        while let Ok(cmd) = { self.window_commands_rx.borrow_mut().try_recv() } {
            match cmd {
                WindowCommand::CloseWindow => {
                    self.window.request_close();
                }
                WindowCommand::ResizeWindow { size } => {
                    let zoom_factor = self.shared_window_size.user_scale_factor();

                    let size = baseview::dpi::Size::Logical(LogicalSize {
                        width: (size.width * zoom_factor) as f64,
                        height: (size.height * zoom_factor) as f64,
                    });

                    if let Err(e) = self.window.resize(size) {
                        error!("Failed to resize window to {:?}: {}", size, e);
                    }
                }
                WindowCommand::Focus => {
                    if let Err(e) = self.window.focus() {
                        error!("Failed to focus window: {}", e);
                    }
                }
                WindowCommand::SetCursorIcon(cursor) => {
                    if let Err(e) = self.window.set_mouse_cursor(cursor) {
                        error!("Failed to set window cursor icon: {}", e);
                    }
                }
            }
        }
    }

    fn poll_runtime(&self) -> Result<(), HandlerError> {
        {
            let mut inner = self.inner.borrow_mut();

            if inner.processed_close_signal {
                return Ok(());
            }

            inner.runtime_event_tx.start_send(RuntimeEvent::Poll)?;
        }

        {
            let mut runtime = self.runtime_state.borrow_mut();

            let RuntimeState {
                instance,
                runtime_context,
            } = &mut *runtime;

            // Flush all messages. This will block until the instance is finished.
            let _ = instance.as_mut().poll(runtime_context);
        }

        Ok(())
    }

    fn update_runtime(&self, send_draw_event: bool) -> Result<(), HandlerError> {
        // Poll subscriptions and send the corresponding messages.
        {
            let mut inner = self.inner.borrow_mut();

            while let Ok(message) = inner.action_rx.try_recv() {
                inner
                    .runtime_event_tx
                    .start_send(RuntimeEvent::UserEvent(message))?;
            }

            if send_draw_event {
                inner.runtime_event_tx.start_send(RuntimeEvent::Draw)?;
            }
        }

        {
            let mut runtime = self.runtime_state.borrow_mut();

            let RuntimeState {
                instance,
                runtime_context,
            } = &mut *runtime;

            // Flush all messages. This will block until the instance is finished.
            let _ = instance.as_mut().poll(runtime_context);
        }

        self.drain_window_commands();

        Ok(())
    }
}

impl<Message: 'static + Send> WindowHandler for IcedWindowHandler<Message> {
    fn poll(&self) {
        self.poll_runtime().unwrap();
        self.update_runtime(false).unwrap();
    }

    fn on_event(&self, event: baseview::Event) -> EventStatus {
        let file_drag = matches!(
            &event,
            baseview::Event::Mouse(
                baseview::MouseEvent::DragEntered { data: baseview::DropData::Files(_), .. }
                    | baseview::MouseEvent::DragMoved { data: baseview::DropData::Files(_), .. }
                    | baseview::MouseEvent::DragDropped { data: baseview::DropData::Files(_), .. }
            )
        );
        // Parent/embedded windows do not always gain keyboard focus
        // Automatically on click. Request focus explicitly before forwarding the event.
        //
        // TODO: Check if this is still necessary.
        if matches!(
            event,
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed { .. })
        ) && !self.window.has_focus()
        {
            if let Err(e) = self.window.focus() {
                error!("Failed to focus window: {}", e);
            }
        }

        let ignore_event;

        {
            let mut inner = self.inner.borrow_mut();

            if inner.processed_close_signal {
                return EventStatus::Ignored;
            }

            ignore_event = if requests_exit(&event) {
                inner.processed_close_signal = true;

                inner
                    .runtime_event_tx
                    .start_send(RuntimeEvent::WillClose)
                    .expect("Send event");

                true
            } else {
                // Send the event to the instance.
                inner
                    .runtime_event_tx
                    .start_send(RuntimeEvent::Baseview((event, true)))
                    .expect("Send event");

                false
            };
        }

        self.update_runtime(false).unwrap();

        if ignore_event {
            EventStatus::Ignored
        } else if file_drag {
            // mesh vst patch: accept dropped files.
            EventStatus::AcceptDrop(baseview::DropEffect::Copy)
        } else {
            *self.event_status.borrow()
        }
    }

    fn draw(&self) -> Result<(), HandlerError> {
        self.poll_runtime()?;
        self.update_runtime(true)?;

        Ok(())
    }

    fn resized(&self, new_size: baseview::WindowSize) -> Result<(), HandlerError> {
        {
            let mut inner = self.inner.borrow_mut();

            if inner.processed_close_signal {
                return Ok(());
            }

            inner
                .runtime_event_tx
                .start_send(RuntimeEvent::Resized(new_size))?;
        }

        self.update_runtime(true)?;

        Ok(())
    }

    fn on_timer(&self, timer: &baseview::TimerHandle) {
        if timer == self.poll_timer {
            self.poll_runtime().unwrap();
            self.update_runtime(true).unwrap();
        }
    }
}

/// Closes the application window.
pub fn close<T>() -> Task<T> {
    iced_runtime::window::close(Id::unique())
}

/// Resize the application window to the given logical dimensions.
pub fn resize<T>(new_size: Size) -> Task<T> {
    iced_runtime::window::resize(Id::unique(), new_size)
}

/// Brings the application window to the front and sets input focus. Has no effect if the window
/// is already in focus, minimized, or not visible.
///
/// This [`Task`] steals input focus from other applications. Do not use this method unless
/// you are certain that's what the user wants. Focus stealing can cause an extremely disruptive
/// user experience.
pub fn gain_focus<T>() -> Task<T> {
    iced_runtime::window::gain_focus(Id::unique())
}

/// Returns true if the provided event should cause an
/// [`Application`](crate::application::Application) to exit.
pub fn requests_exit(event: &baseview::Event) -> bool {
    match event {
        baseview::Event::Window(baseview::WindowEvent::WillClose) => true,
        #[cfg(target_os = "macos")]
        baseview::Event::Keyboard(event) => {
            if event.code == keyboard_types::Code::KeyQ
                && event.modifiers == keyboard_types::Modifiers::META
                && event.state == keyboard_types::KeyState::Down
            {
                return true;
            }

            false
        }
        _ => false,
    }
}

/// Use this to send custom events to the iced window.
///
/// Please note this channel is ***not*** realtime-safe and should never be
/// be used to send events from the audio thread. Use a realtime-safe ring
/// buffer instead.
#[allow(missing_debug_implementations)]
pub struct IcedMessageSender<Message: 'static + Send> {
    tx: mpsc::UnboundedSender<RuntimeEvent<Message>>,
}

impl<Message: 'static + Send> IcedMessageSender<Message> {
    pub(crate) fn new(tx: mpsc::UnboundedSender<RuntimeEvent<Message>>) -> Self {
        Self { tx }
    }

    /// Send a custom message to the window.
    ///
    /// Please note this channel is ***NOT*** realtime-safe and should never be
    /// used to send events from the audio thread. Use a realtime-safe ring
    /// buffer instead (the [`PollSubNotifier`](crate::PollSubNotifier) can be
    /// used by the audio thread to notify the GUI that it should poll messages
    /// from you realtime-safe ring buffer.)
    pub fn send_message(&mut self, msg: Message) -> Result<(), SendError> {
        self.tx
            .start_send(RuntimeEvent::UserEvent(iced_runtime::Action::Output(msg)))
    }
}

impl<Message: 'static + Send> Clone for IcedMessageSender<Message> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

pub(crate) enum WindowCommand {
    CloseWindow,
    ResizeWindow { size: Size },
    Focus,
    SetCursorIcon(baseview::MouseCursor),
}

/// Used to request things from the `baseview` window.
#[derive(Clone)]
pub(crate) struct WindowQueue {
    tx: mpsc::UnboundedSender<WindowCommand>,
}

impl WindowQueue {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<WindowCommand>) {
        let (tx, rx) = mpsc::unbounded();

        (Self { tx }, rx)
    }

    pub fn send(&mut self, command: WindowCommand) {
        if let Err(e) = self.tx.start_send(command) {
            #[cfg(any(feature = "tracing", feature = "log"))]
            error!("Failed to send command to window: {}", e);
        }
    }
}

/*
pub(super) struct Preedit<Renderer>
where
    Renderer: text::Renderer,
{
    position: Point,
    content: Renderer::Paragraph,
    spans: Vec<text::Span<'static, (), Renderer::Font>>,
}

impl<Renderer> Preedit<Renderer>
where
    Renderer: text::Renderer,
{
    fn new() -> Self {
        Self {
            position: Point::ORIGIN,
            spans: Vec::new(),
            content: Renderer::Paragraph::default(),
        }
    }

    fn update(
        &mut self,
        cursor: Rectangle,
        preedit: &input_method::Preedit,
        background: Color,
        renderer: &Renderer,
    ) {
        self.position = cursor.position() + Vector::new(0.0, cursor.height);

        let background = Color {
            a: 1.0,
            ..background
        };

        let spans = match &preedit.selection {
            Some(selection) => {
                vec![
                    text::Span::new(&preedit.content[..selection.start]),
                    text::Span::new(if selection.start == selection.end {
                        "\u{200A}"
                    } else {
                        &preedit.content[selection.start..selection.end]
                    })
                    .color(background),
                    text::Span::new(&preedit.content[selection.end..]),
                ]
            }
            _ => vec![text::Span::new(&preedit.content)],
        };

        if spans != self.spans.as_slice() {
            use text::Paragraph as _;

            self.content = Renderer::Paragraph::with_spans(Text {
                content: &spans,
                bounds: Size::INFINITE,
                size: preedit.text_size.unwrap_or_else(|| renderer.default_size()),
                line_height: text::LineHeight::default(),
                font: renderer.default_font(),
                align_x: text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: text::Shaping::Advanced,
                wrapping: text::Wrapping::None,
            });

            self.spans.clear();
            self.spans
                .extend(spans.into_iter().map(text::Span::to_static));
        }
    }

    pub fn draw(
        &self,
        renderer: &mut Renderer,
        color: Color,
        background: Color,
        viewport: &Rectangle,
    ) {
        use text::Paragraph as _;

        if self.content.min_width() < 1.0 {
            return;
        }

        let mut bounds = Rectangle::new(
            self.position - Vector::new(0.0, self.content.min_height()),
            self.content.min_bounds(),
        );

        bounds.x = bounds
            .x
            .max(viewport.x)
            .min(viewport.x + viewport.width - bounds.width);

        bounds.y = bounds
            .y
            .max(viewport.y)
            .min(viewport.y + viewport.height - bounds.height);

        renderer.with_layer(bounds, |renderer| {
            let background = Color {
                a: 1.0,
                ..background
            };

            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    ..Default::default()
                },
                background,
            );

            renderer.fill_paragraph(&self.content, bounds.position(), color, bounds);

            const UNDERLINE: f32 = 2.0;

            renderer.fill_quad(
                renderer::Quad {
                    bounds: bounds.shrink(Padding {
                        top: bounds.height - UNDERLINE,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                color,
            );

            for span_bounds in self.content.span_bounds(1) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: span_bounds + (bounds.position() - Point::ORIGIN),
                        ..Default::default()
                    },
                    color,
                );
            }
        });
    }
}
*/
