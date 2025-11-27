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

// =============================================================================
// Animation Timing Constants
// =============================================================================

/// Duration for window appear animation (ms)
const WINDOW_APPEAR_DURATION_MS: u64 = 300;
/// Duration for window disappear animation (ms)
const WINDOW_DISAPPEAR_DURATION_MS: u64 = 250;
/// Duration for opacity fade at start of appear (ms)
const OPACITY_FADE_DURATION_MS: u64 = 50;
/// Duration for timer width transition (ms)
const TIMER_WIDTH_TRANSITION_MS: u64 = 150;

// =============================================================================
// Window Animation Constants
// =============================================================================

/// Minimum scale when window is collapsed
const WINDOW_MIN_SCALE: f32 = 0.5;
/// Content starts appearing at this progress (0.0-1.0)
const CONTENT_APPEAR_THRESHOLD: f32 = 0.7;
/// Content finishes fading out at this progress (0.0-1.0)
const CONTENT_FADE_THRESHOLD: f32 = 0.3;

// =============================================================================
// Status Dot Pulse Constants
// =============================================================================

/// Minimum alpha for pulsing status dot
const PULSE_ALPHA_MIN: f32 = 0.7;
/// Maximum alpha for pulsing status dot
const PULSE_ALPHA_MAX: f32 = 1.0;
/// Pulse frequency in Hz (cycles per second)
const PULSE_FREQUENCY_HZ: f32 = 0.5;

// =============================================================================
// Transcribing Waveform Animation Constants
// =============================================================================

/// Duration for one full sweep across the waveform (ms)
const TRANSCRIBING_SWEEP_DURATION_MS: u64 = 1500;
/// Peak height of the active bar during transcribing
const TRANSCRIBING_PEAK_HEIGHT: f32 = 0.85;
/// Background height of inactive bars (matches recording silence floor after scaling)
const TRANSCRIBING_BACKGROUND_HEIGHT: f32 = 0.005;
/// Distance from active position where falloff begins
const TRANSCRIBING_FALLOFF_DISTANCE: f32 = 1.5;
/// Rate at which peak falls off with distance
const TRANSCRIBING_FALLOFF_RATE: f32 = 0.5;

// =============================================================================
// Timer Constants
// =============================================================================

/// Width of timer display when visible (px)
pub const TIMER_WIDTH: f32 = 28.0;

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
            duration: Duration::from_millis(WINDOW_APPEAR_DURATION_MS),
            direction: WindowDirection::Appearing,
        }
    }

    pub fn new_disappearing() -> Self {
        Self {
            started_at: Instant::now(),
            duration: Duration::from_millis(WINDOW_DISAPPEAR_DURATION_MS),
            direction: WindowDirection::Disappearing,
        }
    }
}

/// Calculate pulsing alpha for status dot animation
///
/// Used during Recording and Transcribing states.
pub fn pulse_alpha(tween: &PulseTween, now: Instant) -> f32 {
    let elapsed_ms = (now - tween.started_at).as_millis() as f32;
    let pulse_t = (elapsed_ms / 1000.0) * PULSE_FREQUENCY_HZ;
    let alpha_range = PULSE_ALPHA_MAX - PULSE_ALPHA_MIN;
    PULSE_ALPHA_MIN + alpha_range * (pulse_t * 2.0 * std::f32::consts::PI).sin()
}

/// Compute window animation values: (opacity, scale, content_alpha)
pub fn compute_window_animation(tween: &WindowTween, now: Instant) -> (f32, f32, f32) {
    let elapsed = now.saturating_duration_since(tween.started_at);
    let t = (elapsed.as_secs_f32() / tween.duration.as_secs_f32()).clamp(0.0, 1.0);
    let fade_duration = OPACITY_FADE_DURATION_MS as f32;
    let scale_range = 1.0 - WINDOW_MIN_SCALE;
    
    match tween.direction {
        WindowDirection::Appearing => {
            let opacity = if elapsed.as_millis() < OPACITY_FADE_DURATION_MS as u128 {
                (elapsed.as_millis() as f32 / fade_duration).clamp(0.0, 1.0)
            } else { 1.0 };
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
            let remaining_ms = tween.duration.as_millis() as i64 - elapsed.as_millis() as i64;
            let opacity = if remaining_ms > OPACITY_FADE_DURATION_MS as i64 { 
                1.0 
            } else {
                (remaining_ms.max(0) as f32 / fade_duration).clamp(0.0, 1.0)
            };
            (opacity, scale, content_alpha)
        }
    }
}

use crate::recording::SPECTRUM_BANDS;

/// Calculate pulsing waveform values for transcribing state
/// Creates a bar-by-bar sweep effect that moves across the waveform
pub fn pulsing_waveform(timestamp_ms: u64) -> [f32; SPECTRUM_BANDS] {
    let mut arr = [0.0f32; SPECTRUM_BANDS];
    
    // Sweep one bar at a time across the waveform
    let position = (timestamp_ms % TRANSCRIBING_SWEEP_DURATION_MS) as f32 
        / TRANSCRIBING_SWEEP_DURATION_MS as f32;
    let active_position = position * SPECTRUM_BANDS as f32;
    
    for i in 0..SPECTRUM_BANDS {
        // Calculate distance from the active position (with smooth falloff)
        let dist = (i as f32 - active_position).abs();
        
        // Create a smooth peak that moves across
        let v = if dist < TRANSCRIBING_FALLOFF_DISTANCE {
            TRANSCRIBING_PEAK_HEIGHT - (dist * TRANSCRIBING_FALLOFF_RATE)
        } else {
            TRANSCRIBING_BACKGROUND_HEIGHT
        };
        arr[i] = v.clamp(TRANSCRIBING_BACKGROUND_HEIGHT, TRANSCRIBING_PEAK_HEIGHT);
    }
    arr
}

/// Timer width transition tween - animates timer container width between states
#[derive(Debug, Clone, Copy)]
pub struct WidthTween {
    pub started_at: Instant,
    pub duration: Duration,
    pub from_width: f32,
    pub to_width: f32,
}

impl WidthTween {
    pub fn new(from: f32, to: f32) -> Self {
        Self {
            started_at: Instant::now(),
            duration: Duration::from_millis(TIMER_WIDTH_TRANSITION_MS),
            from_width: from,
            to_width: to,
        }
    }

    /// Check if the tween has completed
    pub fn is_complete(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= self.duration
    }
}

/// Compute current width from tween
pub fn compute_width(tween: &WidthTween, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(tween.started_at);
    let t = (elapsed.as_secs_f32() / tween.duration.as_secs_f32()).clamp(0.0, 1.0);
    let eased = ease_out_cubic(t);
    tween.from_width + (tween.to_width - tween.from_width) * eased
}


