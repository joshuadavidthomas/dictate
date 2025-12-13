//! OSD Application v2 - Using new state machine and timeline patterns
//!
//! This module demonstrates how to integrate:
//! - OsdVisualState for explicit state machine transitions
//! - Timeline for declarative animations
//! - Theme constants for centralized styling
//!
//! This is a prototype showing the patterns from the architecture exploration.

use iced::time::{self, Duration as IcedDuration};
use iced::widget::{container, text};
use iced::{window, Color, Element, Subscription, Task};
use iced_layershell::build_pattern::MainSettings;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings};
use iced_layershell::settings::{LayerShellSettings, StartMode};
use iced_layershell::to_layer_message;
use iced_runtime::window::Action as WindowAction;
use iced_runtime::{task, Action};
use std::time::Instant;

use super::state::{OsdVisualState, StateEvent};
use super::theme::{animation as anim_const, colors, dimensions, timing};
use super::timeline::{ids, Chain, PulseAnimation, Timeline, WidthAnimation, WindowAnimation};
use super::widgets::{osd_bar, OsdBarStyle};
use crate::recording::{RecordingSnapshot, SPECTRUM_BANDS};
use tokio::sync::broadcast;

/// Current OSD state for rendering (same as v1)
#[derive(Debug, Clone)]
pub struct OsdState {
    pub state: RecordingSnapshot,
    pub idle_hot: bool,
    pub pulse_alpha: f32,
    pub content_alpha: f32,
    pub spectrum_bands: [f32; SPECTRUM_BANDS],
    pub window_opacity: f32,
    pub window_scale: f32,
    pub timer_width: f32,
    pub recording_elapsed_secs: Option<u32>,
    pub current_ts: u64,
}

/// OSD Application v2 - Using state machine and timeline patterns
pub struct OsdAppV2 {
    // Visual state machine (replaces scattered flags)
    visual_state: OsdVisualState,

    // Animation timeline (replaces manual tweens)
    timeline: Timeline,

    // Recording data
    spectrum_buffer: super::buffer::SpectrumRingBuffer,
    recording_start_ts: Option<u64>,
    current_ts: u64,
    transcription_result: Option<String>,
    animation_epoch: Instant,

    // Timer width tracking (for animation)
    current_timer_width: f32,

    // App infrastructure
    broadcast_rx: broadcast::Receiver<crate::broadcast::Message>,
    render_state: OsdState,
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

impl OsdAppV2 {
    /// Create a new OsdAppV2 instance
    pub fn new(
        broadcast_rx: broadcast::Receiver<crate::broadcast::Message>,
        osd_position: crate::conf::OsdPosition,
    ) -> (Self, Task<Message>) {
        log::debug!("OSD v2: Created with state machine and timeline");

        let now = Instant::now();

        let mut app = OsdAppV2 {
            // State machine - starts hidden
            visual_state: OsdVisualState::Hidden,

            // Timeline - starts empty
            timeline: Timeline::new(),

            // Recording data
            spectrum_buffer: super::buffer::SpectrumRingBuffer::new(),
            recording_start_ts: None,
            current_ts: 0,
            transcription_result: None,
            animation_epoch: now,

            current_timer_width: dimensions::TIMER_WIDTH,

            // App infrastructure
            broadcast_rx,
            render_state: OsdState {
                state: RecordingSnapshot::Idle,
                idle_hot: false,
                pulse_alpha: 1.0,
                content_alpha: 0.0,
                spectrum_bands: [0.0; SPECTRUM_BANDS],
                window_opacity: 0.0,
                window_scale: dimensions::WINDOW_MIN_SCALE,
                timer_width: dimensions::TIMER_WIDTH,
                recording_elapsed_secs: None,
                current_ts: 0,
            },
            window_id: None,
            transcription_initiated: false,
            osd_position,
        };

        app.render_state = app.compute_render_state(now);
        (app, Task::done(Message::InitiateTranscription))
    }

