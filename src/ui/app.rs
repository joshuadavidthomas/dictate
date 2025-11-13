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
use super::widgets::{OsdBarStyle, osd_bar};
use crate::protocol::{Event, Response, State};
use crate::text::TextInserter;

/// Action taken with transcription result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionAction {
    Copied,
    Inserted,
    Printed,
}

/// Current OSD state for rendering
#[derive(Debug, Clone)]
pub struct OsdState {
    pub state: State,
    pub idle_hot: bool,
    pub alpha: f32,
    pub spectrum_bands: [f32; 8],
    pub window_opacity: f32,                 // 0.0 → 1.0 for fade animation
    pub window_scale: f32,                   // 0.5 → 1.0 for expand/shrink animation
    pub recording_elapsed_secs: Option<u32>, // Elapsed seconds while recording
    pub current_ts: u64,                     // Current timestamp in milliseconds
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
    // Protocol state & data (from Osd)
    state: State,
    idle_hot: bool,
    recording_state: Option<super::animation::RecordingState>,
    transcribing_state: Option<super::animation::TranscribingState>,
    level_buffer: super::animation::LevelRingBuffer,
    spectrum_buffer: super::animation::SpectrumRingBuffer,
    last_message: Instant,
    linger_until: Option<Instant>,
    window_animation: Option<super::animation::WindowAnimation>,
    is_window_disappearing: bool,
    is_mouse_hovering: bool,
    last_mouse_event: Instant,
    recording_start_ts: Option<u64>,
    current_ts: u64,
    transcription_result: Option<String>,
    should_auto_exit: bool,
    completion_action: Option<CompletionAction>,
    completion_started_at: Option<Instant>,

    // App infrastructure
    socket: OsdSocket,
    render_state: OsdState,
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

    let now = Instant::now();

    let mut app = OsdApp {
        // Protocol state & data
        state: crate::protocol::State::Idle,
        idle_hot: false,
        recording_state: None,
        transcribing_state: None,
        level_buffer: super::animation::LevelRingBuffer::new(),
        spectrum_buffer: super::animation::SpectrumRingBuffer::new(),
        last_message: now,
        linger_until: None,
        window_animation: None,
        is_window_disappearing: false,
        is_mouse_hovering: false,
        last_mouse_event: now,
        recording_start_ts: None,
        current_ts: 0,
        transcription_result: None,
        should_auto_exit: false,
        completion_action: None,
        completion_started_at: None,

        // App infrastructure
        socket,
        render_state: OsdState {
            state: crate::protocol::State::Idle,
            idle_hot: false,
            alpha: 1.0,
            spectrum_bands: [0.0; 8],
            window_opacity: 1.0,
            window_scale: 1.0,
            recording_elapsed_secs: None,
            current_ts: 0,
        },
        window_id: None,
        config,
        text_inserter: TextInserter::new(),
        transcription_initiated: false,
    };

    // Initialize render state
    app.render_state = app.tick(now);

    (app, Task::done(Message::InitiateTranscription))
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
            if state.has_timeout() {
                eprintln!("OSD: Timeout - no messages for 15 seconds");
                state.set_error();
            }

