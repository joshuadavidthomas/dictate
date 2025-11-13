//! OSD Application using iced_layershell framework

use iced::time::{self, Duration as IcedDuration};
use iced::widget::{container, horizontal_space, mouse_area, row, text};
use iced::{window, Center, Color, Element, Length, Shadow, Subscription, Task, Vector};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings};
use iced_layershell::to_layer_message;
use iced_runtime::{task, Action};
use iced_runtime::window::Action as WindowAction;
use std::time::Instant;

use super::colors;
use super::socket::{OsdMessage, OsdSocket};
use super::state::{OsdState, state_visual};
use super::widgets::{status_dot, spectrum_waveform};
use crate::protocol::State;
use crate::text::TextInserter;

/// Configuration for transcription session
#[derive(Debug, Clone)]
pub struct TranscriptionConfig {
    pub max_duration: u64,
    pub silence_duration: u64,
    pub sample_rate: u32,
    pub insert: bool,
    pub copy: bool,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            max_duration: 30,
            silence_duration: 2,
            sample_rate: 16000,
            insert: false,
            copy: false,
        }
    }
}

pub struct OsdApp {
    state: OsdState,
    socket: OsdSocket,
    cached_visual: super::state::OsdVisual,
    window_id: Option<window::Id>,
    config: TranscriptionConfig,
    text_inserter: TextInserter,
    transcription_initiated: bool,
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Message {
    SocketMessage(OsdMessage),
    Tick,
    SocketError(String),
    MouseEntered,
    MouseExited,
    InitiateTranscription,
    Exit,
}

/// Initialization function for daemon pattern
pub fn new_osd_app(socket_path: &str, config: TranscriptionConfig) -> (OsdApp, Task<Message>) {
    let mut socket = OsdSocket::new(socket_path.to_string());

    // Try to connect immediately
    if let Err(e) = socket.connect() {
        eprintln!("OSD: Initial socket connection failed: {}", e);
    }

    let mut state = OsdState::new();
    let cached_visual = state.tick(Instant::now());

    (
        OsdApp {
            state,
            socket,
            cached_visual,
            window_id: None, // No window initially
            config,
            text_inserter: TextInserter::new(),
            transcription_initiated: false,
        },
        Task::done(Message::InitiateTranscription),
    )
}

/// Namespace function for daemon pattern
pub fn namespace(_state: &OsdApp) -> String {
    String::from("Dictate OSD")
}

/// Update function for daemon pattern
pub fn update(state: &mut OsdApp, message: Message) -> Task<Message> {
    let had_window_before = state.window_id.is_some();

    match message {
        Message::SocketMessage(msg) => {
            state.handle_socket_message(msg);
        }
        Message::Tick => {
            // Check for timeout (no messages for 15 seconds)
            if state.state.has_timeout() {
                eprintln!("OSD: Timeout - no messages for 15 seconds");
                state.state.set_error();
            }

            // Safety fallback: If we're hovering but haven't seen ANY mouse event recently,
            // the mouse probably left but we didn't get the exit event. Only reset after
            // a reasonable delay that's long enough for actual hovering use.
            if state.state.is_mouse_hovering 
                && state.state.last_mouse_event.elapsed() > std::time::Duration::from_secs(30) {
                eprintln!("OSD: Resetting stale mouse hover state (no mouse movement for 30s - assuming left)");
                state.state.is_mouse_hovering = false;
            }

            // Try to reconnect if needed
            if state.socket.should_reconnect(Instant::now()) {
                eprintln!("OSD: Attempting to reconnect...");
                match state.socket.connect() {
                    Ok(_) => eprintln!("OSD: Reconnected successfully"),
                    Err(e) => {
                        eprintln!("OSD: Reconnection failed: {}", e);
                        state.socket.schedule_reconnect();
                    }
                }
            }

            // Try to read socket messages
            loop {
                match state.socket.read_message() {
                    Ok(Some(msg)) => state.handle_socket_message(msg),
                    Ok(None) => break, // No more messages
                    Err(e) => {
                        eprintln!("OSD: Socket read error: {}", e);
                        state.socket.schedule_reconnect();
                        state.state.set_error();
                        break;
                    }
                }
            }

            // Update cached visual state for rendering
            state.cached_visual = state.state.tick(Instant::now());
            
            // Check if we should auto-exit (linger expired and not hovering)
            if state.state.check_auto_exit() {
                eprintln!("OSD: Auto-exit condition met");
                return Task::done(Message::Exit);
            }
        }
        Message::SocketError(err) => {
            eprintln!("OSD: Socket error: {}", err);
            state.state.set_error();
        }
        Message::MouseEntered => {
            eprintln!("OSD: Mouse entered window (state={:?}, disappearing={}, needs_window={})",
                state.state.state, state.state.is_window_disappearing, state.state.needs_window());
            state.state.is_mouse_hovering = true;
            state.state.last_mouse_event = Instant::now();
        }
        Message::MouseExited => {
            eprintln!("OSD: Mouse exited window (state={:?}, disappearing={}, needs_window={})",
                state.state.state, state.state.is_window_disappearing, state.state.needs_window());
            state.state.is_mouse_hovering = false;
            state.state.last_mouse_event = Instant::now();
        }
        Message::InitiateTranscription => {
            if !state.transcription_initiated {
                eprintln!("OSD: Sending transcribe request - max_duration={}, silence_duration={}, sample_rate={}", 
                    state.config.max_duration, state.config.silence_duration, state.config.sample_rate);
                match state.socket.send_transcribe(
                    state.config.max_duration,
                    state.config.silence_duration,
                    state.config.sample_rate,
                ) {
                    Ok(_) => {
                        eprintln!("OSD: Transcribe request sent successfully");
                        state.transcription_initiated = true;
                    }
                    Err(e) => {
                        eprintln!("OSD: Failed to send transcription request: {}", e);
                        state.state.set_error();
                    }
                }
            }
        }
        Message::Exit => {
            eprintln!("OSD: Initiating clean shutdown");
            // Close window first if it exists
            if let Some(id) = state.window_id.take() {
                eprintln!("OSD: Closing window before exit");
                // Schedule exit after window has time to close
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    eprintln!("OSD: Exiting process");
                    std::process::exit(0);
                });
                return task::effect(Action::Window(WindowAction::Close(id)));
            }
            // No window, exit immediately
            eprintln!("OSD: No window to close, exiting immediately");
            std::process::exit(0);
        }
        _ => {
            // All other messages (NewLayerShell, etc.) are handled by the framework
        }
    }

    // Check for state transitions that require window management

    if state.state.should_create_window(had_window_before) {
        // Start appearing animation
        state.state.start_appearing_animation();

        // Create window
        let id = window::Id::unique();
        state.window_id = Some(id);

        eprintln!("OSD: Creating window with fade-in animation for state {:?}", state.state.state);

        return Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                size: Some((440, 56)),
                exclusive_zone: None,
                anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                layer: Layer::Overlay,
                margin: Some((10, 0, 0, 0)),
                keyboard_interactivity: KeyboardInteractivity::None,
                use_last_output: false,
                ..Default::default()
            },
            id,
        });
    } else if state.state.should_start_disappearing(had_window_before) {
        // Start disappearing animation (don't close window yet)
        state.state.start_disappearing_animation();
        eprintln!("OSD: Starting fade-out animation");
    } else if state.state.should_close_window() && had_window_before {
        // Animation finished - now actually close window
        if let Some(id) = state.window_id.take() {
            // Reset disappearing flag and clear linger so window doesn't come back
            state.state.is_window_disappearing = false;
            state.state.linger_until = None;
            eprintln!("OSD: Destroying window (fade-out complete)");
            return task::effect(Action::Window(WindowAction::Close(id)));
        }
    }

    Task::none()
}

