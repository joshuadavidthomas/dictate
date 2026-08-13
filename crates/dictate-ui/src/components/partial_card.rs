use gpui::App;
use gpui::BoxShadow;
use gpui::IntoElement;
use gpui::RenderOnce;
use gpui::Role;
use gpui::ScrollHandle;
use gpui::SharedString;
use gpui::Window;
use gpui::div;
use gpui::hsla;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::rgba;
use gpui::transparent_black;

use crate::partial::PartialTextStyle;

const CARD_WIDTH: f32 = 404.0;
const MAX_VISIBLE_LINES: f32 = 4.0;
const PADDING_X: f32 = 12.0;
const PADDING_Y: f32 = 8.0;
const LINE_HEIGHT_RATIO: f32 = 1.4;
const SCROLLBAR_INSET: f32 = 5.0;
const SCROLLBAR_WIDTH: f32 = 3.0;
const MINIMUM_THUMB_HEIGHT: f32 = 14.0;

#[derive(IntoElement)]
pub struct PartialCard {
    text: SharedString,
    style: PartialTextStyle,
    scroll_handle: ScrollHandle,
    opacity: f32,
}

impl PartialCard {
    pub fn new(
        text: impl Into<SharedString>,
        style: PartialTextStyle,
        scroll_handle: ScrollHandle,
        opacity: f32,
    ) -> Self {
        Self {
            text: text.into(),
            style,
            scroll_handle,
            opacity,
        }
    }
}

impl RenderOnce for PartialCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let label = self.text.clone();
        let line_height = self.style.font_size() * LINE_HEIGHT_RATIO;
        let minimum_height = line_height + 2.0 * PADDING_Y;
        let maximum_height = line_height * MAX_VISIBLE_LINES + 2.0 * PADDING_Y;
        let bounds = self.scroll_handle.bounds();
        let viewport_height = bounds.size.height;
        let overflow = self.scroll_handle.max_offset().y;
        let scrollbar = if overflow > px(0.0) && viewport_height > px(2.0 * SCROLLBAR_INSET) {
            let track_height = viewport_height - px(2.0 * SCROLLBAR_INSET);
            let content_height = viewport_height + overflow;
            let thumb_height = (track_height * (viewport_height / content_height))
                .max(px(MINIMUM_THUMB_HEIGHT))
                .min(track_height);
            let progress = (-self.scroll_handle.offset().y / overflow).clamp(0.0, 1.0);
            let thumb_top = (track_height - thumb_height) * progress;

            Some(
                div()
                    .absolute()
                    .top(px(SCROLLBAR_INSET))
                    .right(px(SCROLLBAR_INSET))
                    .h(track_height)
                    .w(px(SCROLLBAR_WIDTH))
                    .rounded_full()
                    .bg(rgba(0xffff_ff24))
                    .child(
                        div()
                            .absolute()
                            .top(thumb_top)
                            .h(thumb_height)
                            .w_full()
                            .rounded_full()
                            .bg(rgba(0xffff_ff8c)),
                    ),
            )
        } else {
            None
        };

        let mut card = div()
            .id("dictate-partials-status")
            .role(Role::Status)
            .aria_label(label)
            .relative()
            .flex()
            .flex_col()
            .w(px(CARD_WIDTH))
            .min_h(px(minimum_height))
            .max_h(px(maximum_height))
            .px(px(PADDING_X))
            .py(px(PADDING_Y))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.scroll_handle)
            .whitespace_normal()
            .rounded(px(14.0))
            .bg(rgba(0x1e1e_1ef0))
            .shadow(vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.35),
                blur_radius: px(8.0),
                spread_radius: px(0.0),
                offset: point(px(0.0), px(2.0)),
                inset: false,
            }])
            .text_size(px(self.style.font_size()))
            .line_height(px(line_height))
            .text_color(hsla(0.0, 0.0, 0.90, 0.92))
            .opacity(self.opacity)
            .child(self.text);
        if let Some(font_family) = self.style.font_family() {
            card = card.font_family(font_family.to_owned());
        }
        if let Some(scrollbar) = scrollbar {
            card = card.child(scrollbar);
        }

        div()
            .flex()
            .size_full()
            .items_end()
            .justify_center()
            .bg(transparent_black())
            .child(card)
    }
}
