//! OSD widgets
//!
//! Custom iced widgets for the overlay: status dot, spectrum visualizer,
//! timer display, and the main OSD bar.

use crate::audio::SPECTRUM_BANDS;
use crate::osd::colors;
use crate::RecordingStatus;
use iced::advanced;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::alignment::Vertical::Center;
use iced::mouse;
use iced::widget::{container, horizontal_space, mouse_area, row, text};
use iced::{Border, Color, Element, Length, Rectangle, Shadow, Size, Theme, Vector};

// ============================================================================
// Status Dot
// ============================================================================

pub struct StatusDot {
    radius: f32,
    color: Color,
}

pub fn status_dot(radius: f32, color: Color) -> StatusDot {
    StatusDot { radius, color }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for StatusDot
where
    Renderer: advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fixed(self.radius * 2.0),
            height: Length::Fixed(self.radius * 2.0),
        }
    }

    fn layout(&self, _: &mut widget::Tree, _: &Renderer, limits: &layout::Limits) -> layout::Node {
        let size = limits
            .width(Length::Fixed(self.radius * 2.0))
            .height(Length::Fixed(self.radius * 2.0))
            .resolve(Length::Fixed(self.radius * 2.0), Length::Fixed(self.radius * 2.0), Size::ZERO);
        layout::Node::new(size)
    }

    fn draw(&self, _: &widget::Tree, renderer: &mut Renderer, _: &Theme, _: &renderer::Style, layout: Layout<'_>, _: mouse::Cursor, _: &Rectangle) {
        let bounds = layout.bounds();
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x,
                    y: bounds.y,
                    width: self.radius * 2.0,
                    height: self.radius * 2.0,
                },
                border: Border { radius: self.radius.into(), ..Default::default() },
                shadow: Shadow::default(),
            },
            self.color,
        );
    }
}

impl<'a, Message, Renderer> From<StatusDot> for Element<'a, Message, Theme, Renderer>
where
    Renderer: advanced::Renderer,
{
    fn from(dot: StatusDot) -> Self {
        Self::new(dot)
    }
}

// ============================================================================
// Spectrum Waveform
// ============================================================================

pub struct SpectrumWaveform {
    bands: [f32; SPECTRUM_BANDS],
    color: Color,
}

pub fn spectrum_waveform(bands: [f32; SPECTRUM_BANDS], color: Color) -> SpectrumWaveform {
    SpectrumWaveform { bands, color }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for SpectrumWaveform
where
    Renderer: advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        let width = (SPECTRUM_BANDS as f32) * 5.0 - 2.0; // 3px bar + 2px gap
        Size {
            width: Length::Fixed(width),
            height: Length::Fixed(20.0),
        }
    }

    fn layout(&self, _: &mut widget::Tree, _: &Renderer, limits: &layout::Limits) -> layout::Node {
        let width = (SPECTRUM_BANDS as f32) * 5.0 - 2.0;
        let size = limits
            .width(Length::Fixed(width))
            .height(Length::Fixed(20.0))
            .resolve(Length::Fixed(width), Length::Fixed(20.0), Size::ZERO);
        layout::Node::new(size)
    }

    fn draw(&self, _: &widget::Tree, renderer: &mut Renderer, _: &Theme, _: &renderer::Style, layout: Layout<'_>, _: mouse::Cursor, _: &Rectangle) {
        let bounds = layout.bounds();
        let center_y = bounds.y + 10.0;
        let max_height = 9.0;

        for (i, &level) in self.bands.iter().enumerate() {
            let amplified = (level * 2.0).clamp(0.0, 1.0).powf(0.6);
            let normalized = amplified.max(0.05);
            let bar_height = normalized * max_height;
            let x = bounds.x + (i as f32) * 5.0;

            // Top bar
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle { x, y: center_y - bar_height, width: 3.0, height: bar_height },
                    border: Border { radius: 1.0.into(), ..Default::default() },
                    shadow: Shadow::default(),
                },
                self.color,
            );

            // Bottom bar (mirrored)
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle { x, y: center_y, width: 3.0, height: bar_height },
                    border: Border { radius: 1.0.into(), ..Default::default() },
                    shadow: Shadow::default(),
                },
                self.color,
            );
        }
    }
}

impl<'a, Message, Renderer> From<SpectrumWaveform> for Element<'a, Message, Theme, Renderer>
where
    Renderer: advanced::Renderer,
{
    fn from(waveform: SpectrumWaveform) -> Self {
        Self::new(waveform)
    }
}

