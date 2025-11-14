use iced::time::{self, Duration as IcedDuration};
use iced::widget::{container, text};
use iced::{Color, Element, Subscription, Task, window};
use iced_layershell::build_pattern::MainSettings;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings};
use iced_layershell::settings::{LayerShellSettings, StartMode};
use iced_layershell::to_layer_message;
use iced_runtime::window::Action as WindowAction;
use iced_runtime::{Action, task};
use std::time::Instant;

use super::colors;
use super::widgets::{OsdBarStyle, osd_bar};
use crate::audio::SPECTRUM_BANDS;
use crate::protocol::{ClientMessage, ServerMessage, State};
use crate::text::TextInserter;
use crate::transport::{SyncTransport, decode_server_message};

/// Current OSD state for rendering
#[derive(Debug, Clone)]
pub struct OsdState {
    pub state: State,
    pub idle_hot: bool,
    pub alpha: f32,
    pub spectrum_bands: [f32; SPECTRUM_BANDS],
    pub window_opacity: f32,                 // 0.0 → 1.0 for fade animation
    pub window_scale: f32,                   // 0.5 → 1.0 for expand/shrink animation
    pub recording_elapsed_secs: Option<u32>, // Elapsed seconds while recording
    pub current_ts: u64,                     // Current timestamp in milliseconds
    // TODO: Add warning UI for long recordings (>10 minutes) to inform user about memory usage
    // TODO: Add progress indicator for very long recordings
}

/// Configuration for transcription session
#[derive(Debug, Clone)]
pub struct TranscriptionConfig {
    pub max_duration: u64,
    pub silence_duration: u64,
    pub sample_rate: u32,
    pub insert: bool,
    pub copy: bool,
}

/// Mode of transcription initiation
#[derive(Debug, Clone)]
pub enum TranscriptionMode {
    /// One-shot transcription with silence detection
    Transcribe,
    /// Observer mode - UI just displays, doesn't send commands (server-spawned)
    Observer,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            max_duration: 0,  // 0 = unlimited duration (relies on silence detection)
            silence_duration: 2,
            sample_rate: 16000,
            insert: false,
            copy: false,
        }
    }
}

pub struct OsdApp {
    // Protocol state & data (from Osd)
    state: State,
    idle_hot: bool,
    state_pulse: Option<super::animation::PulseTween>,
    spectrum_buffer: super::buffer::SpectrumRingBuffer,
    last_message: Instant,
    linger_until: Option<Instant>,
    window_tween: Option<super::animation::WindowTween>,
    is_window_disappearing: bool,
    is_mouse_hovering: bool,
    last_mouse_event: Instant,
    recording_start_ts: Option<u64>,
    current_ts: u64,
    transcription_result: Option<String>,

    // App infrastructure
    transport: SyncTransport,
    render_state: OsdState,
    window_id: Option<window::Id>,
    config: TranscriptionConfig,
    text_inserter: TextInserter,
    transcription_initiated: bool,
    transcription_mode: TranscriptionMode,
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    SocketError(String),
    MouseEntered,
    MouseExited,
    InitiateTranscription,
    Exit,
}

impl OsdApp {
    /// Create a new OsdApp instance
    pub fn new(socket_path: &str, config: TranscriptionConfig, mode: TranscriptionMode) -> (Self, Task<Message>) {
        let mut transport = SyncTransport::new(socket_path.to_string());

        // Try to connect and subscribe immediately
        if let Err(e) = Self::connect_and_subscribe(&mut transport) {
            eprintln!("OSD: Initial socket connection failed: {}", e);
        }

        let now = Instant::now();

        let mut app = OsdApp {
            // Protocol state & data
            state: crate::protocol::State::Idle,
            idle_hot: false,
            state_pulse: None,
            spectrum_buffer: super::buffer::SpectrumRingBuffer::new(),
            last_message: now,
            linger_until: None,
            window_tween: None,
            is_window_disappearing: false,
            is_mouse_hovering: false,
            last_mouse_event: now,
            recording_start_ts: None,
            current_ts: 0,
            transcription_result: None,

            // App infrastructure
            transport,
            render_state: OsdState {
                state: crate::protocol::State::Idle,
                idle_hot: false,
                alpha: 1.0,
                spectrum_bands: [0.0; SPECTRUM_BANDS],
                window_opacity: 1.0,
                window_scale: 1.0,
                recording_elapsed_secs: None,
                current_ts: 0,
            },
            window_id: None,
            config,
            text_inserter: TextInserter::new(),
            transcription_initiated: false,
            transcription_mode: mode,
        };

        // Initialize render state
        app.render_state = app.tick(now);

        (app, Task::done(Message::InitiateTranscription))
    }

