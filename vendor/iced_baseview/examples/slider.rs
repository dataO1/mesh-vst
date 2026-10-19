use iced_baseview::{
    Center, Fill, IcedWindowSettings, PollSubNotifier, Task, Theme, application,
    baseview::dpi::LogicalSize,
    widget::{Column, column, container, slider, text, vertical_slider},
};

const WINDOW_SIZE: LogicalSize<f32> = LogicalSize::new(500.0, 300.0);
const ZOOM_FACTOR: f32 = 1.5;

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

    let (window, _message_sender) = iced_baseview::create_window(
        IcedWindowSettings::new()
            .with_title("iced_baseview slider")
            .with_size(WINDOW_SIZE)
            .with_scale_factor(ZOOM_FACTOR),
        PollSubNotifier::new(),
        |_window_size_sub| {
            Ok(
                application(MyProgram::default, MyProgram::update, MyProgram::view)
                    .theme(MyProgram::theme)
                    .scale_factor(|_| ZOOM_FACTOR)
                    .run(),
            )
        },
        None,
    )
    .unwrap();

    window.run_until_closed().unwrap();
}

#[derive(Debug, Clone, Copy)]
enum Message {
    SliderChanged(f32),
}

#[derive(Default)]
struct MyProgram {
    value: f32,
}

impl MyProgram {
    pub fn theme(&self) -> Option<Theme> {
        Some(iced_baseview::Theme::Dark)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SliderChanged(value) => {
                self.value = value;
            }
        }

        Task::none()
    }

    pub fn view(&self) -> Column<'_, Message> {
        let h_slider = container(
            slider(0.0..=1.0, self.value, Message::SliderChanged)
                .default(0.5f32)
                .step(0.01f32)
                .shift_step(0.1f32),
        )
        .width(250);

        let v_slider = container(
            vertical_slider(0.0..=1.0, self.value, Message::SliderChanged)
                .default(0.5f32)
                .step(0.01f32)
                .shift_step(0.1f32),
        )
        .height(200);

        let text = text(format!("{:.2}", self.value));

        column![v_slider, h_slider, text,]
            .width(Fill)
            .align_x(Center)
            .spacing(20)
            .padding(20)
            .into()
    }
}