    /// Settings for the daemon pattern
    pub fn settings(osd_position: crate::conf::OsdPosition) -> MainSettings {
        let (anchor, margin) = match osd_position {
            crate::conf::OsdPosition::Top => {
                (Anchor::Top | Anchor::Left | Anchor::Right, (10, 0, 0, 0))
            }
            crate::conf::OsdPosition::Bottom => {
                (Anchor::Bottom | Anchor::Left | Anchor::Right, (0, 0, 10, 0))
            }
        };

        MainSettings {
            layer_settings: LayerShellSettings {
                size: None,
                exclusive_zone: 0,
                anchor,
                layer: Layer::Overlay,
                margin,
                start_mode: StartMode::Background,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Namespace for the daemon pattern
    pub fn namespace(&self) -> String {
        String::from("Dictate OSD v2")
    }

    /// Update function using state machine transitions
    pub fn update(&mut self, message: Message) -> Task<Message> {
        let had_window_before = self.window_id.is_some();
        let now = Instant::now();

        match message {
            Message::Tick => {
                self.handle_tick(now);
            }
            Message::MouseEntered => {
                log::trace!("OSD v2: Mouse entered");
                self.visual_state.transition(StateEvent::MouseEnter);
            }
            Message::MouseExited => {
                log::trace!("OSD v2: Mouse exited");
                self.visual_state.transition(StateEvent::MouseExit);
            }
            Message::InitiateTranscription => {
                if !self.transcription_initiated {
                    log::debug!("OSD v2: Observer mode - listening to broadcast channel");
                    self.transcription_initiated = true;
                }
            }
            _ => {}
        }

        // Handle window management based on state machine
        self.handle_window_management(had_window_before, now)
    }

    /// Handle tick - process broadcast messages and update animations
    fn handle_tick(&mut self, now: Instant) {
        // Tick the timeline to remove completed animations
        self.timeline.tick();

        // Process broadcast messages
        let messages = crate::broadcast::BroadcastServer::drain_messages(&mut self.broadcast_rx);

        for msg in messages {
            match msg {
                crate::broadcast::Message::StatusEvent {
                    state,
                    spectrum,
                    idle_hot,
                    ts,
                    ..
                } => {
                    self.handle_state_change(state, idle_hot, ts);
                    if let Some(bands) = spectrum {
                        self.update_spectrum(bands, ts);
                    }
                }
                crate::broadcast::Message::Result { text, .. } => {
                    log::info!("OSD v2: Received transcription result - '{}'", text);
                    self.transcription_result = Some(text);
                }
                crate::broadcast::Message::Error { error, .. } => {
                    log::error!("OSD v2: Received error: {}", error);
                    self.handle_state_change(RecordingSnapshot::Error, false, self.current_ts);
                }
                crate::broadcast::Message::ConfigUpdate { osd_position } => {
                    log::debug!("OSD v2: Config update - position: {:?}", osd_position);
                    self.osd_position = osd_position;
                    self.transcription_result = None;

                    // Show preview with linger
                    let linger_until = Instant::now() + std::time::Duration::from_secs(2);
                    self.visual_state.transition(StateEvent::Show {
                        state: RecordingSnapshot::Idle,
                        idle_hot: true,
                    });
                    self.visual_state.transition(StateEvent::StartLinger {
                        until: linger_until,
                    });
                }
                _ => {}
            }
        }

        // Check linger expiration
        if let Some(until) = self.visual_state.linger_until() {
            if now >= until {
                self.visual_state.transition(StateEvent::LingerExpired);
                self.start_disappear_animation();
            }
        }

        // Check animation completion
        if self.visual_state.is_appearing() && self.timeline.is_complete(*ids::WINDOW) {
            self.visual_state.transition(StateEvent::AppearComplete);
        }
        if self.visual_state.is_disappearing() && self.timeline.is_complete(*ids::WINDOW) {
            self.visual_state.transition(StateEvent::DisappearComplete);
        }

        // Update render state
        self.render_state = self.compute_render_state(now);
    }

    /// Handle state change from recording state
    fn handle_state_change(&mut self, new_state: RecordingSnapshot, idle_hot: bool, ts: u64) {
        self.current_ts = ts;
        let (old_state, _) = self.visual_state.current_state_info();

        // Handle recording start
        if new_state == RecordingSnapshot::Recording && old_state != RecordingSnapshot::Recording {
            self.recording_start_ts = Some(ts);
            // Start pulse animation
            self.timeline.set(
                *ids::PULSE,
                PulseAnimation::pulse(
                    anim_const::PULSE_ALPHA_MIN,
                    anim_const::PULSE_ALPHA_MAX,
                    std::time::Duration::from_secs_f32(1.0 / timing::PULSE_HZ),
                ),
            );
            // Start timer width animation to full
            if self.current_timer_width != dimensions::TIMER_WIDTH {
                self.timeline.set(
                    *ids::TIMER_WIDTH,
                    WidthAnimation::transition(
                        self.current_timer_width,
                        dimensions::TIMER_WIDTH,
                        timing::TIMER_WIDTH,
                    ),
                );
            }
        } else if new_state != RecordingSnapshot::Recording {
            self.recording_start_ts = None;
        }

        // Handle transcribing start
        if new_state == RecordingSnapshot::Transcribing
            && old_state != RecordingSnapshot::Transcribing
        {
            // Keep pulse animation running
            if !self.timeline.is_running(*ids::PULSE) {
                self.timeline.set(
                    *ids::PULSE,
                    PulseAnimation::pulse(
                        anim_const::PULSE_ALPHA_MIN,
                        anim_const::PULSE_ALPHA_MAX,
                        std::time::Duration::from_secs_f32(1.0 / timing::PULSE_HZ),
                    ),
                );
            }
            // Animate timer width to 0
            if self.current_timer_width != 0.0 {
                self.timeline.set(
                    *ids::TIMER_WIDTH,
                    WidthAnimation::transition(self.current_timer_width, 0.0, timing::TIMER_WIDTH),
                );
            }
        }

        // Stop pulse when not recording/transcribing
        if !matches!(
            new_state,
            RecordingSnapshot::Recording | RecordingSnapshot::Transcribing
        ) && self.timeline.is_running(*ids::PULSE)
        {
            self.timeline.remove(*ids::PULSE);
        }

        // Update visual state machine
        if self.visual_state.is_visible() || self.visual_state.is_hovering() {
            self.visual_state.transition(StateEvent::StateChanged {
                state: new_state,
                idle_hot,
            });
        } else if matches!(
            new_state,
            RecordingSnapshot::Recording
                | RecordingSnapshot::Transcribing
                | RecordingSnapshot::Error
        ) && !self.visual_state.is_visible()
        {
            // Need to show window
            self.visual_state.transition(StateEvent::Show {
                state: new_state,
                idle_hot,
            });
            self.start_appear_animation();
        }
    }

    /// Update spectrum bands
    fn update_spectrum(&mut self, bands: Vec<f32>, ts: u64) {
        self.current_ts = ts;
        if bands.len() == SPECTRUM_BANDS {
            let bands_array: [f32; SPECTRUM_BANDS] =
                bands.try_into().unwrap_or([0.0; SPECTRUM_BANDS]);
            self.spectrum_buffer.push(bands_array);
        }
    }

    /// Start appear animation using timeline
    fn start_appear_animation(&mut self) {
        self.timeline
            .set(*ids::WINDOW, WindowAnimation::appear(timing::APPEAR));
        self.timeline
            .set(*ids::CONTENT, Chain::new(0.0).then(1.0, timing::APPEAR));
    }

    /// Start disappear animation using timeline
    fn start_disappear_animation(&mut self) {
        self.timeline
            .set(*ids::WINDOW, WindowAnimation::disappear(timing::DISAPPEAR));
        self.timeline
            .set(*ids::CONTENT, Chain::new(1.0).then(0.0, timing::DISAPPEAR));
    }

    /// Handle window creation/destruction based on state machine
    fn handle_window_management(&mut self, had_window: bool, now: Instant) -> Task<Message> {
        // Create window if needed
        if self.visual_state.is_appearing() && !had_window {
            let id = window::Id::unique();
            self.window_id = Some(id);
            log::debug!("OSD v2: Creating window");

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
                    size: Some(dimensions::WINDOW_SIZE),
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
        }

        // Close window if disappeared
        if matches!(self.visual_state, OsdVisualState::Hidden) && had_window {
            if let Some(id) = self.window_id.take() {
                log::debug!("OSD v2: Destroying window");
                return task::effect(Action::Window(WindowAction::Close(id)));
            }
        }

        // Start disappearing if needed
        if self.visual_state.is_visible()
            && !self.visual_state.needs_window()
            && !self.visual_state.is_hovering()
        {
            self.visual_state.transition(StateEvent::StartDisappear);
            self.start_disappear_animation();
        }

        Task::none()
    }

    /// Compute render state from timeline and state machine
    fn compute_render_state(&mut self, now: Instant) -> OsdState {
        // Get animation values from timeline
        let window_progress = self
            .timeline
            .get(*ids::WINDOW, self.default_window_progress());
        let content_alpha = self
            .timeline
            .get(*ids::CONTENT, self.default_content_alpha());
        let pulse_alpha = self.timeline.get(*ids::PULSE, 1.0);

        // Compute window visuals
        let window_opacity = window_progress;
        let window_scale =
            dimensions::WINDOW_MIN_SCALE + (1.0 - dimensions::WINDOW_MIN_SCALE) * window_progress;

        // Timer width
        let timer_width = self
            .timeline
            .get(*ids::TIMER_WIDTH, self.current_timer_width);
        if self.timeline.is_complete(*ids::TIMER_WIDTH) {
            self.current_timer_width = timer_width;
        }

        // Recording timer
        let recording_elapsed_secs = if let (RecordingSnapshot::Recording, Some(start_ts)) =
            (self.visual_state.recording_state(), self.recording_start_ts)
        {
            let elapsed_ms = self.current_ts.saturating_sub(start_ts);
            Some((elapsed_ms / 1000) as u32)
        } else {
            None
        };

        // Animation timestamp
        let animation_ts = now.duration_since(self.animation_epoch).as_millis() as u64;

        let (state, idle_hot) = self.visual_state.current_state_info();

        OsdState {
            state,
            idle_hot,
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

    /// Default window progress based on state
    fn default_window_progress(&self) -> f32 {
        if self.visual_state.is_visible()
            || self.visual_state.is_hovering()
            || self.visual_state.is_lingering()
        {
            1.0
        } else {
            0.0
        }
    }

    /// Default content alpha based on state
    fn default_content_alpha(&self) -> f32 {
        if matches!(self.visual_state, OsdVisualState::Hidden) {
            0.0
        } else {
            1.0
        }
    }

    /// View function for daemon pattern
    pub fn view(&self, id: window::Id) -> Element<'_, Message> {
        if self.window_id != Some(id) {
            return container(text("")).into();
        }

        let visual = &self.render_state;

        let bar_style = OsdBarStyle {
            height: dimensions::BAR_HEIGHT,
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
        time::every(IcedDuration::from_millis(16)).map(|_| Message::Tick)
    }

    /// Remove window ID when window is closed
    pub fn remove_id(&mut self, id: window::Id) {
        if self.window_id == Some(id) {
            log::debug!("OSD v2: Window removed: {:?}", id);
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
}
