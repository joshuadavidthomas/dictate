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

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_cubic(t: f32) -> f32 {
    t.powi(3)
}

/// Pulse animation tween - used for status dot animation during Recording and Transcribing
#[derive(Debug, Clone, Copy)]
pub struct PulseTween {
    pub started_at: Instant,
}

impl PulseTween {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
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

/// Calculate pulsing alpha for status dot animation
///
/// Used during Recording and Transcribing states.
/// Returns alpha value oscillating between 0.7-1.0 at 0.5Hz (2 second cycle).
pub fn pulse_alpha(tween: &PulseTween, now: Instant) -> f32 {
    let elapsed_ms = (now - tween.started_at).as_millis() as f32;
    let pulse_t = (elapsed_ms / 1000.0) * 0.5; // 0.5 Hz (2 second cycle)
    0.7 + 0.3 * (pulse_t * 2.0 * std::f32::consts::PI).sin()
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

