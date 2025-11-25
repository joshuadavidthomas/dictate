//! OSD application state machine
//!
//! Manages the iced layer-shell window lifecycle, animations, and event handling.

use crate::audio::SPECTRUM_BANDS;
use crate::osd::animation::{PulseTween, WindowTween};
use crate::osd::widgets::{osd_bar, OsdVisual};
use crate::osd::{colors, SpectrumBuffer};
use crate::settings::OsdPosition;
use crate::{Broadcaster, Event, RecordingStatus};

use iced::time::{self, Duration};
use iced::widget::{container, text};
use iced::{window, Color, Element, Subscription, Task};
use iced_layershell::build_pattern::MainSettings;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings};
use iced_layershell::settings::{LayerShellSettings, StartMode};
use iced_layershell::to_layer_message;
use iced_layershell::Appearance;
use iced_runtime::window::Action as WindowAction;
use iced_runtime::{task, Action};
use std::time::Instant;
use tokio::sync::broadcast;

pub fn settings(position: OsdPosition) -> MainSettings {
    let (anchor, margin) = match position {
        OsdPosition::Top => (Anchor::Top | Anchor::Left | Anchor::Right, (10, 0, 0, 0)),
        OsdPosition::Bottom => (Anchor::Bottom | Anchor::Left | Anchor::Right, (0, 0, 10, 0)),
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

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    MouseEntered,
    MouseExited,
}

pub struct OsdApp {
    // State
    state: RecordingStatus,
    idle_hot: bool,
    spectrum_buffer: SpectrumBuffer,
    recording_start_ts: Option<u64>,
    current_ts: u64,

    // Animation
    pulse_tween: Option<PulseTween>,
    window_tween: Option<WindowTween>,
    is_disappearing: bool,
    is_hovering: bool,
    linger_until: Option<Instant>,

    // Infrastructure
    event_rx: broadcast::Receiver<Event>,
    window_id: Option<window::Id>,
    position: OsdPosition,
    last_event: Instant,
}

impl OsdApp {
    pub fn new(
        event_rx: broadcast::Receiver<Event>,
        position: OsdPosition,
    ) -> (Self, Task<Message>) {
        let now = Instant::now();

        let app = Self {
            state: RecordingStatus::Idle,
            idle_hot: false,
            spectrum_buffer: SpectrumBuffer::new(),
            recording_start_ts: None,
            current_ts: 0,
            pulse_tween: None,
            window_tween: None,
            is_disappearing: false,
            is_hovering: false,
            linger_until: None,
            event_rx,
            window_id: None,
            position,
            last_event: now,
        };

        (app, Task::none())
    }

    pub fn namespace(&self) -> String {
        "Dictate OSD".into()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let had_window = self.window_id.is_some();
        let now = Instant::now();

        match message {
            Message::Tick => {
                // Reset stale hover state
                if self.is_hovering && self.last_event.elapsed() > std::time::Duration::from_secs(30) {
                    self.is_hovering = false;
                }

                // Process events from broadcast channel
                for event in Broadcaster::drain(&mut self.event_rx) {
                    self.handle_event(event);
                }
            }
            Message::MouseEntered => {
                self.is_hovering = true;
                self.last_event = now;
            }
            Message::MouseExited => {
                self.is_hovering = false;
                self.last_event = now;
            }
            _ => {}
        }

        // Window lifecycle management
        if self.should_create_window(had_window) {
            self.window_tween = Some(WindowTween::appearing());
            self.is_disappearing = false;

            let id = window::Id::unique();
            self.window_id = Some(id);

            let (anchor, margin) = match self.position {
                OsdPosition::Top => (Anchor::Top | Anchor::Left | Anchor::Right, Some((10, 0, 0, 0))),
                OsdPosition::Bottom => (Anchor::Bottom | Anchor::Left | Anchor::Right, Some((0, 0, 10, 0))),
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
        } else if self.should_start_disappearing(had_window) {
            self.window_tween = Some(WindowTween::disappearing());
            self.is_disappearing = true;
        } else if self.should_close_window(now) && had_window {
            if let Some(id) = self.window_id.take() {
                self.is_disappearing = false;
                self.linger_until = None;
                self.window_tween = None;
                return task::effect(Action::Window(WindowAction::Close(id)));
            }
        }

        Task::none()
    }

    pub fn view(&self, id: window::Id) -> Element<'_, Message> {
        if self.window_id != Some(id) {
            return container(text("")).into();
        }

        let visual = self.compute_visual(Instant::now());

        osd_bar(&visual, Message::MouseEntered, Message::MouseExited)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        time::every(Duration::from_millis(16)).map(|_| Message::Tick)
    }

    pub fn remove_id(&mut self, id: window::Id) {
        if self.window_id == Some(id) {
            self.window_id = None;
        }
    }

    pub fn style(&self, _theme: &iced::Theme) -> Appearance {
        Appearance {
            background_color: Color::TRANSPARENT,
            text_color: colors::LIGHT_GRAY,
        }
    }

    // ========================================================================
    // Private helpers
    // ========================================================================

    fn handle_event(&mut self, event: Event) {
        self.last_event = Instant::now();

        match event {
            Event::Status { state, spectrum, ts } => {
                self.update_state(state, ts);
                if let Some(bands) = spectrum {
                    if bands.len() == SPECTRUM_BANDS {
                        let arr: [f32; SPECTRUM_BANDS] = bands.try_into().unwrap();
                        self.spectrum_buffer.push(arr);
                    }
                }
            }
            Event::Result { .. } => {
                // Result received - will transition to idle shortly
            }
            Event::Error { .. } => {
                self.state = RecordingStatus::Error;
            }
            Event::ConfigUpdate { osd_position } => {
                self.position = osd_position;
                self.linger_until = Some(Instant::now() + std::time::Duration::from_secs(2));
                self.idle_hot = true;
                if self.window_id.is_some() {
                    self.is_disappearing = true;
                }
            }
            Event::ModelProgress { .. } => {
                // OSD doesn't display model progress
            }
        }
    }

    fn update_state(&mut self, new_state: RecordingStatus, ts: u64) {
        self.current_ts = ts;

        // Handle recording start
        if new_state == RecordingStatus::Recording && self.state != RecordingStatus::Recording {
            self.pulse_tween = Some(PulseTween::new());
            self.recording_start_ts = Some(ts);
            self.linger_until = None;
        } else if new_state != RecordingStatus::Recording && self.recording_start_ts.is_some() {
            self.recording_start_ts = None;
        }

        // Handle transcribing start
        if new_state == RecordingStatus::Transcribing && self.state != RecordingStatus::Transcribing {
            self.pulse_tween = Some(PulseTween::new());
            self.linger_until = None;
        } else if new_state != RecordingStatus::Transcribing && new_state != RecordingStatus::Recording {
            self.pulse_tween = None;
        }

        self.state = new_state;
    }

    fn compute_visual(&self, now: Instant) -> OsdVisual {
        let pulse_alpha = self.pulse_tween
            .as_ref()
            .map(|t| t.alpha(now))
            .unwrap_or(1.0);

        let elapsed_secs = if self.state == RecordingStatus::Recording {
            self.recording_start_ts.map(|start| {
                ((self.current_ts.saturating_sub(start)) / 1000) as u32
            })
        } else {
            None
        };

        let (window_opacity, window_scale, content_alpha) = if let Some(ref tween) = self.window_tween {
            let (opacity, scale, _) = tween.values(now);
            let content = if tween.appearing {
                let t = (now - tween.started_at).as_secs_f32() / tween.duration.as_secs_f32();
                if t < 0.7 { 0.0 } else { ((t - 0.7) / 0.3).clamp(0.0, 1.0) }
            } else {
                let t = (now - tween.started_at).as_secs_f32() / tween.duration.as_secs_f32();
                if t < 0.3 { 1.0 - (t / 0.3) } else { 0.0 }
            };
            (opacity, scale, content)
        } else if self.is_disappearing {
            (0.0, 0.5, 0.0)
        } else if self.needs_window() {
            (1.0, 1.0, 1.0)
        } else {
            (0.0, 0.5, 0.0)
        };

        OsdVisual {
            state: self.state,
            idle_hot: self.idle_hot,
            pulse_alpha,
            content_alpha,
            window_opacity,
            window_scale,
            spectrum: self.spectrum_buffer.last(),
            elapsed_secs,
            timestamp_ms: self.current_ts,
        }
    }

    fn needs_window(&self) -> bool {
        let state_needs = matches!(
            self.state,
            RecordingStatus::Recording | RecordingStatus::Transcribing | RecordingStatus::Error
        );
        let lingering = self.linger_until.map(|t| Instant::now() < t).unwrap_or(false);
        state_needs || lingering
    }

    fn should_create_window(&self, had_window: bool) -> bool {
        self.needs_window() && !had_window
    }

    fn should_start_disappearing(&self, had_window: bool) -> bool {
        !self.needs_window() && had_window && !self.is_disappearing && !self.is_hovering
    }

    fn should_close_window(&self, now: Instant) -> bool {
        if !self.is_disappearing {
            return false;
        }
        self.window_tween
            .as_ref()
            .map(|t| t.values(now).2)
            .unwrap_or(true)
    }
}
