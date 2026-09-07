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
const SHADOW_BLUR_RADIUS: f32 = 8.0;
const SHADOW_SPREAD_RADIUS: f32 = 0.0;
const SHADOW_OFFSET_X: f32 = 0.0;
const SHADOW_OFFSET_Y: f32 = 2.0;

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

    /// Returns the minimum host overlay-window height that contains the pill's
    /// full gaussian shadow support, so the shadow is not hard-clipped at the
    /// surface edge.
    #[must_use]
    pub(crate) const fn min_overlay_window_height() -> f32 {
        // The pill is vertically centered by the host's flex container, so its
        // top edge sits at `(window - PILL_HEIGHT) / 2`. The shadow silhouette
        // is the pill translated by `offset` then dilated by `spread` (gpui's
        // `shadow_bounds = (bounds + offset).dilate(spread)`), and the paint
        // region extends `3 * blur_radius` beyond the silhouette on each side
        // (the 3σ gaussian support). Containment requires:
        //   top:    (window - PILL_HEIGHT)/2 + offset_y - spread - 3*blur >= 0
        //   bottom: (window - PILL_HEIGHT)/2 + PILL_HEIGHT + offset_y
        //           + spread + 3*blur <= window
        // Solving each for `window` and flooring at `PILL_HEIGHT` (the pill
        // itself must still fit) yields the binding constraint, which is the
        // bottom one whenever `offset_y >= 0`.
        let top =
            PILL_HEIGHT + 2.0 * (3.0 * SHADOW_BLUR_RADIUS - SHADOW_OFFSET_Y + SHADOW_SPREAD_RADIUS);
        let bottom =
            PILL_HEIGHT + 2.0 * (3.0 * SHADOW_BLUR_RADIUS + SHADOW_OFFSET_Y + SHADOW_SPREAD_RADIUS);
        top.max(bottom).max(PILL_HEIGHT)
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

    fn shadow_paint_region(window: f32) -> (f32, f32) {
        let pill_top = (window - PILL_HEIGHT) / 2.0;
        let pill_bottom = pill_top + PILL_HEIGHT;
        let silhouette_top = pill_top + SHADOW_OFFSET_Y - SHADOW_SPREAD_RADIUS;
        let silhouette_bottom = pill_bottom + SHADOW_OFFSET_Y + SHADOW_SPREAD_RADIUS;
        (
            silhouette_top - SHADOW_SUPPORT,
            silhouette_bottom + SHADOW_SUPPORT,
        )
    }

    #[test]
    fn min_overlay_window_height_contains_full_shadow_support() {
        let window = Panel::min_overlay_window_height();
        let (paint_top, paint_bottom) = shadow_paint_region(window);

        assert!(
            paint_top + TOLERANCE >= 0.0,
            "shadow top paint region {paint_top} leaks above the overlay window"
        );
        assert!(
            paint_bottom <= window + TOLERANCE,
            "shadow bottom paint region {paint_bottom} leaks below the overlay window"
        );
    }

    #[test]
    fn shrinking_window_below_min_clips_the_shadow() {
        // Regression guard: any window shorter than the minimum clips the
        // shadow on (at least) one side, which is the original bug.
        let window = Panel::min_overlay_window_height() - 1.0;
        let (paint_top, paint_bottom) = shadow_paint_region(window);
        let clipped_above = paint_top < -TOLERANCE;
        let clipped_below = paint_bottom > window + TOLERANCE;
        assert!(
            clipped_above || clipped_below,
            "a window of {window} should clip the shadow but the paint region {paint_top}..{paint_bottom} \
             fits in 0..{window}"
        );
    }
}
