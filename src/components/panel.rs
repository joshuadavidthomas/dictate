use gpui::AnyElement;
use gpui::App;
use gpui::BoxShadow;
use gpui::ElementId;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::RenderOnce;
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
    children: Vec<AnyElement>,
}

impl Panel {
    pub fn new(id: impl Into<SharedString>) -> Self {
        let id = id.into();

        Self {
            id: ElementId::Name(id),
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
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded_full()
                    .px(px(PADDING_X))
                    .py(px(PADDING_Y))
                    .gap(px(GAP))
                    .bg(rgba(0x1e1e1ef0))
                    .shadow(vec![BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.35),
                        blur_radius: px(12.0),
                        spread_radius: px(0.0),
                        offset: point(px(0.0), px(2.0)),
                        inset: false,
                    }])
                    .children(self.children),
            )
    }
}