            // Safety fallback: If we're hovering but haven't seen ANY mouse event recently,
            // the mouse probably left but we didn't get the exit event. Only reset after
            // a reasonable delay that's long enough for actual hovering use.
            if state.is_mouse_hovering
                && state.last_mouse_event.elapsed() > std::time::Duration::from_secs(30)
            {
                eprintln!(
                    "OSD: Resetting stale mouse hover state (no mouse movement for 30s - assuming left)"
                );
                state.is_mouse_hovering = false;
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
                        state.set_error();
                        break;
                    }
                }
            }

            // Update cached visual state for rendering
            state.render_state = state.tick(Instant::now());

            // Check if we should auto-exit (linger expired and not hovering)
            if state.check_auto_exit() {
                eprintln!("OSD: Auto-exit condition met");
                return Task::done(Message::Exit);
            }
        }
        Message::SocketError(err) => {
            eprintln!("OSD: Socket error: {}", err);
            state.set_error();
        }
        Message::MouseEntered => {
            eprintln!(
                "OSD: Mouse entered window (state={:?}, disappearing={}, needs_window={})",
                state.state,
                state.is_window_disappearing,
                state.needs_window()
            );
            state.is_mouse_hovering = true;
            state.last_mouse_event = Instant::now();
        }
        Message::MouseExited => {
            eprintln!(
                "OSD: Mouse exited window (state={:?}, disappearing={}, needs_window={})",
                state.state,
                state.is_window_disappearing,
                state.needs_window()
            );
            state.is_mouse_hovering = false;
            state.last_mouse_event = Instant::now();
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
                        state.set_error();
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

    if state.should_create_window(had_window_before) {
        // Start appearing animation
        state.start_appearing_animation();

        // Create window
        let id = window::Id::unique();
        state.window_id = Some(id);

        eprintln!(
            "OSD: Creating window with fade-in animation for state {:?}",
            state.state
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
    } else if state.should_start_disappearing(had_window_before) {
        // Start disappearing animation (don't close window yet)
        state.start_disappearing_animation();
        eprintln!("OSD: Starting fade-out animation");
    } else if state.should_close_window() && had_window_before {
        // Animation finished - now actually close window
        if let Some(id) = state.window_id.take() {
            // Reset disappearing flag and clear linger so window doesn't come back
            state.is_window_disappearing = false;
            state.linger_until = None;
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

    let visual = &state.render_state;

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
                self.update_state(state, idle_hot, ts);
                self.update_level(level, ts);
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
                self.update_state(state, idle_hot, ts);
            }
            Event::Level { v, ts, .. } => {
                eprintln!("OSD: Received Level - v={}, ts={}", v, ts);
                self.update_level(v, ts);
            }
            Event::Spectrum { bands, ts, .. } => {
                eprintln!("OSD: Received Spectrum - bands={:?}, ts={}", bands, ts);
                self.update_spectrum(bands, ts);
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
                self.set_transcription_result(text.clone());

                // Determine what action to take and show corresponding completion message
                let completion_action = if self.config.insert {
                    match self.text_inserter.insert_text(&text) {
                        Ok(()) => {
                            eprintln!("OSD: Text inserted at cursor position");
                            CompletionAction::Inserted
                        }
                        Err(e) => {
                            eprintln!("OSD: Failed to insert text: {}", e);
                            CompletionAction::Printed
                        }
                    }
                } else if self.config.copy {
                    match self.text_inserter.copy_to_clipboard(&text) {
                        Ok(()) => {
                            eprintln!("OSD: Text copied to clipboard");
                            CompletionAction::Copied
                        }
                        Err(e) => {
                            eprintln!("OSD: Failed to copy to clipboard: {}", e);
                            CompletionAction::Printed
                        }
                    }
                } else {
                    println!("{}", text);
                    CompletionAction::Printed
                };

                // Set completion action to trigger flash and exit timer
                self.set_completion_action(completion_action);
            }
            Response::Error { error, .. } => {
                eprintln!("OSD: Received error from server: {}", error);
                self.set_error();
            }
            _ => {
                // Ignore other response types (Status, Subscribed)
            }
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
            self.recording_state = Some(super::animation::RecordingState::new());
            self.recording_start_ts = Some(ts);
            self.linger_until = None;
        } else if new_state != crate::protocol::State::Recording {
            self.recording_state = None;
            self.recording_start_ts = None;
        }

        // Handle transcribing state transition
        if new_state == crate::protocol::State::Transcribing
            && self.state != crate::protocol::State::Transcribing
        {
            // Entering transcribing - freeze current level
            let frozen_level = self.level_buffer.last_10()[9]; // Last sample
            self.transcribing_state = Some(super::animation::TranscribingState::new(frozen_level));
            // Clear any lingering when starting a new transcription
            self.linger_until = None;
        } else if new_state != crate::protocol::State::Transcribing {
            // If transitioning away from Transcribing, check minimum display time
            if self.state == crate::protocol::State::Transcribing
                && let Some(trans_state) = &self.transcribing_state
            {
                let elapsed = Instant::now().duration_since(trans_state.started_at());
                if elapsed < std::time::Duration::from_millis(500) {
                    // Don't transition yet - keep Transcribing state for minimum visibility
                    return;
                }

                // No linger needed - completion flash will be shown instead
            }
            self.transcribing_state = None;
        }

        self.state = new_state;
        self.idle_hot = idle_hot;
    }

    /// Update audio level
    pub fn update_level(&mut self, level: f32, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;
        self.level_buffer.push(level);
    }

    /// Update spectrum bands
    pub fn update_spectrum(&mut self, bands: Vec<f32>, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;
        if bands.len() == 8 {
            let bands_array: [f32; 8] = bands.try_into().unwrap_or([0.0; 8]);
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

    /// Set completion action and start exit timer
    pub fn set_completion_action(&mut self, action: CompletionAction) {
        self.completion_action = Some(action);
        self.completion_started_at = Some(Instant::now());
        // Transition to Complete state for UI display
        self.state = crate::protocol::State::Complete;
    }

    /// Check if completion flash has expired and we should exit
    pub fn check_completion_exit(&self) -> bool {
        if let Some(started_at) = self.completion_started_at {
            // Exit after 750ms completion flash
            started_at.elapsed() >= std::time::Duration::from_millis(750)
        } else {
            false
        }
    }

    /// Check if we should auto-exit - simple state-driven approach
    pub fn check_auto_exit(&mut self) -> bool {
        // Exit when we have a transcription result, don't need window, and mouse isn't hovering
        if self.transcription_result.is_some() && !self.needs_window() && !self.is_mouse_hovering {
            self.should_auto_exit = true;
            return true;
        }
        false
    }

    /// Tick animations and return current state
    pub fn tick(&mut self, now: Instant) -> OsdState {
        // Get current alpha (for dot pulsing)
        let (_level, alpha) = if let Some(transcribing) = &self.transcribing_state {
            transcribing.tick(now)
        } else if let Some(recording) = &self.recording_state {
            // Recording: pulse alpha, use live level
            (self.level_buffer.last_10()[9], recording.tick(now))
        } else {
            (self.level_buffer.last_10()[9], 1.0)
        };

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

        // Calculate window animation values
        let (window_opacity, window_scale) = if let Some(anim) = &self.window_animation {
            let (t, complete) = anim.tick(now);

            let result = match anim.state {
                super::animation::WindowAnimationState::Appearing => {
                    // Ease out for smooth deceleration
                    let eased = super::animation::ease_out_cubic(t);
                    let opacity = eased;
                    let scale = 0.5 + (0.5 * eased);
                    eprintln!(
                        "OSD: Appearing animation - t={:.3}, opacity={:.3}, scale={:.3}",
                        t, opacity, scale
                    );
                    (opacity, scale) // opacity: 0→1, scale: 0.5→1.0
                }
                super::animation::WindowAnimationState::Disappearing => {
                    // Ease in for smooth acceleration
                    let eased = super::animation::ease_in_cubic(t);
                    let inv = 1.0 - eased;
                    let opacity = inv;
                    let scale = 0.5 + (0.5 * inv);
                    eprintln!(
                        "OSD: Disappearing animation - t={:.3}, opacity={:.3}, scale={:.3}",
                        t, opacity, scale
                    );
                    (opacity, scale) // opacity: 1→0, scale: 1.0→0.5
                }
            };

            if complete {
                eprintln!("OSD: Animation complete, clearing animation state");
                self.window_animation = None;
            }

            result
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
        if matches!(
            self.state,
            crate::protocol::State::Recording
                | crate::protocol::State::Transcribing
                | crate::protocol::State::Error
        ) {
            return true;
        }

        // Also show window during completion flash
        if self.completion_action.is_some() && !self.check_completion_exit() {
            return true;
        }

        false
    }

    /// Returns true if we just transitioned to needing a window
    pub fn should_create_window(&self, had_window: bool) -> bool {
        self.needs_window() && !had_window
    }

    /// Start appearing animation
    pub fn start_appearing_animation(&mut self) {
        self.window_animation = Some(super::animation::WindowAnimation::new_appearing());
        self.is_window_disappearing = false;
    }

    /// Returns true if we should start disappearing animation
    pub fn should_start_disappearing(&self, had_window: bool) -> bool {
        !self.needs_window()
            && had_window
            && !self.is_window_disappearing
            && !self.is_mouse_hovering // Don't start disappearing if mouse is hovering
    }

    /// Start disappearing animation
    pub fn start_disappearing_animation(&mut self) {
        self.window_animation = Some(super::animation::WindowAnimation::new_disappearing());
        self.is_window_disappearing = true;
    }

    /// Returns true if disappearing animation is complete and we should close window
    pub fn should_close_window(&self) -> bool {
        // Close window if we're marked as disappearing but animation is done (cleared)
        self.is_window_disappearing && self.window_animation.is_none()
    }
}
