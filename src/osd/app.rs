//! OSD Application using iced_layershell framework

use iced::time::{self, Duration as IcedDuration};
use iced::widget::{container, horizontal_space, row, text};
use iced::{Center, Color, Element, Length, Shadow, Subscription, Task, Theme, Vector};
use iced_layershell::{actions::LayershellCustomActions, Application};
use std::time::Instant;

use super::socket::{OsdMessage, OsdSocket};
use super::state::{OsdState, State as OsdStateEnum};
use super::widgets::{status_dot, waveform};

pub struct OsdApp {
    state: OsdState,
    socket: OsdSocket,
    cached_visual: super::state::OsdVisual,
}

#[derive(Debug, Clone)]
pub enum Message {
    SocketMessage(OsdMessage),
    Tick,
    SocketError(String),
}

// Required for iced_layershell to convert messages to layer actions
impl TryFrom<Message> for LayershellCustomActions {
    type Error = Message;

    fn try_from(message: Message) -> Result<Self, Self::Error> {
        // No layer-specific actions in our messages
        Err(message)
    }
}

impl Application for OsdApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = String;

    fn new(socket_path: String) -> (Self, Task<Self::Message>) {
        let mut socket = OsdSocket::new(socket_path.clone());
        
        // Try to connect immediately
        if let Err(e) = socket.connect() {
            eprintln!("OSD: Initial socket connection failed: {}", e);
        }

        let mut state = OsdState::new();
        let cached_visual = state.tick(Instant::now());

        (
            Self {
                state,
                socket,
                cached_visual,
            },
            Task::none(),
        )
    }

    fn namespace(&self) -> String {
        String::from("Dictate OSD")
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::SocketMessage(msg) => {
                self.handle_socket_message(msg);
            }
            Message::Tick => {
                // Check for timeout (no messages for 15 seconds)
                if self.state.has_timeout() {
                    eprintln!("OSD: Timeout - no messages for 15 seconds");
                    self.state.set_error();
                }
                
                // Try to reconnect if needed
                if self.socket.should_reconnect(Instant::now()) {
                    eprintln!("OSD: Attempting to reconnect...");
                    match self.socket.connect() {
                        Ok(_) => eprintln!("OSD: Reconnected successfully"),
                        Err(e) => {
                            eprintln!("OSD: Reconnection failed: {}", e);
                            self.socket.schedule_reconnect();
                        }
                    }
                }
                
                // Try to read socket messages
                loop {
                    match self.socket.read_message() {
                        Ok(Some(msg)) => self.handle_socket_message(msg),
                        Ok(None) => break, // No more messages
                        Err(e) => {
                            eprintln!("OSD: Socket read error: {}", e);
                            self.socket.schedule_reconnect();
                            self.state.set_error();
                            break;
                        }
                    }
                }
                
                // Update cached visual state for rendering
                self.cached_visual = self.state.tick(Instant::now());
            }
            Message::SocketError(err) => {
                eprintln!("OSD: Socket error: {}", err);
                self.state.set_error();
            }
        }
        
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let visual = &self.cached_visual;

        // Status dot with color and alpha
        let dot_color = Color {
            r: visual.color.r,
            g: visual.color.g,
            b: visual.color.b,
            a: visual.alpha,
        };
        let dot = status_dot(8.0, dot_color);

        // Status text
        let status_text = text(state_label(visual.state))
            .size(14)
            .color(Color::from_rgb8(200, 200, 200));

        // Waveform (only during recording)
        let show_waveform = visual.state == OsdStateEnum::Recording;
        
        let content = if show_waveform {
            let wave = waveform(visual.level_bars, dot_color);
            row![
                dot,
                text(" ").size(4), // Small spacer
                status_text,
                horizontal_space(),
                wave
            ]
            .spacing(8)
            .padding([6, 12]) // Reduced vertical padding: 6px top/bottom, 12px left/right
            .align_y(Center)
        } else {
            row![
                dot,
                text(" ").size(4), // Small spacer
                status_text,
            ]
            .spacing(8)
            .padding([6, 12]) // Reduced vertical padding: 6px top/bottom, 12px left/right
            .align_y(Center)
        };

        // Inner container with background, border, and shadow
        let styled_bar = container(content)
            .width(Length::Fixed(420.0))
            .height(Length::Fixed(36.0))
            .center_y(36.0) // Center content vertically in the bar
            .style(|_theme| container::Style {
                background: Some(
                    Color::from_rgba8(30, 30, 30, 0.94).into()
                ),
                border: iced::Border {
                    radius: 12.0.into(),
                    ..Default::default()
                },
                shadow: Shadow {
                    color: Color::from_rgba8(0, 0, 0, 0.35),
                    offset: Vector::new(0.0, 2.0),
                    blur_radius: 12.0,
                },
                ..Default::default()
            });

        // Outer container with padding for shadow space
        container(styled_bar)
            .padding(10) // Padding to give shadow room to render
            .center(Length::Fill)
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        // Animation tick subscription (60 FPS for smooth animations)
        time::every(IcedDuration::from_millis(16)).map(|_| Message::Tick)
    }

    fn theme(&self) -> Self::Theme {
        Theme::Dark
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        iced_layershell::Appearance {
            background_color: Color::TRANSPARENT,
            text_color: Color::from_rgb8(200, 200, 200),
        }
    }
}

impl OsdApp {
    /// Handle incoming socket message
    fn handle_socket_message(&mut self, msg: OsdMessage) {
        match msg {
            OsdMessage::Status {
                state,
                level,
                idle_hot,
                ..
            } => {
                eprintln!("OSD: Received Status - state='{}', level={}, idle_hot={}", state, level, idle_hot);
                let osd_state = parse_state(&state);
                eprintln!("OSD: Parsed state: {:?}", osd_state);
                self.state.update_state(osd_state, idle_hot);
                self.state.update_level(level);
            }
            OsdMessage::State { state, idle_hot, .. } => {
                eprintln!("OSD: Received State - state='{}', idle_hot={}", state, idle_hot);
                let osd_state = parse_state(&state);
                eprintln!("OSD: Parsed state: {:?}", osd_state);
                self.state.update_state(osd_state, idle_hot);
            }
            OsdMessage::Level { v, .. } => {
                eprintln!("OSD: Received Level - v={}", v);
                self.state.update_level(v);
            }
        }
    }
}

/// Parse state string to enum
fn parse_state(state: &str) -> OsdStateEnum {
    match state {
        "Idle" => OsdStateEnum::Idle,
        "Recording" => OsdStateEnum::Recording,
        "Transcribing" => OsdStateEnum::Transcribing,
        "Error" => OsdStateEnum::Error,
        _ => OsdStateEnum::Idle,
    }
}

/// Get human-readable label for state
fn state_label(state: OsdStateEnum) -> &'static str {
    match state {
        OsdStateEnum::Idle => "Ready",
        OsdStateEnum::Recording => "Recording",
        OsdStateEnum::Transcribing => "Transcribing",
        OsdStateEnum::Error => "Error",
    }
}
