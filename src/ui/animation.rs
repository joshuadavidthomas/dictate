//! Animation system using tweens (pure functions operating on data structures)
//!
//! A "tween" (from "in-betweening") is a time-based value generator that computes
//! intermediate values between keyframes. This module provides animation capabilities
//! using tweens as pure functions of time.
//!
//! This module provides:
//! - Data structures that bundle tween parameters (just data, no behavior)
//! - Pure functions that compute animation values given data + time
//!
//! Pattern: Structs hold state, functions do computation

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

/// Parameters for recording pulse tween
#[derive(Debug, Clone, Copy)]
pub struct RecordingTween {
    pub started_at: Instant,
}

impl RecordingTween {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }
}

/// Parameters for transcribing fade + pulse tween
#[derive(Debug, Clone, Copy)]
pub struct TranscribingTween {
    pub started_at: Instant,
    pub frozen_level: f32,
}

impl TranscribingTween {
    pub fn new(frozen_level: f32) -> Self {
        Self {
            started_at: Instant::now(),
            frozen_level,
        }
    }

    pub fn started_at(&self) -> Instant {
        self.started_at
    }
}

/// State-based tweens (mutually exclusive)
#[derive(Debug, Clone, Copy)]
pub enum StateTween {
    Recording(RecordingTween),
    Transcribing(TranscribingTween),
}

impl StateTween {
    /// Sample the current alpha value for this state tween
    pub fn sample_alpha(&self, now: Instant) -> f32 {
        match self {
            StateTween::Recording(tween) => pulse_alpha(tween, now),
            StateTween::Transcribing(tween) => {
                let (_level, alpha) = transcribing_effect(tween, now);
                alpha
            }
        }
    }
}

/// Direction of window transition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowDirection {
    Appearing,
    Disappearing,
}

/// Parameters for window fade/scale tween
#[derive(Debug, Clone, Copy)]
pub struct WindowTween {
    pub started_at: Instant,
    pub duration: Duration,
    pub direction: WindowDirection,
}

impl WindowTween {
    pub fn new_appearing() -> Self {
        Self {
            started_at: Instant::now(),
            duration: Duration::from_millis(200),
            direction: WindowDirection::Appearing,
        }
    }

    pub fn new_disappearing() -> Self {
        Self {
            started_at: Instant::now(),
            duration: Duration::from_millis(150),
            direction: WindowDirection::Disappearing,
        }
    }
}

/// Calculate pulsing alpha for recording state
///
/// Returns alpha value oscillating between 0.7-1.0 at 0.5Hz
pub fn pulse_alpha(tween: &RecordingTween, now: Instant) -> f32 {
    let elapsed_ms = (now - tween.started_at).as_millis() as f32;
    let pulse_t = (elapsed_ms / 1000.0) * 0.5; // 0.5 Hz (2 second cycle)
    0.7 + 0.3 * (pulse_t * 2.0 * std::f32::consts::PI).sin()
}

/// Calculate level fade and pulsing alpha for transcribing state
///
/// Returns (level, alpha) where:
/// - level: freeze 300ms, then ease to 0 over 300ms
/// - alpha: oscillates between 0.7-1.0 at 0.5Hz
pub fn transcribing_effect(tween: &TranscribingTween, now: Instant) -> (f32, f32) {
    let elapsed_ms = (now - tween.started_at).as_millis() as f32;

    // Level: freeze 300ms, ease to 0 over 300ms
    let level = if elapsed_ms < 300.0 {
        tween.frozen_level
    } else if elapsed_ms < 600.0 {
        let t = (elapsed_ms - 300.0) / 300.0;
        tween.frozen_level * (1.0 - ease_out_quad(t))
    } else {
        0.0
    };

    // Alpha: pulse
    let pulse_t = (elapsed_ms / 1000.0) * 0.5; // 0.5 Hz
    let alpha = 0.7 + 0.3 * (pulse_t * 2.0 * std::f32::consts::PI).sin();

    (level, alpha)
}

/// Calculate window fade and scale transition
///
/// Returns (opacity, scale, is_complete) where:
/// - opacity: 0.0→1.0 (appearing) or 1.0→0.0 (disappearing)
/// - scale: 0.5→1.0 (appearing) or 1.0→0.5 (disappearing)
/// - is_complete: true when animation has finished
pub fn window_transition(tween: &WindowTween, now: Instant) -> (f32, f32, bool) {
    let elapsed = (now - tween.started_at).as_secs_f32();
    let t = (elapsed / tween.duration.as_secs_f32()).clamp(0.0, 1.0);
    let complete = t >= 1.0;

    let (opacity, scale) = match tween.direction {
        WindowDirection::Appearing => {
            // Ease out for smooth deceleration
            let eased = ease_out_cubic(t);
            let opacity = eased;
            let scale = 0.5 + (0.5 * eased);
            (opacity, scale)
        }
        WindowDirection::Disappearing => {
            // Ease in for smooth acceleration
            let eased = ease_in_cubic(t);
            let inv = 1.0 - eased;
            let opacity = inv;
            let scale = 0.5 + (0.5 * inv);
            (opacity, scale)
        }
    };

    (opacity, scale, complete)
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
