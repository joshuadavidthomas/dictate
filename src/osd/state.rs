//! OSD state machine and visual properties

use std::time::{Duration, Instant};

/// OSD state machine states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Recording,
    Transcribing,
    Error,
}

/// Visual properties for a state
#[derive(Debug, Clone, Copy)]
pub struct Visual {
    pub color: iced::Color,
    pub ratio: f32, // Width ratio [0.0, 1.0]
}

impl State {
    /// Get the visual properties for this state
    pub fn visual(&self, idle_hot: bool) -> Visual {
        match (self, idle_hot) {
            (State::Idle, false) => Visual {
                color: gray(),
                ratio: 1.00,  // Consistent width
            },
            (State::Idle, true) => Visual {
                color: dim_green(),
                ratio: 1.00,  // Consistent width
            },
            (State::Recording, _) => Visual {
                color: red(),
                ratio: 1.00,
            },
            (State::Transcribing, _) => Visual {
                color: blue(),
                ratio: 1.00,  // Consistent width
            },
            (State::Error, _) => Visual {
                color: orange(),
                ratio: 1.00,  // Consistent width
            },
        }
    }
}

// Color helper functions
fn gray() -> iced::Color {
    iced::Color::from_rgb8(122, 122, 122)
}

fn dim_green() -> iced::Color {
    iced::Color::from_rgb8(118, 211, 155)
}

fn red() -> iced::Color {
    iced::Color::from_rgb8(231, 76, 60)
}

fn blue() -> iced::Color {
    iced::Color::from_rgb8(52, 152, 219)
}

fn orange() -> iced::Color {
    iced::Color::from_rgb8(243, 156, 18)
}

/// Width animation with ease-out
#[derive(Debug)]
pub struct WidthAnimation {
    start: Instant,
    duration: Duration,
    from: f32,
    to: f32,
}

impl WidthAnimation {
    pub fn new(from: f32, to: f32) -> Self {
        Self {
            start: Instant::now(),
            duration: Duration::from_millis(180),
            from,
            to,
        }
    }

    /// Get current animated value and whether animation is complete
    pub fn tick(&self, now: Instant) -> (f32, bool) {
        let elapsed = (now - self.start).as_secs_f32();
        let t = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        let ratio = self.from + (self.to - self.from) * ease_out_quad(t);
        (ratio, t >= 1.0)
    }
}

fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}

/// Should we animate between these two ratios?
pub fn should_animate(from: f32, to: f32) -> bool {
    (to - from).abs() >= 0.10
}

/// Transcribing animation state
#[derive(Debug)]
pub struct TranscribingState {
    entered_at: Instant,
    frozen_level: f32,
}

impl TranscribingState {
    pub fn new(frozen_level: f32) -> Self {
        Self {
            entered_at: Instant::now(),
            frozen_level,
        }
    }

    /// Animate level (freeze 300ms, ease to 0 over 300ms) and alpha (pulse)
    pub fn animate(&self, now: Instant) -> (f32, f32) {
        let elapsed_ms = (now - self.entered_at).as_millis() as f32;

        // 1. Level: freeze 300ms, ease to 0 over 300ms
        let level = if elapsed_ms < 300.0 {
            self.frozen_level
        } else if elapsed_ms < 600.0 {
            let t = (elapsed_ms - 300.0) / 300.0;
            self.frozen_level * (1.0 - ease_out_quad(t))
        } else {
            0.0
        };

        // 2. Pulse: blue dot alpha oscillates 0.4-1.0 @ 0.5Hz (slower, more dramatic)
        let pulse_t = (elapsed_ms / 1000.0) * 0.5; // 0.5 Hz (2 second cycle)
        let alpha = 0.7 + 0.3 * (pulse_t * 2.0 * std::f32::consts::PI).sin();

        (level, alpha)
    }
}

/// Recording animation state (for pulsing dot)
#[derive(Debug)]
pub struct RecordingState {
    entered_at: Instant,
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            entered_at: Instant::now(),
        }
    }

    /// Pulse: red dot alpha oscillates 0.4-1.0 @ 0.5Hz (slower, more dramatic)
    pub fn animate(&self, now: Instant) -> f32 {
        let elapsed_ms = (now - self.entered_at).as_millis() as f32;
        let pulse_t = (elapsed_ms / 1000.0) * 0.5; // 0.5 Hz (2 second cycle)
        // Use sin without abs() for smooth fade in/out, map from [-1, 1] to [0.4, 1.0]
        0.7 + 0.3 * (pulse_t * 2.0 * std::f32::consts::PI).sin()
    }
}

/// Ring buffer for level bars (last 10 samples from 30-sample buffer)
#[derive(Debug)]
pub struct LevelRingBuffer {
    buffer: [f32; 30],
    index: usize,
}

