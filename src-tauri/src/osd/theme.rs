//! Theme constants for the OSD
//!
//! Centralizes colors, timing, and dimensions inspired by COSMIC's theming patterns.
//! This module provides a single source of truth for visual styling.

use iced::Color;
use std::time::Duration;

// =============================================================================
// Colors
// =============================================================================

/// State indicator colors for the status dot and waveform
pub mod colors {
    use super::*;

    /// Helper to create colors from RGB8 values
    const fn rgb8(r: u8, g: u8, b: u8) -> Color {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    /// Idle state - gray, inactive
    pub const IDLE: Color = rgb8(122, 122, 122);

    /// Idle "hot" state - green, ready to record
    pub const IDLE_HOT: Color = rgb8(118, 211, 155);

    /// Recording state - red, actively capturing
    pub const RECORDING: Color = rgb8(231, 76, 60);

    /// Transcribing state - blue, processing audio
    pub const TRANSCRIBING: Color = rgb8(52, 152, 219);

    /// Error state - orange, something went wrong
    pub const ERROR: Color = rgb8(243, 156, 18);

    /// Light gray for text and secondary elements
    pub const LIGHT_GRAY: Color = rgb8(200, 200, 200);

    /// Dark gray for backgrounds
    pub const DARK_GRAY: Color = rgb8(30, 30, 30);

    /// Pure black
    pub const BLACK: Color = rgb8(0, 0, 0);

    /// Helper function for creating colors with alpha
    pub const fn with_alpha(color: Color, alpha: f32) -> Color {
        Color {
            r: color.r,
            g: color.g,
            b: color.b,
            a: alpha,
        }
    }
}

// =============================================================================
// Animation Timing
// =============================================================================

/// Animation timing constants
pub mod timing {
    use super::*;

    /// Duration for window appear animation
    pub const APPEAR: Duration = Duration::from_millis(300);

    /// Duration for window disappear animation
    pub const DISAPPEAR: Duration = Duration::from_millis(250);

    /// Duration for opacity fade at animation boundaries
    pub const OPACITY_FADE: Duration = Duration::from_millis(50);

    /// Duration for timer width transitions
    pub const TIMER_WIDTH: Duration = Duration::from_millis(150);

    /// Duration for one full sweep across the waveform during transcribing
    pub const TRANSCRIBING_SWEEP: Duration = Duration::from_millis(1500);

    /// How long to linger after state change before disappearing
    pub const LINGER: Duration = Duration::from_millis(1500);

    /// Pulse frequency in Hz (cycles per second) for status dot
    pub const PULSE_HZ: f32 = 0.5;
}

// =============================================================================
// Dimensions
// =============================================================================

/// Dimension constants for the OSD layout
pub mod dimensions {
    /// Height of the OSD bar
    pub const BAR_HEIGHT: f32 = 32.0;

    /// Corner radius for the OSD bar
    pub const BAR_RADIUS: f32 = 12.0;

    /// Radius of the status dot indicator
    pub const DOT_RADIUS: f32 = 6.0;

    /// Timer display width when visible
    pub const TIMER_WIDTH: f32 = 28.0;

    /// Window size (width, height)
    pub const WINDOW_SIZE: (u32, u32) = (140, 48);

    /// Minimum scale when window is collapsed (for animation)
    pub const WINDOW_MIN_SCALE: f32 = 0.5;
}

// =============================================================================
// Spacing
// =============================================================================

/// Spacing constants for layout consistency
pub mod spacing {
    /// Extra extra small spacing (2px)
    pub const XXSMALL: f32 = 2.0;

    /// Extra small spacing (4px)
    pub const XSMALL: f32 = 4.0;

    /// Small spacing (8px)
    pub const SMALL: f32 = 8.0;

    /// Medium spacing (12px)
    pub const MEDIUM: f32 = 12.0;

    /// Large spacing (16px)
    pub const LARGE: f32 = 16.0;
}

// =============================================================================
// Animation Constants
// =============================================================================

/// Animation-specific constants
pub mod animation {
    /// Content starts appearing at this progress (0.0-1.0) during window appear
    pub const CONTENT_APPEAR_THRESHOLD: f32 = 0.7;

    /// Content finishes fading out at this progress (0.0-1.0) during window disappear
    pub const CONTENT_FADE_THRESHOLD: f32 = 0.3;

    /// Minimum alpha for pulsing status dot
    pub const PULSE_ALPHA_MIN: f32 = 0.7;

    /// Maximum alpha for pulsing status dot
    pub const PULSE_ALPHA_MAX: f32 = 1.0;

    /// Peak height of the active bar during transcribing waveform
    pub const TRANSCRIBING_PEAK_HEIGHT: f32 = 0.85;

    /// Background height of inactive bars (matches recording silence floor after scaling)
    pub const TRANSCRIBING_BACKGROUND_HEIGHT: f32 = 0.005;

    /// Distance from active position where falloff begins
    pub const TRANSCRIBING_FALLOFF_DISTANCE: f32 = 1.5;

    /// Rate at which peak falls off with distance
    pub const TRANSCRIBING_FALLOFF_RATE: f32 = 0.5;
}
