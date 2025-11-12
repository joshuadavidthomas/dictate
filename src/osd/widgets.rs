//! Custom widgets for the OSD

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::advanced;
use iced::mouse;
use iced::{Border, Color, Element, Length, Rectangle, Shadow, Size, Theme};

/// A circular status indicator dot
pub struct StatusDot {
    radius: f32,
    color: Color,
}

impl StatusDot {
    pub fn new(radius: f32, color: Color) -> Self {
        Self { radius, color }
    }
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

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits
            .width(Length::Fixed(self.radius * 2.0))
            .height(Length::Fixed(self.radius * 2.0))
            .resolve(Length::Fixed(self.radius * 2.0), Length::Fixed(self.radius * 2.0), Size::ZERO);

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

        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x,
                    y: bounds.y,
                    width: self.radius * 2.0,
                    height: self.radius * 2.0,
                },
                border: Border {
                    radius: self.radius.into(),
                    ..Default::default()
                },
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
    fn from(status_dot: StatusDot) -> Self {
        Self::new(status_dot)
    }
}

/// Create a status dot widget
pub fn status_dot(radius: f32, color: Color) -> StatusDot {
    StatusDot::new(radius, color)
}

/// A waveform visualization widget showing audio levels as vertical bars
pub struct Waveform {
    bars: [f32; 10],
    color: Color,
    bar_width: f32,
    bar_spacing: f32,
    max_height: f32,
}

impl Waveform {
    pub fn new(bars: [f32; 10], color: Color) -> Self {
        Self {
            bars,
            color,
            bar_width: 3.0,
            bar_spacing: 2.0,
            max_height: 20.0,
        }
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for Waveform
where
    Renderer: advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        let total_width = (self.bars.len() as f32) * (self.bar_width + self.bar_spacing) - self.bar_spacing;
        Size {
            width: Length::Fixed(total_width),
            height: Length::Fixed(self.max_height),
        }
    }

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let total_width = (self.bars.len() as f32) * (self.bar_width + self.bar_spacing) - self.bar_spacing;
        let size = limits
            .width(Length::Fixed(total_width))
            .height(Length::Fixed(self.max_height))
            .resolve(Length::Fixed(total_width), Length::Fixed(self.max_height), Size::ZERO);

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

        for (i, &level) in self.bars.iter().enumerate() {
            // Apply amplification formula from original code
            let amplified = (level * 6.0).clamp(0.0, 1.0).powf(0.4);
            let normalized = amplified.max(0.15); // 15% minimum
            
            let bar_height = normalized * self.max_height;
            let x = bounds.x + (i as f32) * (self.bar_width + self.bar_spacing);
            let y = bounds.y + (self.max_height - bar_height);

            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x,
                        y,
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

impl<'a, Message, Renderer> From<Waveform> for Element<'a, Message, Theme, Renderer>
where
    Renderer: advanced::Renderer,
{
    fn from(waveform: Waveform) -> Self {
        Self::new(waveform)
    }
}

/// Create a waveform widget
pub fn waveform(bars: [f32; 10], color: Color) -> Waveform {
    Waveform::new(bars, color)
}