/// View function for daemon pattern
pub fn view(state: &OsdApp, id: window::Id) -> Element<'_, Message> {
    // Verify this is our window
    if state.window_id != Some(id) {
        return container(text("")).into();
    }

    let visual = &state.cached_visual;

    // Calculate scaled dimensions for animation
    let base_width = 420.0;
    let base_height = 36.0;
    let scaled_width = base_width * visual.window_scale;
    let scaled_height = base_height * visual.window_scale;

    // Apply window opacity to background (alpha is f32 0.0-1.0)
    let bg_alpha = 0.94 * visual.window_opacity;
    let shadow_alpha = 0.35 * visual.window_opacity;

    // Check if we're showing completion flash
    if state.state.completion_action.is_some() {
        // Show completion message with green dot
        let dot = status_dot(8.0, colors::GREEN);

        let completion_text = text("Done")
            .size(14)
            .color(colors::LIGHT_GRAY);
        
        let content = row![
            dot,
            text(" ").size(4),
            completion_text,
        ]
        .spacing(8)
        .padding([6, 12])
        .align_y(Center);
        
        let styled_bar = container(content)
            .width(Length::Fixed(scaled_width))
            .height(Length::Fixed(scaled_height))
            .center_y(scaled_height)
            .style(move |_theme| container::Style {
                background: Some(colors::with_alpha(colors::DARK_GRAY, bg_alpha).into()),
                border: iced::Border {
                    radius: (12.0 * visual.window_scale).into(),
                    ..Default::default()
                },
                shadow: Shadow {
                    color: colors::with_alpha(colors::BLACK, shadow_alpha),
                    offset: Vector::new(0.0, 2.0),
                    blur_radius: 12.0,
                },
                ..Default::default()
            });
        
        return container(styled_bar)
            .padding(10)
            .center(Length::Fill)
            .into();
    }

    // Normal state display (recording/transcribing)
    // Status dot with color and alpha (pulsing)
    // Override to yellow/orange when near recording limit (25s+)
    let base_color = if visual.is_near_limit {
        colors::ORANGE
    } else {
        visual.color
    };
    
    let dot_color = Color {
        r: base_color.r,
        g: base_color.g,
        b: base_color.b,
        a: visual.alpha,
    };
    let dot = status_dot(8.0, dot_color);

    // Base color without alpha pulse (for waveform)
    let base_color = Color {
        r: visual.color.r,
        g: visual.color.g,
        b: visual.color.b,
        a: 1.0, // Full opacity for waveform
    };

    // Status text
    let status_text = text(visual.state.ui_label())
        .size(14)
        .color(colors::LIGHT_GRAY);

    // Timer text (only during recording, blink colon only)
    let timer_text = if visual.state == State::Recording && visual.recording_elapsed_secs.is_some() {
        let elapsed = visual.recording_elapsed_secs.unwrap();
        // Blink the colon only, not the entire timer
        let timer_str = format_duration(elapsed, visual.timer_visible);

        Some(text(timer_str)
            .size(14)
            .color(colors::LIGHT_GRAY))
    } else {
        None
    };

    // Spectrum waveform (only during recording)
    let show_waveform = visual.state == State::Recording;

    let content = if show_waveform {
        let wave = spectrum_waveform(visual.spectrum_bands, base_color);
        if let Some(timer) = timer_text {
            row![
                dot,
                text(" ").size(4), // Small spacer
                status_text,
                horizontal_space(),
                timer,
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
                horizontal_space(),
                wave
            ]
            .spacing(8)
            .padding([6, 12]) // Reduced vertical padding: 6px top/bottom, 12px left/right
            .align_y(Center)
        }
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

    // Inner container with background, border, and shadow - with animation
    let styled_bar = container(content)
        .width(Length::Fixed(scaled_width))
        .height(Length::Fixed(scaled_height))
        .center_y(scaled_height) // Center content vertically in the bar
        .style(move |_theme| container::Style {
            background: Some(colors::with_alpha(colors::DARK_GRAY, bg_alpha).into()),
            border: iced::Border {
                radius: (12.0 * visual.window_scale).into(),
                ..Default::default()
            },
            shadow: Shadow {
                color: colors::with_alpha(colors::BLACK, shadow_alpha),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 12.0,
            },
            ..Default::default()
        });

    // Wrap the styled bar directly with mouse_area FIRST, before outer container
    // This ensures mouse events track the actual visual bounds of the widget
    let interactive_bar = mouse_area(styled_bar)
        .on_enter(Message::MouseEntered)
        .on_exit(Message::MouseExited);

    // Then wrap in outer container with padding for shadow space
    container(interactive_bar)
        .padding(10) // Padding to give shadow room to render
        .center(Length::Fill)
        .into()
}

