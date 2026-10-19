use iced_baseview::{
    Alignment, IcedWindowSettings, Length, PollSubNotifier, Theme, application,
    baseview::dpi::LogicalSize,
    widget::{Column, Container, Text},
};

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
            .with_title("iced_baseview hello world")
            .with_size(LogicalSize::new(500.0, 300.0)),
        PollSubNotifier::new(),
        |_window_size_sub| {
            Ok(
                application(MyProgram::default, MyProgram::update, MyProgram::view)
                    .theme(MyProgram::theme)
                    .run(),
            )
        },
        None,
    )
    .unwrap();

    window.run_until_closed().unwrap();
}

#[derive(Default)]
struct MyProgram;

impl MyProgram {
    pub fn theme(&self) -> Option<Theme> {
        Some(iced_baseview::Theme::Dark)
    }

    pub fn update(&mut self, _message: ()) {}

    pub fn view(&self) -> Container<'_, ()> {
        let content = Column::new()
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .push(Text::new("Hello World!"));

        Container::new(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .into()
    }
}
