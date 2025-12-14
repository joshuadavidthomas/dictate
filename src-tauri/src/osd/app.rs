use iced::time::{self, Duration as IcedDuration};
use iced::widget::{container, text};
use iced::{Color, Element, Subscription, Task, window};
use iced_layershell::to_layer_message;
use iced_runtime::window::Action as WindowAction;
use iced_runtime::{Action, task};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;

use super::backend::OsdBackend;
use super::state::{self, OsdAction, OsdEvent, VisualState};
use super::theme::{animation as theme_animation, dimensions, timing};
use super::timeline::{self, PulseAnimation, Timeline, WindowAnimation, ids};
use super::widgets::{OsdBarStyle, osd_bar};
use crate::recording::{RecordingSnapshot, SPECTRUM_BANDS};

/// Current OSD state for rendering
#[derive(Debug, Clone)]
pub struct RenderState {
    pub state: RecordingSnapshot,
    pub idle_hot: bool,
    pub pulse_alpha: f32,
    pub content_alpha: f32,
    pub spectrum_bands: [f32; SPECTRUM_BANDS],
    pub window_opacity: f32,                 // 0.0 - 1.0 for fade animation
    pub window_scale: f32,                   // 0.5 - 1.0 for expand/shrink animation
    pub timer_width: f32,                    // Timer container width (animated, 0 when transcribing)
    pub recording_elapsed_secs: Option<u32>, // Elapsed seconds while recording
    pub current_ts: u64,                     // Current timestamp in milliseconds
}

pub struct OsdApp {
    // Backend
    backend: Arc<dyn OsdBackend>,
    
    // State machine
    osd_state: state::OsdState,
    
    // Animation timeline
    timeline: Timeline,
    window_animation_gen: Option<u64>,
    
    // Data
    spectrum_buffer: super::buffer::SpectrumRingBuffer,
    last_message: Instant,
    linger_until: Option<Instant>,
    recording_start_ts: Option<u64>,
    current_ts: u64,
    transcription_result: Option<String>,
    animation_epoch: Instant,  // For computing monotonic animation timestamps

