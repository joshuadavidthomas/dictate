use gpui::AnyElement;
use gpui::App;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::rgb;

pub trait DebugComponent {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn scenarios(&self) -> &'static [&'static str];
    fn preview(&self, scenario: &str, window: &mut Window, cx: &mut App) -> AnyElement;
}

pub fn registry() -> Vec<Box<dyn DebugComponent>> {
    vec![Box::new(StubScreen)]
}

struct StubScreen;

impl DebugComponent for StubScreen {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn description(&self) -> &'static str {
        "Phase 2 debug harness stub screen."
    }

    fn scenarios(&self) -> &'static [&'static str] {
        &["default"]
    }

    fn preview(&self, scenario: &str, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .id("debug-stub-preview")
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x3b4252))
            .bg(rgb(0x111827))
            .p(px(24.0))
            .text_color(rgb(0xe5e7eb))
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Debug harness stub"),
            )
            .child(div().text_sm().child(format!("scenario: {scenario}")))
            .child(
                div()
                    .text_sm()
                    .child("Screens added in later phases will render here."),
            )
            .into_any_element()
    }
}
