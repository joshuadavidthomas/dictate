use crate::audio::SPECTRUM_BANDS;
use iced::advanced;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::mouse;
use iced::{Border, Color, Element, Length, Rectangle, Shadow, Size, Theme};

/// Create a spectrum waveform widget
pub fn spectrum_waveform(bands: [f32; SPECTRUM_BANDS], color: Color) -> SpectrumWaveform {
    SpectrumWaveform {
        bands,
        color,
        bar_width: 3.0,
        bar_spacing: 2.0,
        total_height: 20.0,
        max_bar_height: 9.0, // Leaves 1px for center line
    }
}

/// A spectrum analyzer widget showing frequency bands as mirrored vertical bars
pub struct SpectrumWaveform {
    bands: [f32; SPECTRUM_BANDS],
    color: Color,
    bar_width: f32,
    bar_spacing: f32,
    total_height: f32,
    max_bar_height: f32,
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for SpectrumWaveform
where
    Renderer: advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        let total_width =
            (self.bands.len() as f32) * (self.bar_width + self.bar_spacing) - self.bar_spacing;
        Size {
            width: Length::Fixed(total_width),
            height: Length::Fixed(self.total_height),
        }
    }

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let total_width =
            (self.bands.len() as f32) * (self.bar_width + self.bar_spacing) - self.bar_spacing;
        let size = limits
            .width(Length::Fixed(total_width))
            .height(Length::Fixed(self.total_height))
            .resolve(
                Length::Fixed(total_width),
                Length::Fixed(self.total_height),
                Size::ZERO,
            );

        layout::Node::new(size)
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let center_y = bounds.y + (self.total_height / 2.0);

        for (i, &level) in self.bands.iter().enumerate() {
            // Apply amplification and minimum height
            // Reduced amplification since we now have better normalization
            let amplified = (level * 2.0).clamp(0.0, 1.0).powf(0.6);
            let normalized = amplified.max(0.05); // 5% minimum for true silence

            let bar_height = normalized * self.max_bar_height;
            let x = bounds.x + (i as f32) * (self.bar_width + self.bar_spacing);

            // Top bar (extends upward from center)
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x,
                        y: center_y - bar_height,
                        width: self.bar_width,
                        height: bar_height,
                    },
                    border: Border {
                        radius: 1.0.into(),
                        ..Default::default()
                    },
                    shadow: Shadow::default(),
                },
                self.color,
            );

            // Bottom bar (extends downward from center, mirrored)
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x,
                        y: center_y,
                        width: self.bar_width,
                        height: bar_height,
                    },
                    border: Border {
                        radius: 1.0.into(),
                        ..Default::default()
                    },
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
    fn from(spectrum: SpectrumWaveform) -> Self {
        Self::new(spectrum)
    }
}
