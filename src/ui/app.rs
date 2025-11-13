use iced::time::{self, Duration as IcedDuration};
use iced::widget::{container, text};
use iced::{Color, Element, Subscription, Task, window};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings};
use iced_layershell::to_layer_message;
use iced_runtime::window::Action as WindowAction;
use iced_runtime::{Action, task};
use std::time::Instant;

use super::colors;
use super::socket::{OsdSocket, SocketMessage};
use super::state::OsdState;
use super::widgets::{osd_bar, OsdBarStyle};
use crate::protocol::{Event, Response};
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
    Event(Event),
    Response(Response),
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
        Message::Event(event) => {
            state.handle_event(event);
        }
        Message::Response(response) => {
            state.handle_response(response);
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
                && state.state.last_mouse_event.elapsed() > std::time::Duration::from_secs(30)
            {
                eprintln!(
                    "OSD: Resetting stale mouse hover state (no mouse movement for 30s - assuming left)"
                );
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
                    Ok(Some(SocketMessage::Event(event))) => state.handle_event(event),
                    Ok(Some(SocketMessage::Response(response))) => state.handle_response(response),
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
            eprintln!(
                "OSD: Mouse entered window (state={:?}, disappearing={}, needs_window={})",
                state.state.state,
                state.state.is_window_disappearing,
                state.state.needs_window()
            );
            state.state.is_mouse_hovering = true;
            state.state.last_mouse_event = Instant::now();
        }
        Message::MouseExited => {
            eprintln!(
                "OSD: Mouse exited window (state={:?}, disappearing={}, needs_window={})",
                state.state.state,
                state.state.is_window_disappearing,
                state.state.needs_window()
            );
            state.state.is_mouse_hovering = false;
            state.state.last_mouse_event = Instant::now();
        }
        Message::InitiateTranscription => {
            if !state.transcription_initiated {
                eprintln!(
                    "OSD: Sending transcribe request - max_duration={}, silence_duration={}, sample_rate={}",
                    state.config.max_duration,
                    state.config.silence_duration,
                    state.config.sample_rate
                );
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

        eprintln!(
            "OSD: Creating window with fade-in animation for state {:?}",
            state.state.state
        );

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

    // Create styling configuration for OSD bar
    let bar_style = OsdBarStyle {
        width: 420.0,
        height: 36.0,
        window_scale: visual.window_scale,
        window_opacity: visual.window_opacity,
    };

    osd_bar(
        visual.state,
        visual.color,
        visual.alpha,
        visual.recording_elapsed_secs,
        visual.current_ts,
        visual.spectrum_bands,
        bar_style,
        Message::MouseEntered,
        Message::MouseExited,
    )
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
    /// Handle incoming event from the server
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Status {
                state,
                level,
                idle_hot,
                ts,
                ..
            } => {
                eprintln!(
                    "OSD: Received Status - state={:?}, level={}, idle_hot={}, ts={}",
                    state, level, idle_hot, ts
                );
                self.state.update_state(state, idle_hot, ts);
                self.state.update_level(level, ts);
            }
            Event::State {
                state,
                idle_hot,
                ts,
                ..
            } => {
                eprintln!(
                    "OSD: Received State - state={:?}, idle_hot={}, ts={}",
                    state, idle_hot, ts
                );
                self.state.update_state(state, idle_hot, ts);
            }
            Event::Level { v, ts, .. } => {
                eprintln!("OSD: Received Level - v={}, ts={}", v, ts);
                self.state.update_level(v, ts);
            }
            Event::Spectrum { bands, ts, .. } => {
                eprintln!("OSD: Received Spectrum - bands={:?}, ts={}", bands, ts);
                self.state.update_spectrum(bands, ts);
            }
        }
    }

    /// Handle incoming response from the server
    fn handle_response(&mut self, response: Response) {
        match response {
            Response::Result {
                text,
                duration,
                model,
                ..
            } => {
                eprintln!(
                    "OSD: Received transcription result - text='{}', duration={}, model={}",
                    text, duration, model
                );

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
            Response::Error { error, .. } => {
                eprintln!("OSD: Received error from server: {}", error);
                self.state.set_error();
            }
            _ => {
                // Ignore other response types (Status, Subscribed)
            }
        }
    }
}
