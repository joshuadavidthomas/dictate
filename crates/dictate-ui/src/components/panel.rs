use gpui::AnyElement;
use gpui::App;
use gpui::BoxShadow;
use gpui::ElementId;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::RenderOnce;
use gpui::Role;
use gpui::SharedString;
use gpui::Window;
use gpui::div;
use gpui::hsla;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::rgba;
use gpui::transparent_black;

const PADDING_X: f32 = 12.0;
const PADDING_Y: f32 = 8.0;
const GAP: f32 = 8.0;
const PILL_HEIGHT: f32 = 36.0;
const SHADOW_BLUR_RADIUS: f32 = 4.0;
const SHADOW_SPREAD_RADIUS: f32 = 0.0;
const SHADOW_OFFSET_X: f32 = 0.0;
const SHADOW_OFFSET_Y: f32 = 1.0;

#[derive(IntoElement)]
pub struct Panel {
    id: ElementId,
    width: f32,
    label: SharedString,
    children: Vec<AnyElement>,
}

impl Panel {
    pub fn new(id: impl Into<SharedString>, width: f32, label: impl Into<SharedString>) -> Self {
        let id = id.into();

        Self {
            id: ElementId::Name(id),
            width,
            label: label.into(),
            children: Vec::new(),
        }
    }

    /// Returns the minimum host overlay-window width that contains the pill's
    /// full gaussian shadow support.
    #[must_use]
    pub(crate) const fn min_overlay_window_width(pill_width: f32) -> f32 {
        Self::min_host_size(pill_width, SHADOW_OFFSET_X)
    }

    /// Returns the minimum host overlay-window height that contains the pill's
    /// full gaussian shadow support.
    #[must_use]
    pub(crate) const fn min_overlay_window_height() -> f32 {
        Self::min_host_size(PILL_HEIGHT, SHADOW_OFFSET_Y)
    }

    const fn min_host_size(pill_size: f32, shadow_offset: f32) -> f32 {
        // The pill is centered by the host's flex container. GPUI translates
        // its shadow by `offset`, dilates it by `spread`, and paints 3σ beyond
        // that silhouette. The host therefore needs symmetric slack for the
        // support plus whichever side the offset favors.
        pill_size + 2.0 * (3.0 * SHADOW_BLUR_RADIUS + shadow_offset.abs() + SHADOW_SPREAD_RADIUS)
    }
}

impl ParentElement for Panel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Panel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .id(self.id)
            .size_full()
            .bg(transparent_black())
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("dictate-overlay-status")
                    .role(Role::Status)
                    .aria_label(self.label)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .w(px(self.width))
                    .h(px(PILL_HEIGHT))
                    .rounded_full()
                    .px(px(PADDING_X))
                    .py(px(PADDING_Y))
                    .gap(px(GAP))
                    .bg(rgba(0x1e1e_1ef0))
                    .shadow(vec![BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.35),
                        blur_radius: px(SHADOW_BLUR_RADIUS),
                        spread_radius: px(SHADOW_SPREAD_RADIUS),
                        offset: point(px(SHADOW_OFFSET_X), px(SHADOW_OFFSET_Y)),
                        inset: false,
                    }])
                    .children(self.children),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f32 = 1e-4;
    const SHADOW_SUPPORT: f32 = 3.0 * SHADOW_BLUR_RADIUS;

    fn shadow_paint_region(window: f32, pill: f32, offset: f32) -> (f32, f32) {
        let pill_start = (window - pill) / 2.0;
        let pill_end = pill_start + pill;
        let silhouette_start = pill_start + offset - SHADOW_SPREAD_RADIUS;
        let silhouette_end = pill_end + offset + SHADOW_SPREAD_RADIUS;
        (
            silhouette_start - SHADOW_SUPPORT,
            silhouette_end + SHADOW_SUPPORT,
        )
    }

    #[test]
    fn minimum_overlay_window_contains_full_shadow_support() {
        let width = Panel::min_overlay_window_width(crate::overlay::PILL_WIDTH);
        let (paint_left, paint_right) =
            shadow_paint_region(width, crate::overlay::PILL_WIDTH, SHADOW_OFFSET_X);
        let height = Panel::min_overlay_window_height();
        let (paint_top, paint_bottom) = shadow_paint_region(height, PILL_HEIGHT, SHADOW_OFFSET_Y);

        assert!(
            paint_left + TOLERANCE >= 0.0 && paint_right <= width + TOLERANCE,
            "horizontal shadow paint region {paint_left}..{paint_right} leaks outside 0..{width}"
        );
        assert!(
            paint_top + TOLERANCE >= 0.0 && paint_bottom <= height + TOLERANCE,
            "vertical shadow paint region {paint_top}..{paint_bottom} leaks outside 0..{height}"
        );
    }

    #[test]
    fn shrinking_either_window_dimension_clips_the_shadow() {
        let width = Panel::min_overlay_window_width(crate::overlay::PILL_WIDTH) - 1.0;
        let (paint_left, paint_right) =
            shadow_paint_region(width, crate::overlay::PILL_WIDTH, SHADOW_OFFSET_X);
        let height = Panel::min_overlay_window_height() - 1.0;
        let (paint_top, paint_bottom) = shadow_paint_region(height, PILL_HEIGHT, SHADOW_OFFSET_Y);

        assert!(
            paint_left < -TOLERANCE || paint_right > width + TOLERANCE,
            "horizontal shadow paint region {paint_left}..{paint_right} unexpectedly fits in 0..{width}"
        );
        assert!(
            paint_top < -TOLERANCE || paint_bottom > height + TOLERANCE,
            "vertical shadow paint region {paint_top}..{paint_bottom} unexpectedly fits in 0..{height}"
        );
    }
}
