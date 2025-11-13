use iced::widget::{container, mouse_area};
use iced::{Element, Length, Shadow, Vector};

use crate::ui::colors;

/// Visual configuration for the OSD bar styling
pub struct OsdBarStyle {
    pub width: f32,
    pub height: f32,
    pub window_scale: f32,
    pub window_opacity: f32,
}

/// Create a styled OSD bar container with background, border radius, shadow,
/// mouse interaction, and padding for shadow rendering
pub fn styled_osd_bar<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    style: OsdBarStyle,
    on_mouse_entered: Message,
    on_mouse_exited: Message,
) -> Element<'a, Message>
where
    Message: Clone,
{
    let scaled_width = style.width * style.window_scale;
    let scaled_height = style.height * style.window_scale;

    // Apply window opacity to background (alpha is f32 0.0-1.0)
    let bg_alpha = 0.94 * style.window_opacity;
    let shadow_alpha = 0.35 * style.window_opacity;

    let styled_bar = container(content)
        .width(Length::Fixed(scaled_width))
        .height(Length::Fixed(scaled_height))
        .center_y(scaled_height)
        .style(move |_theme| container::Style {
            background: Some(colors::with_alpha(colors::DARK_GRAY, bg_alpha).into()),
            border: iced::Border {
                radius: (12.0 * style.window_scale).into(),
                ..Default::default()
            },
            shadow: Shadow {
                color: colors::with_alpha(colors::BLACK, shadow_alpha),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 12.0,
            },
            ..Default::default()
        });

    // Wrap the styled bar with mouse_area FIRST, before outer container
    // This ensures mouse events track the actual visual bounds of the widget
    let interactive_bar = mouse_area(styled_bar)
        .on_enter(on_mouse_entered)
        .on_exit(on_mouse_exited);

    // Then wrap in outer container with padding for shadow space
    container(interactive_bar)
        .padding(10) // Padding to give shadow room to render
        .center(Length::Fill)
        .into()
}
