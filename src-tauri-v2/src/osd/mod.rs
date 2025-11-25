//! On-Screen Display overlay using iced layer-shell
//!
//! This module provides the visual overlay that shows recording status,
//! audio spectrum, and transcription progress.

mod app;
mod widgets;

use crate::settings::OsdPosition;
use crate::Event;
use anyhow::Result;
use iced_layershell::build_pattern::daemon;
use tokio::sync::broadcast;

/// Run the OSD overlay
pub fn run(event_rx: broadcast::Receiver<Event>, position: OsdPosition) -> Result<()> {
    eprintln!("[osd] Starting layer-shell overlay");

    daemon(
        app::OsdApp::namespace,
        app::OsdApp::update,
        app::OsdApp::view,
        app::OsdApp::remove_id,
    )
    .style(app::OsdApp::style)
    .subscription(app::OsdApp::subscription)
    .settings(app::settings(position))
    .run_with(move || app::OsdApp::new(event_rx, position))?;

    Ok(())
}

// ============================================================================
// Colors
// ============================================================================

pub mod colors {
    use iced::Color;

    const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    pub const GRAY: Color = rgb(122, 122, 122);
    pub const DIM_GREEN: Color = rgb(118, 211, 155);
    pub const RED: Color = rgb(231, 76, 60);
    pub const BLUE: Color = rgb(52, 152, 219);
    pub const ORANGE: Color = rgb(243, 156, 18);
    pub const LIGHT_GRAY: Color = rgb(200, 200, 200);
    pub const DARK_GRAY: Color = rgb(30, 30, 30);
    pub const BLACK: Color = rgb(0, 0, 0);

    pub const fn with_alpha(color: Color, alpha: f32) -> Color {
        Color { a: alpha, ..color }
    }
}

// ============================================================================
// Animation
// ============================================================================

pub mod animation {
    use std::time::{Duration, Instant};

    pub fn ease_out_cubic(t: f32) -> f32 {
        1.0 - (1.0 - t).powi(3)
    }

    pub fn ease_in_cubic(t: f32) -> f32 {
        t.powi(3)
    }

    /// Pulse animation for status dot
    #[derive(Debug, Clone, Copy)]
    pub struct PulseTween {
        pub started_at: Instant,
    }

    impl PulseTween {
        pub fn new() -> Self {
            Self { started_at: Instant::now() }
        }

        /// Calculate pulsing alpha (0.7-1.0 at 0.5Hz)
        pub fn alpha(&self, now: Instant) -> f32 {
            let elapsed_ms = (now - self.started_at).as_millis() as f32;
            let t = (elapsed_ms / 1000.0) * 0.5;
            0.7 + 0.3 * (t * 2.0 * std::f32::consts::PI).sin()
        }
    }

    /// Window fade/scale animation
    #[derive(Debug, Clone, Copy)]
    pub struct WindowTween {
        pub started_at: Instant,
        pub duration: Duration,
        pub appearing: bool,
    }

    impl WindowTween {
        pub fn appearing() -> Self {
            Self {
                started_at: Instant::now(),
                duration: Duration::from_millis(240),
                appearing: true,
            }
        }

        pub fn disappearing() -> Self {
            Self {
                started_at: Instant::now(),
                duration: Duration::from_millis(200),
                appearing: false,
            }
        }

        /// Returns (opacity, scale, is_complete)
        pub fn values(&self, now: Instant) -> (f32, f32, bool) {
            let elapsed = (now - self.started_at).as_secs_f32();
            let t = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
            let complete = t >= 1.0;

            if self.appearing {
                let eased = ease_out_cubic(t);
                (eased, 0.5 + 0.5 * eased, complete)
            } else {
                let eased = ease_in_cubic(t);
                let inv = 1.0 - eased;
                (inv, 0.5 + 0.5 * inv, complete)
            }
        }
    }
}

// ============================================================================
// Spectrum Buffer
// ============================================================================

use crate::audio::SPECTRUM_BANDS;

/// Ring buffer for spectrum data smoothing
pub struct SpectrumBuffer {
    buffer: [[f32; SPECTRUM_BANDS]; 30],
    index: usize,
}

impl SpectrumBuffer {
    pub fn new() -> Self {
        Self {
            buffer: [[0.0; SPECTRUM_BANDS]; 30],
            index: 0,
        }
    }

    pub fn push(&mut self, bands: [f32; SPECTRUM_BANDS]) {
        self.buffer[self.index] = bands;
        self.index = (self.index + 1) % 30;
    }

    pub fn last(&self) -> [f32; SPECTRUM_BANDS] {
        let idx = (self.index + 29) % 30;
        self.buffer[idx]
    }
}

impl Default for SpectrumBuffer {
    fn default() -> Self {
        Self::new()
    }
}
