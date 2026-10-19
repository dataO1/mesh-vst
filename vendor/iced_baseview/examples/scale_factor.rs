use iced_baseview::{
    Center, IcedWindowSettings, PollSubNotifier, Task, Theme, application,
    baseview::dpi::LogicalSize,
    widget::{Column, button, column, row, text},
};

const WINDOW_SIZE: LogicalSize<f32> = LogicalSize::new(550.0, 200.0);
const MIN_WINDOW_SIZE: LogicalSize<f32> = LogicalSize::new(550.0, 150.0);

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

    let scale_factor = 1.0;

    let (window, _message_sender) = iced_baseview::create_window(
        IcedWindowSettings::new()
            .with_title("iced_baseview scale_factor")
            .with_size(WINDOW_SIZE)
            .with_min_size(Some(MIN_WINDOW_SIZE))
            .with_resizable(true)
            .with_scale_factor(scale_factor),
        PollSubNotifier::new(),
        move |_window_size_sub| {
            Ok(application(
                move || MyProgram { scale_factor },
                MyProgram::update,
                MyProgram::view,
            )
            .theme(MyProgram::theme)
            .scale_factor(MyProgram::scale_factor)
            .run())
        },
        None,
    )
    .unwrap();

    window.run_until_closed().unwrap();
}

#[derive(Debug, Clone, Copy)]
enum Message {
    SetScaleFactor(f32),
}

struct MyProgram {
    scale_factor: f32,
}

impl MyProgram {
    pub fn theme(&self) -> Option<Theme> {
        Some(iced_baseview::Theme::Dark)
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SetScaleFactor(scale_factor) => {
                self.scale_factor = scale_factor;
            }
        }

        Task::none()
    }

    pub fn view(&self) -> Column<'_, Message> {
        column![
            row![
                button("50%").on_press(Message::SetScaleFactor(0.5)),
                button("75%").on_press(Message::SetScaleFactor(0.75)),
                button("100%").on_press(Message::SetScaleFactor(1.0)),
                button("125%").on_press(Message::SetScaleFactor(1.25)),
                button("150%").on_press(Message::SetScaleFactor(1.5)),
                button("175%").on_press(Message::SetScaleFactor(1.75)),
                button("200%").on_press(Message::SetScaleFactor(2.0)),
            ]
            .spacing(10)
            .padding(20)
            .align_y(Center),
            text(format!("Current scale: {}", self.scale_factor)),
        ]
        .align_x(Center)
        .padding(20)
    }
}
