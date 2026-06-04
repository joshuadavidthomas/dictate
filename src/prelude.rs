pub use gpui::AnyElement;
pub use gpui::App;
pub use gpui::BoxShadow;
pub use gpui::Context;
pub use gpui::Div;
pub use gpui::ElementId;
pub use gpui::Entity;
pub use gpui::FontFeatures;
pub use gpui::IntoElement;
pub use gpui::ParentElement;
pub use gpui::Render;
pub use gpui::RenderOnce;
pub use gpui::SharedString;
pub use gpui::Window;
pub use gpui::div;
pub use gpui::hsla;
pub use gpui::point;
pub use gpui::prelude::*;
pub use gpui::px;
pub use gpui::rgba;
pub use gpui::size;
pub use gpui::transparent_black;

/// Horizontally stacks elements. Sets `flex()`, `flex_row()`, and `items_center()`.
pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// Vertically stacks elements. Sets `flex()` and `flex_col()`.
pub fn v_flex() -> Div {
    div().flex().flex_col()
}
