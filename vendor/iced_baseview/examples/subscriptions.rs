use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use iced_baseview::{
    Alignment, IcedWindowSettings, Length, PollSubNotifier, Subscription, Theme, application,
    baseview::dpi::LogicalSize,
    shell::SharedWindowSize,
    widget::{Column, Container, Text},
};

const WINDOW_SIZE: LogicalSize<f32> = LogicalSize::new(500.0, 300.0);

fn main() {
    // Used as a workaround for https://github.com/RustAudio/baseview/issues/321
    //
    // # Safety
    //
    // ONLY call this when running the application in standalone mode. Do *NOT* call this
    // when running as a plugin that is loaded into a host. For more information, see:
    // https://docs.rs/baseview/0.3.4/baseview/fn.assume_standalone_in_process.html
    unsafe { baseview::assume_standalone_in_process() };

    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
    .unwrap();

    let poll_sub_notifier = PollSubNotifier::new();

    let (window, _message_sender) = iced_baseview::create_window(
        IcedWindowSettings::new()
            .with_title("iced_baseview hello world")
            .with_size(WINDOW_SIZE),
        poll_sub_notifier.clone(),
        |window_size_sub| {
            Ok(application(
                move || MyProgram::new(window_size_sub.clone()),
                MyProgram::update,
                MyProgram::view,
            )
            .theme(MyProgram::theme)
            .subscription(MyProgram::subscription)
            .run())
        },
        None,
    )
    .unwrap();

    let run_thread = Arc::new(AtomicBool::new(true));
    let run_thread_1 = Arc::clone(&run_thread);
    let thread = std::thread::spawn(move || {
        while run_thread_1.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));

            // Trigger the poll sub notifier from any thread. This method is realtime-safe.
            poll_sub_notifier.notify();
        }
    });

    window.run_until_closed().unwrap();

    run_thread.store(false, Ordering::Relaxed);
    thread.join().unwrap();
}

#[derive(Debug, Clone)]
enum Message {
    Poll,
    WindowResized,
}

struct MyProgram {
    /// Used to retrieve the size of the window, including its size in physical pixels.
    ///
    /// This can be used to notify the host of the new window size.
    window_size: SharedWindowSize,
}

impl MyProgram {
    pub fn new(window_size: SharedWindowSize) -> Self {
        Self { window_size }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            // This subscription is trigged by the `PollSubNotifier`. This can be used,
            // for example, to notify the GUI that parameters have changed or to notify
            // the GUI that it should update its decibel meter.
            iced_baseview::poll_events().map(|_| Message::Poll),
            // A subscription that triggers when the window is resized.
            iced_baseview::window_resized().map(|_| Message::WindowResized),
        ])
    }

    pub fn theme(&self) -> Option<Theme> {
        Some(iced_baseview::Theme::Dark)
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Poll => {
                println!("polling...");
            }
            Message::WindowResized => {
                let size = self.window_size.get();
                println!("window resized: {:?}", size);
            }
        }
    }

    pub fn view(&self) -> Container<'_, Message> {
        let content = Column::new()
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .push(Text::new("🥪"));

        Container::new(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
    }
}
