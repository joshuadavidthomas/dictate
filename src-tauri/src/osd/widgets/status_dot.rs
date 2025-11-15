use iced::advanced;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::mouse;
use iced::{Border, Color, Element, Length, Rectangle, Shadow, Size, Theme};

/// Create a status dot widget
pub fn status_dot(radius: f32, color: Color) -> StatusDot {
    StatusDot { radius, color }
}

/// A circular status indicator dot
pub struct StatusDot {
    radius: f32,
    color: Color,
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
            .resolve(
                Length::Fixed(self.radius * 2.0),
                Length::Fixed(self.radius * 2.0),
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
