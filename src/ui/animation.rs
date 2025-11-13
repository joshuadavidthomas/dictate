//! Animation utilities for UI state transitions

use std::time::{Duration, Instant};

pub fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_cubic(t: f32) -> f32 {
    t.powi(3)
}

/// Width animation with ease-out
#[derive(Debug)]
pub struct WidthAnimation {
    started_at: Instant,
    duration: Duration,
    from: f32,
    to: f32,
}

impl WidthAnimation {
    pub fn new(from: f32, to: f32) -> Self {
        Self {
            started_at: Instant::now(),
            duration: Duration::from_millis(180),
            from,
            to,
        }
    }

    /// Get current animated value and whether animation is complete
    pub fn tick(&self, now: Instant) -> (f32, bool) {
        let elapsed = (now - self.started_at).as_secs_f32();
        let t = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        let ratio = self.from + (self.to - self.from) * ease_out_quad(t);
        (ratio, t >= 1.0)
    }
}

/// Window animation state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAnimationState {
    Appearing,
    Disappearing,
}

/// Window fade/scale animation
#[derive(Debug)]
pub struct WindowAnimation {
    pub state: WindowAnimationState,
    started_at: Instant,
    duration: Duration,
}

impl WindowAnimation {
    pub fn new_appearing() -> Self {
        Self {
            state: WindowAnimationState::Appearing,
            started_at: Instant::now(),
            duration: Duration::from_millis(200), // Fast, snappy
        }
    }

    pub fn new_disappearing() -> Self {
        Self {
            state: WindowAnimationState::Disappearing,
            started_at: Instant::now(),
            duration: Duration::from_millis(150), // Slightly faster out
        }
    }

    /// Returns (progress, is_complete) where progress is 0.0→1.0
    pub fn tick(&self, now: Instant) -> (f32, bool) {
        let elapsed = (now - self.started_at).as_secs_f32();
        let t = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        (t, t >= 1.0)
    }
}

/// Transcribing animation state
#[derive(Debug)]
pub struct TranscribingState {
    started_at: Instant,
    frozen_level: f32,
}

impl TranscribingState {
    pub fn new(frozen_level: f32) -> Self {
        Self {
            started_at: Instant::now(),
            frozen_level,
        }
    }

    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    /// Animate level (freeze 300ms, ease to 0 over 300ms) and alpha (pulse)
    pub fn tick(&self, now: Instant) -> (f32, f32) {
        let elapsed_ms = (now - self.started_at).as_millis() as f32;

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
    started_at: Instant,
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    /// Pulse: red dot alpha oscillates 0.4-1.0 @ 0.5Hz (slower, more dramatic)
    pub fn tick(&self, now: Instant) -> f32 {
        let elapsed_ms = (now - self.started_at).as_millis() as f32;
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

/// Ring buffer for spectrum data (last 30 frames of 8 bands each)
#[derive(Debug)]
pub struct SpectrumRingBuffer {
    buffer: [[f32; 8]; 30],
    index: usize,
}

impl SpectrumRingBuffer {
    pub fn new() -> Self {
        Self {
            buffer: [[0.0; 8]; 30],
            index: 0,
        }
    }

    pub fn push(&mut self, bands: [f32; 8]) {
        self.buffer[self.index] = bands;
        self.index = (self.index + 1) % 30;
    }

    /// Get the most recent frame
    pub fn last_frame(&self) -> [f32; 8] {
        let idx = (self.index + 29) % 30;
        self.buffer[idx]
    }
}
