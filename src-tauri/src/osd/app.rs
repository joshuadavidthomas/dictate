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
use crate::state::RecordingSnapshot;
use tokio::sync::broadcast;

/// Current OSD state for rendering
#[derive(Debug, Clone)]
pub struct OsdState {
    pub state: RecordingSnapshot,
    pub idle_hot: bool,
    pub alpha: f32,
    pub spectrum_bands: [f32; SPECTRUM_BANDS],
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
            max_duration: 30,
            silence_duration: 2,
            sample_rate: 16000,
        }
    }
}

pub struct OsdApp {
    // Protocol state & data (from Osd)
    state: RecordingSnapshot,
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
    broadcast_rx: broadcast::Receiver<String>,
    render_state: OsdState,
    window_id: Option<window::Id>,
    config: TranscriptionConfig,
    transcription_initiated: bool,
    transcription_mode: TranscriptionMode,
    osd_position: crate::conf::OsdPosition,
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    MouseEntered,
    MouseExited,
    InitiateTranscription,
}

impl OsdApp {
    /// Create a new OsdApp instance
    pub fn new(
        broadcast_rx: broadcast::Receiver<String>, 
        config: TranscriptionConfig, 
        mode: TranscriptionMode,
        osd_position: crate::conf::OsdPosition,
    ) -> (Self, Task<Message>) {
        eprintln!("OSD: Created with broadcast channel receiver");

        let now = Instant::now();

        let mut app = OsdApp {
            // Protocol state & data
            state: RecordingSnapshot::Idle,
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
            broadcast_rx,
            render_state: OsdState {
                state: RecordingSnapshot::Idle,
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
            transcription_initiated: false,
            transcription_mode: mode,
            osd_position,
        };

        // Initialize render state
        app.render_state = app.tick(now);

        (app, Task::done(Message::InitiateTranscription))
    }

    /// Settings for the daemon pattern
    pub fn settings(osd_position: crate::conf::OsdPosition) -> MainSettings {
        let (anchor, margin) = match osd_position {
            crate::conf::OsdPosition::Top => (
                Anchor::Top | Anchor::Left | Anchor::Right,
                (10, 0, 0, 0),
            ),
            crate::conf::OsdPosition::Bottom => (
                Anchor::Bottom | Anchor::Left | Anchor::Right,
                (0, 0, 10, 0),
            ),
        };
        
        MainSettings {
            layer_settings: LayerShellSettings {
                size: None, // No initial window
                exclusive_zone: 0,
                anchor,
                layer: Layer::Overlay,
                margin,
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

                // Try to read broadcast channel messages (non-blocking)
                loop {
                    match self.broadcast_rx.try_recv() {
                        Ok(line) => {
                            match serde_json::from_str::<crate::broadcast::Message>(&line) {
                                Ok(crate::broadcast::Message::StatusEvent {
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
                                Ok(crate::broadcast::Message::Result {
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

                                    // In Observer mode, the main app handles output (clipboard/insert)
                                    // OSD just displays the result
                                    eprintln!("OSD: Transcription complete, waiting for idle state");
                                }
                                Ok(crate::broadcast::Message::Error { error, .. }) => {
                                    eprintln!("OSD: Received error from server: {}", error);
                                    self.set_error();
                                }
                                Ok(crate::broadcast::Message::ConfigUpdate { osd_position }) => {
                                    eprintln!("OSD: Received config update - new position: {:?}", osd_position);
                                    self.osd_position = osd_position;
                                    
                                    // Show preview at new position
                                    // Set a linger time to briefly show the OSD at the new position
                                    self.linger_until = Some(Instant::now() + std::time::Duration::from_secs(2));
                                    
                                    // Set idle_hot to show green "ready" state for preview
                                    self.idle_hot = true;
                                    
                                    // If window exists, close it so it recreates at new position
                                    if self.window_id.is_some() {
                                        eprintln!("OSD: Closing existing window to recreate at new position");
                                        self.is_window_disappearing = true;
                                    }
                                }
                                Ok(_) => {
                                    // Ignore other message types (Status, Subscribed)
                                }
                                Err(e) => {
                                    eprintln!("OSD: Failed to decode message: {}", e);
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::TryRecvError::Empty) => {
                            // No more messages available right now
                            break;
                        }
                        Err(broadcast::error::TryRecvError::Closed) => {
                            eprintln!("OSD: Broadcast channel closed");
                            self.set_error();
                            break;
                        }
                        Err(broadcast::error::TryRecvError::Lagged(n)) => {
                            eprintln!("OSD: Missed {} messages (receiver too slow)", n);
                            // Continue processing - we'll get the latest messages
                        }
                    }
                }

                // Update cached visual state for rendering
                self.render_state = self.tick(Instant::now());
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
                    // Observer mode: we're using a broadcast channel, no need to send requests
                    eprintln!("OSD: Observer mode - listening to broadcast channel");
                    self.transcription_initiated = true;
                }
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

            let (anchor, margin) = match self.osd_position {
                crate::conf::OsdPosition::Top => (
                    Anchor::Top | Anchor::Left | Anchor::Right,
                    Some((10, 0, 0, 0)),
                ),
                crate::conf::OsdPosition::Bottom => (
                    Anchor::Bottom | Anchor::Left | Anchor::Right,
                    Some((0, 0, 10, 0)),
                ),
            };
            
            return Task::done(Message::NewLayerShell {
                settings: NewLayerShellSettings {
                    size: Some((440, 56)),
                    exclusive_zone: None,
                    anchor,
                    layer: Layer::Overlay,
                    margin,
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
    pub fn update_state(&mut self, new_state: RecordingSnapshot, idle_hot: bool, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;

        // Handle recording state transition
        if new_state == RecordingSnapshot::Recording
            && self.state != RecordingSnapshot::Recording
        {
            // Entering recording - start pulsing animation and clear lingering
            self.state_pulse = Some(super::animation::PulseTween::new());
            self.recording_start_ts = Some(ts);
            self.linger_until = None;
        } else if new_state != RecordingSnapshot::Recording && self.state_pulse.is_some() {
            self.state_pulse = None;
            self.recording_start_ts = None;
        }

        // Handle transcribing state transition
        if new_state == RecordingSnapshot::Transcribing
            && self.state != RecordingSnapshot::Transcribing
        {
            // Entering transcribing - start pulse animation
            self.state_pulse = Some(super::animation::PulseTween::new());
            // Clear any lingering when starting a new transcription
            self.linger_until = None;
        } else if new_state != RecordingSnapshot::Transcribing {
            self.window_tween = None;
            if self.state == RecordingSnapshot::Transcribing
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
        self.update_state(RecordingSnapshot::Error, false, self.current_ts);
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
        let recording_elapsed_secs = if self.state == RecordingSnapshot::Recording {
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
        // Show window for Recording, Transcribing, Error, or if we're in linger period
        let state_needs_window = matches!(
            self.state,
            RecordingSnapshot::Recording
                | RecordingSnapshot::Transcribing
                | RecordingSnapshot::Error
        );
        
        let is_lingering = self.linger_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);
        
        state_needs_window || is_lingering
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