    /// Connect to socket and subscribe to events
    fn connect_and_subscribe(transport: &mut SyncTransport) -> anyhow::Result<()> {
        // 1. Connect to socket
        transport.connect()?;

        // 2. Send subscribe request
        let subscribe_request = ClientMessage::new_subscribe();
        transport.send_request(&subscribe_request)?;

        // 3. Read acknowledgment
        let ack = transport
            .read_line()?
            .ok_or_else(|| anyhow::anyhow!("No acknowledgment received"))?;

        // Parse acknowledgment to verify subscription
        let message = decode_server_message(&ack)
            .map_err(|e| anyhow::anyhow!("Failed to decode message: {}", e))?;

        // Verify it's a Subscribed response
        match message {
            ServerMessage::Subscribed { .. } => Ok(()),
            _ => Err(anyhow::anyhow!("Failed to subscribe: unexpected response")),
        }
    }

    /// Settings for the daemon pattern
    pub fn settings() -> MainSettings {
        MainSettings {
            layer_settings: LayerShellSettings {
                size: None, // No initial window
                exclusive_zone: 0,
                anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                layer: Layer::Overlay,
                margin: (10, 0, 0, 0),
                start_mode: StartMode::Background, // KEY: No focus stealing!
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Namespace for the daemon pattern
    pub fn namespace(&self) -> String {
        String::from("Dictate OSD")
    }

    /// Update function for daemon pattern
    pub fn update(&mut self, message: Message) -> Task<Message> {
        let had_window_before = self.window_id.is_some();

        match message {
            Message::Tick => {
                // Check for timeout (no messages for 15 seconds)
                if self.has_timeout() {
                    eprintln!("OSD: Timeout - no messages for 15 seconds");
                    self.set_error();
                }

                // Safety fallback: If we're hovering but haven't seen ANY mouse event recently,
                // the mouse probably left but we didn't get the exit event. Only reset after
                // a reasonable delay that's long enough for actual hovering use.
                if self.is_mouse_hovering
                    && self.last_mouse_event.elapsed() > std::time::Duration::from_secs(30)
                {
                    eprintln!(
                        "OSD: Resetting stale mouse hover state (no mouse movement for 30s - assuming left)"
                    );
                    self.is_mouse_hovering = false;
                }

                // Try to reconnect if needed
                if self.transport.should_reconnect(Instant::now()) {
                    eprintln!("OSD: Attempting to reconnect...");
                    match Self::connect_and_subscribe(&mut self.transport) {
                        Ok(_) => eprintln!("OSD: Reconnected successfully"),
                        Err(e) => {
                            eprintln!("OSD: Reconnection failed: {}", e);
                            self.transport.schedule_reconnect();
                        }
                    }
                }

                // Try to read socket messages
                loop {
                    match self.transport.read_line() {
                        Ok(Some(line)) => {
                            match decode_server_message(&line) {
                                Ok(ServerMessage::StatusEvent {
                                    state,
                                    spectrum,
                                    idle_hot,
                                    ts,
                                    ..
                                }) => {
                                    self.update_state(state, idle_hot, ts);
                                    if let Some(bands) = spectrum {
                                        self.update_spectrum(bands, ts);
                                    }
                                }
                                Ok(ServerMessage::Result {
                                    text,
                                    duration,
                                    model,
                                    ..
                                }) => {
                                    eprintln!(
                                        "OSD: Received transcription result - text='{}', duration={}, model={}",
                                        text, duration, model
                                    );
                                    self.set_transcription_result(text.clone());

                                    // Perform action based on config
                                    if self.config.insert {
                                        match self.text_inserter.insert_text(&text) {
                                            Ok(()) => {
                                                eprintln!("OSD: Text inserted at cursor position");
                                            }
                                            Err(e) => {
                                                eprintln!("OSD: Failed to insert text: {}", e);
                                                println!("{}", text);
                                            }
                                        }
                                    } else if self.config.copy {
                                        match self.text_inserter.copy_to_clipboard(&text) {
                                            Ok(()) => {
                                                eprintln!("OSD: Text copied to clipboard");
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "OSD: Failed to copy to clipboard: {}",
                                                    e
                                                );
                                                println!("{}", text);
                                            }
                                        }
                                    } else {
                                        println!("{}", text);
                                    }

                                    // Exit immediately if not hovering
                                    if !self.is_mouse_hovering {
                                        eprintln!("OSD: Transcription complete, exiting");
                                        return Task::done(Message::Exit);
                                    } else {
                                        eprintln!(
                                            "OSD: Transcription complete but mouse hovering, keeping window open"
                                        );
                                    }
                                }
                                Ok(ServerMessage::Error { error, .. }) => {
                                    eprintln!("OSD: Received error from server: {}", error);
                                    self.set_error();
                                }
                                Ok(_) => {
                                    // Ignore other message types (Status, Subscribed)
                                }
                                Err(e) => {
                                    eprintln!("OSD: Failed to decode message: {}", e);
                                    self.transport.schedule_reconnect();
                                    self.set_error();
                                    break;
                                }
                            }
                        }
                        Ok(None) => break, // No more messages
                        Err(e) => {
                            eprintln!("OSD: Socket read error: {}", e);
                            self.transport.schedule_reconnect();
                            self.set_error();
                            break;
                        }
                    }
                }

                // Update cached visual state for rendering
                self.render_state = self.tick(Instant::now());
            }
            Message::SocketError(err) => {
                eprintln!("OSD: Socket error: {}", err);
                self.set_error();
            }
            Message::MouseEntered => {
                eprintln!(
                    "OSD: Mouse entered window (state={:?}, disappearing={}, needs_window={})",
                    self.state,
                    self.is_window_disappearing,
                    self.needs_window()
                );
                self.is_mouse_hovering = true;
                self.last_mouse_event = Instant::now();
            }
            Message::MouseExited => {
                eprintln!(
                    "OSD: Mouse exited window (state={:?}, disappearing={}, needs_window={})",
                    self.state,
                    self.is_window_disappearing,
                    self.needs_window()
                );
                self.is_mouse_hovering = false;
                self.last_mouse_event = Instant::now();
            }
            Message::InitiateTranscription => {
                if !self.transcription_initiated {
                    match &self.transcription_mode {
                        TranscriptionMode::Transcribe => {
                            eprintln!(
                                "OSD: Sending transcribe request - max_duration={}, silence_duration={}, sample_rate={}",
                                self.config.max_duration,
                                self.config.silence_duration,
                                self.config.sample_rate
                            );
                            let request = ClientMessage::new_transcribe(
                                self.config.max_duration,
                                self.config.silence_duration,
                                self.config.sample_rate,
                            );
                            
                            match self.transport.send_request(&request) {
                                Ok(_) => {
                                    eprintln!("OSD: Request sent successfully");
                                    self.transcription_initiated = true;
                                }
                                Err(e) => {
                                    eprintln!("OSD: Failed to send request: {}", e);
                                    self.set_error();
                                }
                            }
                        }
                        TranscriptionMode::Observer => {
                            // Observer mode: don't send any command, just wait for events
                            eprintln!("OSD: Observer mode - waiting for server events");
                            self.transcription_initiated = true;
                        }
                    }
                }
            }
            Message::Exit => {
                eprintln!("OSD: Initiating clean shutdown");
                // Close window first if it exists
                if let Some(id) = self.window_id.take() {
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

        if self.should_create_window(had_window_before) {
            // Start appearing animation
            self.start_appearing_animation();

            // Create window
            let id = window::Id::unique();
            self.window_id = Some(id);

            eprintln!(
                "OSD: Creating window with fade-in animation for state {:?}",
                self.state
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
        } else if self.should_start_disappearing(had_window_before) {
            // Start disappearing animation (don't close window yet)
            self.start_disappearing_animation();
            eprintln!("OSD: Starting fade-out animation");
        } else if self.should_close_window() && had_window_before {
            // Animation finished - now actually close window
            if let Some(id) = self.window_id.take() {
                // Reset disappearing flag and clear linger so window doesn't come back
                self.is_window_disappearing = false;
                self.linger_until = None;
                eprintln!("OSD: Destroying window (fade-out complete)");
                return task::effect(Action::Window(WindowAction::Close(id)));
            }
        }

        Task::none()
    }

    /// View function for daemon pattern
    pub fn view(&self, id: window::Id) -> Element<'_, Message> {
        // Verify this is our window
        if self.window_id != Some(id) {
            return container(text("")).into();
        }

        let visual = &self.render_state;

        // Create styling configuration for OSD bar
        let bar_style = OsdBarStyle {
            width: 420.0,
            height: 36.0,
            window_scale: visual.window_scale,
            window_opacity: visual.window_opacity,
        };

        osd_bar(
            visual,
            bar_style,
            Message::MouseEntered,
            Message::MouseExited,
        )
    }

    /// Subscription function for daemon pattern
    pub fn subscription(&self) -> Subscription<Message> {
        // Animation tick subscription (60 FPS for smooth animations)
        time::every(IcedDuration::from_millis(16)).map(|_| Message::Tick)
    }

    /// Remove window ID when window is closed
    pub fn remove_id(&mut self, id: window::Id) {
        if self.window_id == Some(id) {
            eprintln!("OSD: Window removed: {:?}", id);
            self.window_id = None;
        }
    }

    /// Style function for daemon pattern
    pub fn style(&self, _theme: &iced::Theme) -> iced_layershell::Appearance {
        iced_layershell::Appearance {
            background_color: Color::TRANSPARENT,
            text_color: colors::LIGHT_GRAY,
        }
    }

    /// Update state from server event
    pub fn update_state(&mut self, new_state: crate::protocol::State, idle_hot: bool, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;

        // Handle recording state transition
        if new_state == crate::protocol::State::Recording
            && self.state != crate::protocol::State::Recording
        {
            // Entering recording - start pulsing animation and clear lingering
            self.state_pulse = Some(super::animation::PulseTween::new());
            self.recording_start_ts = Some(ts);
            self.linger_until = None;
        } else if new_state != crate::protocol::State::Recording && self.state_pulse.is_some() {
            self.state_pulse = None;
            self.recording_start_ts = None;
        }

        // Handle transcribing state transition
        if new_state == crate::protocol::State::Transcribing
            && self.state != crate::protocol::State::Transcribing
        {
            // Entering transcribing - start pulse animation
            self.state_pulse = Some(super::animation::PulseTween::new());
            // Clear any lingering when starting a new transcription
            self.linger_until = None;
        } else if new_state != crate::protocol::State::Transcribing {
            // If transitioning away from Transcribing, check minimum display time
            if self.state == crate::protocol::State::Transcribing
                && let Some(pulse_tween) = &self.state_pulse
            {
                let elapsed = Instant::now().duration_since(pulse_tween.started_at);
                if elapsed < std::time::Duration::from_millis(500) {
                    // Don't transition yet - keep Transcribing state for minimum visibility
                    return;
                }
            }
            if self.state_pulse.is_some() {
                self.state_pulse = None;
            }
        }

        self.state = new_state;
        self.idle_hot = idle_hot;
    }

    /// Update spectrum bands
    pub fn update_spectrum(&mut self, bands: Vec<f32>, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;
        if bands.len() == SPECTRUM_BANDS {
            let bands_array: [f32; SPECTRUM_BANDS] =
                bands.try_into().unwrap_or([0.0; SPECTRUM_BANDS]);
            self.spectrum_buffer.push(bands_array);
        }
    }

    /// Set error state
    pub fn set_error(&mut self) {
        self.update_state(crate::protocol::State::Error, false, self.current_ts);
    }

    /// Store transcription result
    pub fn set_transcription_result(&mut self, text: String) {
        self.transcription_result = Some(text);
    }

    /// Tick tweens and return current state
    pub fn tick(&mut self, now: Instant) -> OsdState {
        // Get current alpha (for dot pulsing)
        let alpha = self
            .state_pulse
            .as_ref()
            .map(|tween| super::animation::pulse_alpha(tween, now))
            .unwrap_or(1.0);

        // Calculate recording timer
        let recording_elapsed_secs = if self.state == crate::protocol::State::Recording {
            if let Some(start_ts) = self.recording_start_ts {
                let elapsed_ms = self.current_ts.saturating_sub(start_ts);
                let elapsed_secs = (elapsed_ms / 1000) as u32;
                Some(elapsed_secs)
            } else {
                None
            }
        } else {
            None
        };

        // Calculate window tween values
        let (window_opacity, window_scale) = if let Some(ref tween) = self.window_tween {
            let (opacity, scale, complete) = super::animation::window_transition(tween, now);

            eprintln!(
                "OSD: Window tween {:?} - opacity={:.3}, scale={:.3}, complete={}",
                tween.direction, opacity, scale, complete
            );

            if complete {
                eprintln!("OSD: Window tween complete, clearing tween state");
                self.window_tween = None;
            }

            (opacity, scale)
        } else {
            (1.0, 1.0) // Fully visible, full scale
        };

        OsdState {
            state: self.state,
            idle_hot: self.idle_hot,
            alpha,
            spectrum_bands: self.spectrum_buffer.last_frame(),
            window_opacity,
            window_scale,
            recording_elapsed_secs,
            current_ts: self.current_ts,
        }
    }

    /// Check for timeout (no messages for 15 seconds)
    pub fn has_timeout(&self) -> bool {
        self.last_message.elapsed() > std::time::Duration::from_secs(15)
    }

    /// Returns true if current state requires a visible window
    pub fn needs_window(&self) -> bool {
        // Show window for Recording, Transcribing, Error
        matches!(
            self.state,
            crate::protocol::State::Recording
                | crate::protocol::State::Transcribing
                | crate::protocol::State::Error
        )
    }

    /// Returns true if we just transitioned to needing a window
    pub fn should_create_window(&self, had_window: bool) -> bool {
        self.needs_window() && !had_window
    }

    /// Start appearing tween
    pub fn start_appearing_animation(&mut self) {
        self.window_tween = Some(super::animation::WindowTween::new_appearing());
        self.is_window_disappearing = false;
    }

    /// Returns true if we should start disappearing tween
    pub fn should_start_disappearing(&self, had_window: bool) -> bool {
        !self.needs_window()
            && had_window
            && !self.is_window_disappearing
            && !self.is_mouse_hovering // Don't start disappearing if mouse is hovering
    }

    /// Start disappearing tween
    pub fn start_disappearing_animation(&mut self) {
        self.window_tween = Some(super::animation::WindowTween::new_disappearing());
        self.is_window_disappearing = true;
    }

    /// Returns true if disappearing tween is complete and we should close window
    pub fn should_close_window(&self) -> bool {
        // Close window if we're marked as disappearing but tween is done (cleared)
        self.is_window_disappearing && self.window_tween.is_none()
    }
}
