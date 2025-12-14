//! Temporary compatibility layer for old animation types
//!
//! This module provides the old animation API using the new timeline system.
//! It will be removed when app.rs is rewritten to use timeline directly.

use std::time::Instant;

pub use crate::osd::theme::dimensions::TIMER_WIDTH;

// Re-export these for compatibility - they were constants in the old animation.rs
// but are now moved to theme
use crate::osd::theme::animation::{PULSE_ALPHA_MAX, PULSE_ALPHA_MIN};
use crate::osd::theme::timing::PULSE_HZ;

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
    pub duration: std::time::Duration,
    pub direction: WindowDirection,
}

impl WindowTween {
    pub fn new_appearing() -> Self {
        Self {
            started_at: Instant::now(),
            duration: crate::osd::theme::timing::APPEAR,
            direction: WindowDirection::Appearing,
        }
    }

    pub fn new_disappearing() -> Self {
        Self {
            started_at: Instant::now(),
            duration: crate::osd::theme::timing::DISAPPEAR,
            direction: WindowDirection::Disappearing,
        }
    }
}

/// Timer width transition tween - animates timer container width between states
#[derive(Debug, Clone, Copy)]
pub struct WidthTween {
    pub started_at: Instant,
    pub duration: std::time::Duration,
    pub from_width: f32,
    pub to_width: f32,
}

impl WidthTween {
    pub fn new(from: f32, to: f32) -> Self {
        Self {
            started_at: Instant::now(),
            duration: crate::osd::theme::timing::TIMER_WIDTH,
            from_width: from,
            to_width: to,
        }
    }

    /// Check if the tween has completed
    pub fn is_complete(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= self.duration
    }
}

/// Calculate pulsing alpha for status dot animation
///
/// Used during Recording and Transcribing states.
pub fn pulse_alpha(tween: &PulseTween, now: Instant) -> f32 {
    let elapsed_ms = (now - tween.started_at).as_millis() as f32;
    let pulse_t = (elapsed_ms / 1000.0) * PULSE_HZ;
    let alpha_range = PULSE_ALPHA_MAX - PULSE_ALPHA_MIN;
    PULSE_ALPHA_MIN + alpha_range * (pulse_t * 2.0 * std::f32::consts::PI).sin()
}

/// Compute window animation values: (opacity, scale, content_alpha)
pub fn compute_window_animation(tween: &WindowTween, now: Instant) -> (f32, f32, f32) {
    use crate::osd::theme::animation::{CONTENT_APPEAR_THRESHOLD, CONTENT_FADE_THRESHOLD};
    use crate::osd::theme::dimensions::WINDOW_MIN_SCALE;
    use crate::osd::timeline::{ease_in_cubic, ease_out_cubic};

    let elapsed = now.saturating_duration_since(tween.started_at);
    let t = (elapsed.as_secs_f64() / tween.duration.as_secs_f64()).clamp(0.0, 1.0) as f32;
    let fade_duration = crate::osd::theme::timing::OPACITY_FADE.as_millis() as f32;
    let scale_range = 1.0 - WINDOW_MIN_SCALE;

    match tween.direction {
        WindowDirection::Appearing => {
            let opacity = if elapsed < crate::osd::theme::timing::OPACITY_FADE {
                (elapsed.as_millis() as f32 / fade_duration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let scale = WINDOW_MIN_SCALE + (ease_out_cubic(t) * scale_range);
            let content_progress = 1.0 - CONTENT_APPEAR_THRESHOLD;
            let content_alpha = if t < CONTENT_APPEAR_THRESHOLD {
                0.0
            } else {
                ((t - CONTENT_APPEAR_THRESHOLD) / content_progress).clamp(0.0, 1.0)
            };
            (opacity, scale, content_alpha)
        }
        WindowDirection::Disappearing => {
            let content_alpha = if t < CONTENT_FADE_THRESHOLD {
                1.0 - (t / CONTENT_FADE_THRESHOLD)
            } else {
                0.0
            };
            let scale = 1.0 - (ease_in_cubic(t) * scale_range);
            let remaining = tween.duration.saturating_sub(elapsed);
            let opacity = if remaining > crate::osd::theme::timing::OPACITY_FADE {
                1.0
            } else {
                (remaining.as_millis() as f32 / fade_duration).clamp(0.0, 1.0)
            };
            (opacity, scale, content_alpha)
        }
    }
}

/// Compute current width from tween
pub fn compute_width(tween: &WidthTween, now: Instant) -> f32 {
    use crate::osd::timeline::ease_out_cubic;

    let elapsed = now.saturating_duration_since(tween.started_at);
    let t = (elapsed.as_secs_f64() / tween.duration.as_secs_f64()).clamp(0.0, 1.0) as f32;
    let eased = ease_out_cubic(t);
    tween.from_width + (tween.to_width - tween.from_width) * eased
}
