use crate::prelude::*;

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
        v_flex()
            .id(self.id)
            .size_full()
            .bg(transparent_black())
            .items_center()
            .justify_center()
            .child(
                h_flex()
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