    // App infrastructure
    broadcast_rx: broadcast::Receiver<crate::broadcast::Message>,
    render_state: RenderState,
    window_id: Option<window::Id>,
    transcription_initiated: bool,
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
        backend: Arc<dyn OsdBackend>,
        broadcast_rx: broadcast::Receiver<crate::broadcast::Message>,
        osd_position: crate::conf::OsdPosition,
    ) -> (Self, Task<Message>) {
        log::debug!("OSD: Created with {} backend", backend.name());

        let now = Instant::now();

        let mut app = OsdApp {
            // Backend
            backend,
            
            // State machine
            osd_state: state::OsdState::new(),
            
            // Animation timeline
            timeline: Timeline::new(),
            window_animation_gen: None,
            
            // Data
            spectrum_buffer: super::buffer::SpectrumRingBuffer::new(),
            last_message: now,
            linger_until: None,
            recording_start_ts: None,
            current_ts: 0,
            transcription_result: None,
            animation_epoch: now,

            // App infrastructure
            broadcast_rx,
            render_state: RenderState {
                state: RecordingSnapshot::Idle,
                idle_hot: false,
                pulse_alpha: 1.0,
                content_alpha: 0.0,
                spectrum_bands: [0.0; SPECTRUM_BANDS],
                window_opacity: 0.0,
                window_scale: 0.5,
                timer_width: dimensions::TIMER_WIDTH,
                recording_elapsed_secs: None,
                current_ts: 0,
            },

            window_id: None,
            transcription_initiated: false,
            osd_position,
        };

        // Initialize render state
        app.render_state = app.compute_render_state(now);

        (app, Task::done(Message::InitiateTranscription))
    }



    /// Namespace for the daemon pattern
    pub fn namespace(&self) -> String {
        String::from("Dictate OSD")
    }

    /// Execute actions returned from state machine transitions
    fn execute_actions(&mut self, actions: Vec<OsdAction>) -> Task<Message> {
        let mut task = Task::none();

        for action in actions {
            match action {
                OsdAction::CreateWindow => {
                    let id = window::Id::unique();
                    self.window_id = Some(id);

                    let settings = self.backend.create_window_settings(self.osd_position);
                    log::debug!("OSD: Creating window via {} backend for state {:?}", 
                                self.backend.name(), self.osd_state.visual);

                    task = Task::done(Message::NewLayerShell { settings, id });
                }
                OsdAction::DestroyWindow => {
                    if let Some(id) = self.window_id.take() {
                        log::debug!("OSD: Destroying window");
                        task = task::effect(Action::Window(WindowAction::Close(id)));
                    }
                }
                OsdAction::StartAppearAnimation => {
                    self.timeline.set(
                        *ids::WINDOW,
                        WindowAnimation::appear(timing::APPEAR),
                    );
                    self.window_animation_gen = self.timeline.generation(*ids::WINDOW);
                    log::debug!("OSD: Started appear animation (gen={:?})", self.window_animation_gen);
                }
                OsdAction::StartDisappearAnimation => {
                    self.timeline.set(
                        *ids::WINDOW,
                        WindowAnimation::disappear(timing::DISAPPEAR),
                    );
                    self.window_animation_gen = self.timeline.generation(*ids::WINDOW);
                    log::debug!("OSD: Started disappear animation (gen={:?})", self.window_animation_gen);
                }
                OsdAction::StartPulseAnimation => {
                    self.timeline.set(
                        *ids::PULSE,
                        PulseAnimation::pulse(
                            theme_animation::PULSE_ALPHA_MIN,
                            theme_animation::PULSE_ALPHA_MAX,
                            std::time::Duration::from_secs_f32(1.0 / timing::PULSE_HZ),
                        ),
                    );
                    log::debug!("OSD: Started pulse animation");
                }
                OsdAction::StopPulseAnimation => {
                    self.timeline.remove(*ids::PULSE);
                    log::debug!("OSD: Stopped pulse animation");
                }
                OsdAction::StartLingerTimer { duration } => {
                    self.linger_until = Some(Instant::now() + duration);
                    log::debug!("OSD: Started linger timer ({:?})", duration);
                }
                OsdAction::CancelLingerTimer => {
                    self.linger_until = None;
                    log::debug!("OSD: Cancelled linger timer");
                }
            }
        }

        task
    }

    /// Update function for daemon pattern
    pub fn update(&mut self, message: Message) -> Task<Message> {
        let now = Instant::now();

        match message {
            Message::Tick => {
                // Process broadcast messages
                let messages =
                    crate::broadcast::BroadcastServer::drain_messages(&mut self.broadcast_rx);

                for msg in messages {
                    match msg {
                        crate::broadcast::Message::StatusEvent {
                            state,
                            spectrum,
                            idle_hot,
                            ts,
                            ..
                        } => {
                            // Update domain state
                            self.last_message = Instant::now();
                            self.current_ts = ts;
                            self.osd_state.update_domain(state, idle_hot);

                            // Track recording start timestamp
                            if state == RecordingSnapshot::Recording
                                && self.recording_start_ts.is_none()
                            {
                                self.recording_start_ts = Some(ts);
                            } else if state != RecordingSnapshot::Recording {
                                self.recording_start_ts = None;
                            }

                            // Update spectrum
                            if let Some(bands) = spectrum {
                                if bands.len() == SPECTRUM_BANDS {
                                    let bands_array: [f32; SPECTRUM_BANDS] =
                                        bands.try_into().unwrap_or([0.0; SPECTRUM_BANDS]);
                                    self.spectrum_buffer.push(bands_array);
                                }
                            }

                            // Dispatch phase changed event
                            let actions = self.osd_state.transition(
                                OsdEvent::PhaseChanged {
                                    phase: state,
                                    idle_hot,
                                },
                                self.window_animation_gen,
                            );

                            // Execute resulting actions
                            if !actions.is_empty() {
                                return self.execute_actions(actions);
                            }
                        }
                        crate::broadcast::Message::Result {
                            text,
                            duration,
                            model,
                            ..
                        } => {
                            log::info!(
                                "OSD: Received transcription result - text='{}', duration={}, model={}",
                                text, duration, model
                            );
                            self.transcription_result = Some(text);
                        }
                        crate::broadcast::Message::Error { error, .. } => {
                            log::error!("OSD: Received error from server: {}", error);
                            // Error state handled by StatusEvent
                        }
                        crate::broadcast::Message::ConfigUpdate { osd_position } => {
                            log::debug!(
                                "OSD: Received config update - new position: {:?}",
                                osd_position
                            );
                            self.osd_position = osd_position;
                            self.transcription_result = None;

                            // Set idle_hot for green preview
                            self.osd_state.update_domain(RecordingSnapshot::Idle, true);

                            // Dispatch preview event
                            let actions = self.osd_state.transition(
                                OsdEvent::PreviewRequested,
                                self.window_animation_gen,
                            );

                            if !actions.is_empty() {
                                return self.execute_actions(actions);
                            }
                        }
                        crate::broadcast::Message::ModelDownloadProgress { .. } => {
                            // OSD does not display model download progress; ignore.
                        }
                    }
                }

                // Check for timer expiry
                if let Some(until) = self.linger_until {
                    if now >= until {
                        self.linger_until = None;
                        let actions = self.osd_state.transition(
                            OsdEvent::LingerExpired,
                            self.window_animation_gen,
                        );

                        if !actions.is_empty() {
                            return self.execute_actions(actions);
                        }
                    }
                }

                // Check for animation completion
                if self.timeline.is_running(*ids::WINDOW) {
                    if matches!(self.osd_state.visual, VisualState::Appearing)
                        && self.timeline.is_complete(*ids::WINDOW)
                    {
                        let current_gen = self.timeline.generation(*ids::WINDOW).unwrap_or(0);
                        let actions = self.osd_state.transition(
                            OsdEvent::AppearComplete { generation: current_gen },
                            self.window_animation_gen,
                        );

                        if !actions.is_empty() {
                            return self.execute_actions(actions);
                        }
                    } else if matches!(self.osd_state.visual, VisualState::Disappearing)
                        && self.timeline.is_complete(*ids::WINDOW)
                    {
                        let current_gen = self.timeline.generation(*ids::WINDOW).unwrap_or(0);
                        let actions = self.osd_state.transition(
                            OsdEvent::DisappearComplete { generation: current_gen },
                            self.window_animation_gen,
                        );

                        if !actions.is_empty() {
                            return self.execute_actions(actions);
                        }
                    }
                }

                // Update render state
                self.render_state = self.compute_render_state(now);
            }
            Message::MouseEntered => {
                log::trace!("OSD: Mouse entered window");
                let actions = self.osd_state.transition(
                    OsdEvent::MouseEnter,
                    self.window_animation_gen,
                );

                if !actions.is_empty() {
                    return self.execute_actions(actions);
                }
            }
            Message::MouseExited => {
                log::trace!("OSD: Mouse exited window");
                let actions = self.osd_state.transition(
                    OsdEvent::MouseExit,
                    self.window_animation_gen,
                );

                if !actions.is_empty() {
                    return self.execute_actions(actions);
                }
            }
            Message::InitiateTranscription => {
                if !self.transcription_initiated {
                    log::debug!("OSD: Observer mode - listening to broadcast channel");
                    self.transcription_initiated = true;
                }
            }
            _ => {
                // All other messages (NewLayerShell, etc.) are handled by the framework
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
            height: 32.0,
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
            log::debug!("OSD: Window removed: {:?}", id);
            self.window_id = None;
        }
    }

    /// Style function for daemon pattern
    pub fn style(&self, _theme: &iced::Theme) -> iced_layershell::Appearance {
        iced_layershell::Appearance {
            background_color: Color::TRANSPARENT,
            text_color: super::theme::colors::LIGHT_GRAY,
        }
    }

    /// Compute current render state from domain state and timeline
    fn compute_render_state(&self, now: Instant) -> RenderState {
        // Get timeline animation values
        let window_progress = self.timeline.get_at(*ids::WINDOW, now, 0.0);
        let pulse_alpha = self.timeline.get_at(*ids::PULSE, now, 1.0);

        // Compute window animation values based on visual state
        let (window_opacity, window_scale, content_alpha) = match self.osd_state.visual {
            VisualState::Hidden => (0.0, dimensions::WINDOW_MIN_SCALE, 0.0),
            VisualState::Appearing => {
                let opacity = if window_progress < 0.16 {
                    window_progress / 0.16
                } else {
                    1.0
                };
                let scale = dimensions::WINDOW_MIN_SCALE
                    + (1.0 - dimensions::WINDOW_MIN_SCALE) * timeline::ease_out_cubic(window_progress);
                let content_alpha = if window_progress < theme_animation::CONTENT_APPEAR_THRESHOLD {
                    0.0
                } else {
                    (window_progress - theme_animation::CONTENT_APPEAR_THRESHOLD)
                        / (1.0 - theme_animation::CONTENT_APPEAR_THRESHOLD)
                };
                (opacity, scale, content_alpha)
            }
            VisualState::Visible | VisualState::Hovering | VisualState::Lingering => {
                (1.0, 1.0, 1.0)
            }
            VisualState::Disappearing => {
                let t = window_progress;
                let content_alpha = if t < theme_animation::CONTENT_FADE_THRESHOLD {
                    1.0 - (t / theme_animation::CONTENT_FADE_THRESHOLD)
                } else {
                    0.0
                };
                let scale = 1.0 - (timeline::ease_in_cubic(t) * (1.0 - dimensions::WINDOW_MIN_SCALE));
                let opacity = if t > 0.84 {
                    1.0 - ((t - 0.84) / 0.16)
                } else {
                    1.0
                };
                (opacity, scale, content_alpha)
            }
        };

        // Calculate recording timer
        let recording_elapsed_secs = if self.osd_state.domain.phase == RecordingSnapshot::Recording {
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

        // Timer width animation (full when recording, 0 when transcribing)
        let timer_width = if self.osd_state.domain.phase == RecordingSnapshot::Recording {
            dimensions::TIMER_WIDTH
        } else {
            0.0
        };

        // Use monotonic time for animations
        let animation_ts = now.duration_since(self.animation_epoch).as_millis() as u64;

        RenderState {
            state: self.osd_state.domain.phase,
            idle_hot: self.osd_state.domain.idle_hot,
            pulse_alpha,
            content_alpha,
            spectrum_bands: self.spectrum_buffer.last_frame(),
            window_opacity,
            window_scale,
            timer_width,
            recording_elapsed_secs,
            current_ts: animation_ts,
        }
    }
}