// ============================================================================
// Timer Display
// ============================================================================

pub fn timer_display<'a, Message: 'a>(elapsed_secs: u32, timestamp_ms: u64) -> Element<'a, Message> {
    let show_colon = (timestamp_ms / 500) % 2 == 0;
    let sep = if show_colon { ":" } else { " " };
    let timer = format!("{}{}{:02}", elapsed_secs / 60, sep, elapsed_secs % 60);
    text(timer).size(14).color(colors::LIGHT_GRAY).into()
}

// ============================================================================
// OSD Bar
// ============================================================================

/// Visual state for rendering
pub struct OsdVisual {
    pub state: RecordingStatus,
    pub idle_hot: bool,
    pub pulse_alpha: f32,
    pub content_alpha: f32,
    pub window_opacity: f32,
    pub window_scale: f32,
    pub spectrum: [f32; SPECTRUM_BANDS],
    pub elapsed_secs: Option<u32>,
    pub timestamp_ms: u64,
}

fn state_color(state: RecordingStatus, idle_hot: bool, elapsed: Option<u32>) -> Color {
    if elapsed.unwrap_or(0) >= 25 {
        return colors::ORANGE;
    }
    match (state, idle_hot) {
        (RecordingStatus::Idle, false) => colors::GRAY,
        (RecordingStatus::Idle, true) => colors::DIM_GREEN,
        (RecordingStatus::Recording, _) => colors::RED,
        (RecordingStatus::Transcribing, _) => colors::BLUE,
        (RecordingStatus::Error, _) => colors::ORANGE,
    }
}

pub fn osd_bar<'a, Message: Clone + 'a>(
    visual: &OsdVisual,
    on_enter: Message,
    on_exit: Message,
) -> Element<'a, Message> {
    let color = state_color(visual.state, visual.idle_hot, visual.elapsed_secs);

    // Status display (dot + text)
    let dot_alpha = visual.pulse_alpha * visual.content_alpha;
    let dot_color = if visual.elapsed_secs.unwrap_or(0) >= 25 {
        colors::ORANGE
    } else {
        color
    };

    let status = row![
        status_dot(8.0, Color { a: dot_alpha, ..dot_color }),
        text(visual.state.as_str())
            .size(14)
            .color(colors::with_alpha(colors::LIGHT_GRAY, visual.content_alpha))
    ]
    .spacing(8)
    .align_y(Center);

    // Audio display (timer + waveform) - only when recording
    let content = if visual.state == RecordingStatus::Recording {
        let display_bands = if visual.spectrum.iter().any(|&v| v > 0.0) {
            visual.spectrum
        } else {
            // Pulsing placeholder while initializing
            let pulse = ((visual.timestamp_ms as f32 / 300.0).sin() + 1.0) / 2.0;
            [0.15 + pulse * 0.1; SPECTRUM_BANDS]
        };

        let waveform = spectrum_waveform(
            display_bands,
            Color { a: visual.content_alpha, ..color },
        );
        let timer = timer_display(visual.elapsed_secs.unwrap_or(0), visual.timestamp_ms);

        row![status, horizontal_space(), timer, waveform]
            .spacing(8)
            .align_y(Center)
    } else {
        row![status].align_y(Center)
    };

    let padded_content = content.padding([6, 12]);

    let scaled_width = 420.0 * visual.window_scale;
    let scaled_height = 36.0 * visual.window_scale;
    let bg_alpha = 0.94 * visual.window_opacity;
    let shadow_alpha = 0.35 * visual.window_opacity;

    let styled_bar = container(padded_content)
        .width(Length::Fixed(scaled_width))
        .height(Length::Fixed(scaled_height))
        .center_y(scaled_height)
        .style(move |_| container::Style {
            background: Some(colors::with_alpha(colors::DARK_GRAY, bg_alpha).into()),
            border: Border {
                radius: (12.0 * visual.window_scale).into(),
                ..Default::default()
            },
            shadow: Shadow {
                color: colors::with_alpha(colors::BLACK, shadow_alpha),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 12.0,
            },
            ..Default::default()
        });

    let interactive = mouse_area(styled_bar)
        .on_enter(on_enter)
        .on_exit(on_exit);

    container(interactive)
        .padding(10)
        .center(Length::Fill)
        .into()
}