/// Subscription function for daemon pattern
pub fn subscription(_state: &OsdApp) -> Subscription<Message> {
    // Animation tick subscription (60 FPS for smooth animations)
    time::every(IcedDuration::from_millis(16)).map(|_| Message::Tick)
}

/// Remove window ID when window is closed
pub fn remove_id(state: &mut OsdApp, id: window::Id) {
    if state.window_id == Some(id) {
        eprintln!("OSD: Window removed: {:?}", id);
        state.window_id = None;
    }
}

/// Style function for daemon pattern
pub fn style(_state: &OsdApp, _theme: &iced::Theme) -> iced_layershell::Appearance {
    iced_layershell::Appearance {
        background_color: Color::TRANSPARENT,
        text_color: colors::LIGHT_GRAY,
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
                ts,
            } => {
                eprintln!("OSD: Received Status - state={:?}, level={}, idle_hot={}, ts={}", state, level, idle_hot, ts);
                self.state.update_state(state, idle_hot, ts);
                self.state.update_level(level, ts);
            }
            OsdMessage::State { state, idle_hot, ts } => {
                eprintln!("OSD: Received State - state={:?}, idle_hot={}, ts={}", state, idle_hot, ts);
                self.state.update_state(state, idle_hot, ts);
            }
            OsdMessage::Level { v, ts } => {
                eprintln!("OSD: Received Level - v={}, ts={}", v, ts);
                self.state.update_level(v, ts);
            }
            OsdMessage::Spectrum { bands, ts } => {
                eprintln!("OSD: Received Spectrum - bands={:?}, ts={}", bands, ts);
                self.state.update_spectrum(bands, ts);
            }
            OsdMessage::TranscriptionResult { text, duration, model } => {
                eprintln!("OSD: Received transcription result - text='{}', duration={}, model={}", text, duration, model);
                
                // Store the result
                self.state.set_transcription_result(text.clone());
                
                // Determine what action to take and show corresponding completion message
                let completion_action = if self.config.insert {
                    match self.text_inserter.insert_text(&text) {
                        Ok(()) => {
                            eprintln!("OSD: Text inserted at cursor position");
                            super::state::CompletionAction::Inserted
                        }
                        Err(e) => {
                            eprintln!("OSD: Failed to insert text: {}", e);
                            super::state::CompletionAction::Printed
                        }
                    }
                } else if self.config.copy {
                    match self.text_inserter.copy_to_clipboard(&text) {
                        Ok(()) => {
                            eprintln!("OSD: Text copied to clipboard");
                            super::state::CompletionAction::Copied
                        }
                        Err(e) => {
                            eprintln!("OSD: Failed to copy to clipboard: {}", e);
                            super::state::CompletionAction::Printed
                        }
                    }
                } else {
                    println!("{}", text);
                    super::state::CompletionAction::Printed
                };
                
                // Set completion action to trigger flash and exit timer
                self.state.set_completion_action(completion_action);
            }
            OsdMessage::Error { error } => {
                eprintln!("OSD: Received error from server: {}", error);
                self.state.set_error();
            }
        }
    }
}

/// Format duration as M:SS or M SS (blink colon only)
/// When show_colon=true: "0:05", when false: "0 05"
fn format_duration(seconds: u32, show_colon: bool) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    let separator = if show_colon { ":" } else { " " };
    format!("{}{}{:02}", mins, separator, secs)
}
