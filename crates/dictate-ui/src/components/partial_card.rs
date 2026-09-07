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
                    .debug_selector(|| "partial-card-scrollbar".into())
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

        let mut scroller = div()
            .id("dictate-partials-status")
            .role(Role::Status)
            .aria_label(label)
            .flex()
            .flex_col()
            .size_full()
            .px(px(PADDING_X))
            .py(px(PADDING_Y))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.scroll_handle)
            .whitespace_normal()
            .text_size(px(self.style.font_size()))
            .line_height(px(line_height))
            .text_color(hsla(0.0, 0.0, 0.90, 0.92))
            .child(self.text);
        if let Some(font_family) = self.style.font_family() {
            scroller = scroller.font_family(font_family.to_owned());
        }

        let mut card = div()
            .relative()
            .debug_selector(|| "partial-card".into())
            .w(px(CARD_WIDTH))
            .min_h(px(minimum_height))
            .max_h(px(maximum_height))
            .rounded(px(14.0))
            .bg(rgba(0x1e1e_1ef0))
            .shadow(vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.35),
                blur_radius: px(8.0),
                spread_radius: px(0.0),
                offset: point(px(0.0), px(2.0)),
                inset: false,
            }])
            .opacity(self.opacity)
            .child(scroller);
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

#[cfg(test)]
mod tests {
    use gpui::Context;
    use gpui::Render;
    use gpui::TestAppContext;
    use gpui::size;

    use super::*;

    const PROSE: &str = "the quick brown fox jumps over the lazy dog while the cat watches from the windowsill and the dog barks loudly at the passing cars on the street below the house where the family lives quietly with their pets and their books and their music playing softly in the background";

    struct CardHost {
        text: SharedString,
        style: PartialTextStyle,
        scroll_handle: ScrollHandle,
        opacity: f32,
    }

    impl Render for CardHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            PartialCard::new(
                self.text.clone(),
                self.style.clone(),
                self.scroll_handle.clone(),
                self.opacity,
            )
        }
    }

    #[gpui::test]
    fn scrollbar_marker_stays_pinned_to_card_top_when_scrolled_to_bottom(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let scroll_handle = ScrollHandle::new();
        scroll_handle.scroll_to_bottom();

        let view = cx.new(|_| CardHost {
            text: PROSE.into(),
            style: PartialTextStyle::default(),
            scroll_handle: scroll_handle.clone(),
            opacity: 1.0,
        });

        cx.draw(point(px(0.), px(0.)), size(px(420.), px(160.)), |_, _| {
            view.clone().into_any_element()
        });

        scroll_handle.scroll_to_bottom();

        cx.draw(point(px(0.), px(0.)), size(px(420.), px(160.)), |_, _| {
            view.clone().into_any_element()
        });

        let card_bounds = cx
            .debug_bounds("partial-card")
            .expect("partial-card host should have measured bounds");
        let marker_bounds = cx
            .debug_bounds("partial-card-scrollbar")
            .expect("scrollbar should be composed once the partial text overflows the card");

        let max_offset = scroll_handle.max_offset();
        let offset = scroll_handle.offset();
        assert!(
            max_offset.y > px(SCROLLBAR_INSET),
            "fixture should overflow past the scrollbar inset, got max_offset.y = {max_offset:?}",
        );
        assert!(
            (f32::from(offset.y) + f32::from(max_offset.y)).abs() < 0.5,
            "card should be pinned to the bottom: offset.y == -max_offset.y",
        );

        let marker_inset = f32::from(marker_bounds.top()) - f32::from(card_bounds.top());
        assert!(
            (marker_inset - SCROLLBAR_INSET).abs() < 0.5,
            "scrollbar top should stay SCROLLBAR_INSET below the card top across scroll_to_bottom, got marker_inset = {marker_inset}",
        );
        assert!(
            marker_bounds.top() >= card_bounds.top(),
            "scrollbar top should not be clipped above the card top",
        );
        assert!(
            marker_bounds.bottom() <= card_bounds.bottom(),
            "scrollbar bottom should not be clipped below the card bottom",
        );
    }
}
