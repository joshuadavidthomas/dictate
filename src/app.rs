use gpui::App;
use gpui::Bounds;
use gpui::WindowBackgroundAppearance;
use gpui::WindowBounds;
use gpui::WindowKind;
use gpui::WindowOptions;
use gpui::layer_shell::*;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::size;
use gpui_platform::application;

use crate::overlay::Overlay;

const WINDOW_WIDTH: f32 = 220.0;
const WINDOW_HEIGHT: f32 = 64.0;
const BOTTOM_MARGIN: f32 = 40.0;

pub fn run() {
    application().run(|cx: &mut App| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.0), px(0.0)),
                    size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
                ))),
                titlebar: None,
                focus: false,
                is_resizable: false,
                is_minimizable: false,
                app_id: Some("dev.joshthomas.dictate.gpui".to_string()),
                window_background: WindowBackgroundAppearance::Transparent,
                kind: WindowKind::LayerShell(LayerShellOptions {
                    namespace: "dictate-osd".to_string(),
                    layer: Layer::Overlay,
                    anchor: Anchor::BOTTOM,
                    margin: Some((px(0.0), px(0.0), px(BOTTOM_MARGIN), px(0.0))),
                    keyboard_interactivity: KeyboardInteractivity::None,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(Overlay::new),
        )
        .unwrap();
    });
}