impl LevelRingBuffer {
    pub fn new() -> Self {
        Self {
            buffer: [0.0; 30],
            index: 0,
        }
    }

    pub fn push(&mut self, level: f32) {
        self.buffer[self.index] = level;
        self.index = (self.index + 1) % 30;
    }

    /// Get the last 10 samples for display
    pub fn last_10(&self) -> [f32; 10] {
        let mut result = [0.0; 10];
        for i in 0..10 {
            let idx = (self.index + 20 + i) % 30;
            result[i] = self.buffer[idx];
        }
        result
    }
}

/// Complete OSD state with animations
#[derive(Debug)]
pub struct OsdState {
    pub state: State,
    pub idle_hot: bool,
    pub current_ratio: f32,
    pub width_animation: Option<WidthAnimation>,
    pub recording_state: Option<RecordingState>,
    pub transcribing_state: Option<TranscribingState>,
    pub level_buffer: LevelRingBuffer,
    pub last_message: Instant,
}

impl OsdState {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            idle_hot: false,
            current_ratio: 1.00, // Consistent full width
            width_animation: None,
            recording_state: None,
            transcribing_state: None,
            level_buffer: LevelRingBuffer::new(),
            last_message: Instant::now(),
        }
    }

    /// Update state from server event
    pub fn update_state(&mut self, new_state: State, idle_hot: bool) {
        self.last_message = Instant::now();

        let visual = new_state.visual(idle_hot);
        let old_ratio = self.current_ratio;
        if (visual.ratio - old_ratio).abs() > 0.01 {
            self.width_animation = Some(WidthAnimation::new(old_ratio, visual.ratio));
            self.current_ratio = visual.ratio;
            self.width_animation = None;
        }

        // Handle recording state transition
        if new_state == State::Recording && self.state != State::Recording {
            // Entering recording - start pulsing animation
            self.recording_state = Some(RecordingState::new());
        } else if new_state != State::Recording {
            self.recording_state = None;
        }

        // Handle transcribing state transition
        if new_state == State::Transcribing && self.state != State::Transcribing {
            // Entering transcribing - freeze current level
            let frozen_level = self.level_buffer.last_10()[9]; // Last sample
            self.transcribing_state = Some(TranscribingState::new(frozen_level));
        } else if new_state != State::Transcribing {
            // If transitioning away from Transcribing, check minimum display time
            if self.state == State::Transcribing {
                if let Some(trans_state) = &self.transcribing_state {
                    let elapsed = Instant::now().duration_since(trans_state.entered_at);
                    if elapsed < std::time::Duration::from_millis(500) {
                        // Don't transition yet - keep Transcribing state for minimum visibility
                        return;
                    }
                }
            }
            self.transcribing_state = None;
        }

        self.state = new_state;
        self.idle_hot = idle_hot;
    }

    /// Update audio level
    pub fn update_level(&mut self, level: f32) {
        self.last_message = Instant::now();
        self.level_buffer.push(level);
    }

    /// Set error state
    pub fn set_error(&mut self) {
        self.update_state(State::Error, false);
    }

    /// Tick animations and return current visual state
    pub fn tick(&mut self, now: Instant) -> OsdVisual {
        // Tick width animation
        if let Some(anim) = &self.width_animation {
            let (ratio, complete) = anim.tick(now);
            self.current_ratio = ratio;
            if complete {
                self.width_animation = None;
            }
        }

        // Get current level and alpha
        let (level, alpha) = if let Some(transcribing) = &self.transcribing_state {
            transcribing.animate(now)
        } else if let Some(recording) = &self.recording_state {
            // Recording: pulse alpha, use live level
            (self.level_buffer.last_10()[9], recording.animate(now))
        } else {
            (self.level_buffer.last_10()[9], 1.0)
        };

        let visual = self.state.visual(self.idle_hot);

        OsdVisual {
            state: self.state,
            color: visual.color,
            alpha,
            content_ratio: self.current_ratio,
            level,
            level_bars: self.level_buffer.last_10(),
        }
    }

    /// Check if any animation is active
    pub fn is_animating(&self) -> bool {
        self.width_animation.is_some() || self.transcribing_state.is_some()
    }

    /// Check for timeout (no messages for 15 seconds)
    pub fn has_timeout(&self) -> bool {
        self.last_message.elapsed() > Duration::from_secs(15)
    }
}

/// Current visual state for rendering
#[derive(Debug, Clone)]
pub struct OsdVisual {
    pub state: State,
    pub color: iced::Color,
    pub alpha: f32,
    pub content_ratio: f32,
    pub level: f32,
    pub level_bars: [f32; 10],
}
