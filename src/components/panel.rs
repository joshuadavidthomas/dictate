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
                    .h(px(36.0))
                    .rounded_full()
                    .px(px(PADDING_X))
                    .py(px(PADDING_Y))
                    .gap(px(GAP))
                    .bg(rgba(0x1e1e_1ef0))
                    .shadow(vec![BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.35),
                        blur_radius: px(8.0),
                        spread_radius: px(0.0),
                        offset: point(px(0.0), px(2.0)),
                        inset: false,
                    }])
                    .children(self.children),
            )
    }
}
