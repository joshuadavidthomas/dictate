//! Animated Spectrum Widget with internal state
//!
//! This demonstrates the pattern from iced examples where animation state
//! is stored in `widget::tree::State` and updated via window events.
//!
//! Benefits:
//! - Widget owns its animation state (no external timeline needed for simple cases)
//! - Automatically requests redraws when animating
//! - Clean separation of concerns

use crate::recording::SPECTRUM_BANDS;
use iced::advanced;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::event::{self, Event};
use iced::mouse;
use iced::window;
use iced::{Border, Color, Element, Length, Rectangle, Shadow, Size, Theme};
use std::time::Instant;

/// Configuration for the animated spectrum
pub struct AnimatedSpectrumConfig {
    pub bar_width: f32,
    pub bar_spacing: f32,
    pub total_height: f32,
    pub max_bar_height: f32,
    pub amplification: f32,
    pub normalization_curve: f32,
    pub silence_floor: f32,
    pub corner_radius: f32,
}

impl Default for AnimatedSpectrumConfig {
    fn default() -> Self {
        Self {
            bar_width: 3.0,
            bar_spacing: 2.0,
            total_height: 20.0,
            max_bar_height: 9.0,
            amplification: 2.0,
            normalization_curve: 0.6,
            silence_floor: 0.05,
            corner_radius: 1.0,
        }
    }
}

/// Internal animation state stored in widget tree
#[derive(Debug, Clone)]
struct SpectrumState {
    /// Current displayed values (smoothed)
    current_values: [f32; SPECTRUM_BANDS],
    /// Target values to animate towards
    target_values: [f32; SPECTRUM_BANDS],
    /// Last update time for delta calculation
    last_update: Instant,
    /// Whether animation is in progress
    is_animating: bool,
}

impl Default for SpectrumState {
    fn default() -> Self {
        Self {
            current_values: [0.0; SPECTRUM_BANDS],
            target_values: [0.0; SPECTRUM_BANDS],
            last_update: Instant::now(),
            is_animating: false,
        }
    }
}

impl SpectrumState {
    /// Smoothing factor for value interpolation (0.0-1.0, higher = faster)
    const SMOOTHING: f32 = 0.3;

    /// Update animation state, returns true if still animating
    fn update(&mut self, now: Instant) -> bool {
        let dt = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;

        // Exponential smoothing towards target
        let factor = (1.0 - (-dt * 60.0 * Self::SMOOTHING).exp()).min(1.0);

        let mut still_animating = false;
        for i in 0..SPECTRUM_BANDS {
            let diff = self.target_values[i] - self.current_values[i];
            if diff.abs() > 0.001 {
                self.current_values[i] += diff * factor;
                still_animating = true;
            } else {
                self.current_values[i] = self.target_values[i];
            }
        }

        self.is_animating = still_animating;
        still_animating
    }

    /// Set new target values
    fn set_target(&mut self, values: [f32; SPECTRUM_BANDS]) {
        self.target_values = values;
        self.is_animating = true;
    }
}

/// Create an animated spectrum waveform widget
pub fn animated_spectrum(bands: [f32; SPECTRUM_BANDS], color: Color) -> AnimatedSpectrum {
    AnimatedSpectrum {
        bands,
        color,
        config: AnimatedSpectrumConfig::default(),
    }
}

/// An animated spectrum analyzer widget with internal smoothing
pub struct AnimatedSpectrum {
    bands: [f32; SPECTRUM_BANDS],
    color: Color,
    config: AnimatedSpectrumConfig,
}

impl AnimatedSpectrum {
    /// Set custom configuration
    pub fn config(mut self, config: AnimatedSpectrumConfig) -> Self {
        self.config = config;
        self
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for AnimatedSpectrum
where
    Renderer: advanced::Renderer,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<SpectrumState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(SpectrumState::default())
    }

    fn size(&self) -> Size<Length> {
        let total_width = (SPECTRUM_BANDS as f32)
            * (self.config.bar_width + self.config.bar_spacing)
            - self.config.bar_spacing;
        Size {
            width: Length::Fixed(total_width),
            height: Length::Fixed(self.config.total_height),
        }
    }

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let total_width = (SPECTRUM_BANDS as f32)
            * (self.config.bar_width + self.config.bar_spacing)
            - self.config.bar_spacing;
        let size = limits
            .width(Length::Fixed(total_width))
            .height(Length::Fixed(self.config.total_height))
            .resolve(
                Length::Fixed(total_width),
                Length::Fixed(self.config.total_height),
                Size::ZERO,
            );

        layout::Node::new(size)
    }

    fn on_event(
        &mut self,
        tree: &mut widget::Tree,
        event: Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn advanced::Clipboard,
        shell: &mut advanced::Shell<'_, Message>,
        _viewport: &Rectangle,
    ) -> event::Status {
        let state = tree.state.downcast_mut::<SpectrumState>();

        match event {
            Event::Window(window::Event::RedrawRequested(now)) => {
                // Update target values from input
                state.set_target(self.bands);

                // Animate towards target
                if state.update(now) {
                    // Request another redraw if still animating
                    shell.request_redraw(window::RedrawRequest::NextFrame);
                }

                event::Status::Captured
            }
            _ => event::Status::Ignored,
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<SpectrumState>();
        let bounds = layout.bounds();
        let center_y = bounds.y + (self.config.total_height / 2.0);

        for (i, &level) in state.current_values.iter().enumerate() {
            // Apply amplification and normalization
            let amplified = (level * self.config.amplification)
                .clamp(0.0, 1.0)
                .powf(self.config.normalization_curve);
            let normalized = amplified.max(self.config.silence_floor);

            let bar_height = normalized * self.config.max_bar_height;
            let x = bounds.x + (i as f32) * (self.config.bar_width + self.config.bar_spacing);

            // Top bar (extends upward from center)
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x,
                        y: center_y - bar_height,
                        width: self.config.bar_width,
                        height: bar_height,
                    },
                    border: Border {
                        radius: self.config.corner_radius.into(),
                        ..Default::default()
                    },
                    shadow: Shadow::default(),
                },
                self.color,
            );

            // Bottom bar (mirrored)
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x,
                        y: center_y,
                        width: self.config.bar_width,
                        height: bar_height,
                    },
                    border: Border {
                        radius: self.config.corner_radius.into(),
                        ..Default::default()
                    },
                    shadow: Shadow::default(),
                },
                self.color,
            );
        }
    }
}

impl<'a, Message, Renderer> From<AnimatedSpectrum> for Element<'a, Message, Theme, Renderer>
where
    Renderer: advanced::Renderer,
{
    fn from(spectrum: AnimatedSpectrum) -> Self {
        Self::new(spectrum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spectrum_state_smoothing() {
        let mut state = SpectrumState::default();
        state.set_target([1.0; SPECTRUM_BANDS]);

        // Initial update should move towards target
        let now = Instant::now();
        state.update(now);

        // Values should be between 0 and 1
        assert!(state.current_values[0] > 0.0);
        assert!(state.current_values[0] < 1.0);
    }
}
