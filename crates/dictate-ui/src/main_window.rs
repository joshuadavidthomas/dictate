use std::sync::Arc;
use std::sync::LazyLock;

use gpui::AnyElement;
use gpui::Context;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::FontWeight;
use gpui::Image;
use gpui::ImageFormat;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Role;
use gpui::SharedString;
use gpui::Window;
use gpui::div;
use gpui::img;
use gpui::prelude::*;
use gpui::px;
use gpui::rgb;

const SIDEBAR_WIDTH: f32 = 184.0;
const BACKGROUND: u32 = 0x00ff_fdf9;
const SIDEBAR_BACKGROUND: u32 = 0x00f7_f1e7;
const TEXT: u32 = 0x0030_2b27;
const MUTED_TEXT: u32 = 0x006c_645d;
const FAINT_TEXT: u32 = 0x0087_7e75;
const BORDER: u32 = 0x00de_d6cc;
const SELECTED_BACKGROUND: u32 = 0x00fc_e4d2;
const ORANGE: u32 = 0x00ff_8a25;
const VALUE_TEXT: u32 = 0x0061_4430;
const LOCAL_GREEN: u32 = 0x0021_a46b;
static DICTATE_MARK: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        include_bytes!("../../../packaging/icons/platform/dictate-mark.svg").to_vec(),
    ))
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MainWindowSettings {
    pub model: SharedString,
    pub partials_model: SharedString,
    pub mode: SharedString,
    pub delivery: SharedString,
    pub microphone: SharedString,
    pub shortcut: SharedString,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Page {
    History,
    #[default]
    Settings,
}

pub struct MainWindow {
    page: Page,
    settings: MainWindowSettings,
    focus_handle: FocusHandle,
}

impl MainWindow {
    pub fn new(settings: MainWindowSettings, cx: &mut Context<Self>) -> Self {
        Self {
            page: Page::Settings,
            settings,
            focus_handle: cx.focus_handle().tab_stop(false),
        }
    }

    fn select_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }

    fn nav_item(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: &'static str,
        page: Page,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.page == page;
        div()
            .id(id)
            .debug_selector(|| id.to_owned())
            .role(Role::Button)
            .aria_label(label)
            .tab_index(match page {
                Page::History => 0,
                Page::Settings => 1,
            })
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .px(px(12.0))
            .py(px(10.0))
            .rounded_md()
            .cursor_pointer()
            .bg(if selected {
                rgb(SELECTED_BACKGROUND)
            } else {
                rgb(SIDEBAR_BACKGROUND)
            })
            .text_color(if selected { rgb(TEXT) } else { rgb(MUTED_TEXT) })
            .hover(|style| style.bg(rgb(0x00f2_e8dc)).text_color(rgb(TEXT)))
            .focus_visible(|style| style.border_2().border_color(rgb(ORANGE)))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.select_page(page, cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.select_page(page, cx)))
            .child(
                div()
                    .w(px(18.0))
                    .text_center()
                    .text_sm()
                    .text_color(if selected {
                        rgb(ORANGE)
                    } else {
                        rgb(MUTED_TEXT)
                    })
                    .child(glyph),
            )
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
            .into_any_element()
    }

    fn render_sidebar(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .w(px(if compact { 128.0 } else { SIDEBAR_WIDTH }))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR_BACKGROUND))
            .p(px(if compact { 12.0 } else { 16.0 }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(6.0))
                    .pb(px(22.0))
                    .child(spectrum_mark("brand-mark", 28.0))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT))
                            .child("Dictate"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.nav_item("nav-history", "History", "◷", Page::History, cx))
                    .child(self.nav_item("nav-settings", "Settings", "⚙", Page::Settings, cx)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .pt(px(14.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(MUTED_TEXT))
                    .child(div().size(px(7.0)).rounded_full().bg(rgb(LOCAL_GREEN)))
                    .child(if compact { "Local" } else { "Local processing" }),
            )
    }

    fn render_settings(&self, compact: bool) -> impl IntoElement {
        div()
            .id("settings-page")
            .debug_selector(|| "settings-page".into())
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(680.0))
            .gap(px(if compact { 18.0 } else { 30.0 }))
            .child(page_heading(
                "Settings",
                "Review the configuration Dictate loaded when the daemon started.",
                compact,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(if compact { 20.0 } else { 30.0 }))
                    .child(settings_section(
                        "Transcription",
                        "Choose how speech is recognized and formatted.",
                        compact,
                        vec![
                            setting_row(
                                "Final model",
                                "Produces the completed transcript",
                                self.settings.model.clone(),
                                compact,
                            ),
                            setting_row(
                                "Live model",
                                "Shows partial text while you speak",
                                self.settings.partials_model.clone(),
                                compact,
                            ),
                            setting_row(
                                "Writing mode",
                                "Controls punctuation and cleanup",
                                self.settings.mode.clone(),
                                compact,
                            ),
                        ],
                    ))
                    .child(settings_section(
                        "Input & delivery",
                        "Control where Dictate listens and sends text.",
                        compact,
                        vec![
                            setting_row(
                                "Microphone",
                                "Audio input used for new recordings",
                                self.settings.microphone.clone(),
                                compact,
                            ),
                            setting_row(
                                "Delivery",
                                "Where completed dictation is sent",
                                self.settings.delivery.clone(),
                                compact,
                            ),
                            setting_row(
                                "Push to talk",
                                "Portal-managed global shortcut",
                                self.settings.shortcut.clone(),
                                compact,
                            ),
                        ],
                    )),
            )
            .child(
                div()
                    .border_l_2()
                    .border_color(rgb(ORANGE))
                    .pl(px(12.0))
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(rgb(MUTED_TEXT))
                    .child(
                        "Editing controls and live reload will arrive in the next settings slice.",
                    ),
            )
    }

    fn render_history() -> impl IntoElement {
        div()
            .id("history-page")
            .debug_selector(|| "history-page".into())
            .flex()
            .flex_col()
            .h_full()
            .child(page_heading(
                "History",
                "Find and reuse your recent dictations.",
                false,
            ))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(400.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .text_center()
                            .child(
                                div()
                                    .mb(px(18.0))
                                    .child(spectrum_mark("history-empty-mark", 48.0)),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child("No history yet"),
                            )
                            .child(
                                div()
                                    .mt(px(8.0))
                                    .text_sm()
                                    .line_height(px(21.0))
                                    .text_color(rgb(MUTED_TEXT))
                                    .child("Completed dictations will appear here once history storage is enabled."),
                            ),
                    ),
            )
    }
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = window.viewport_size().width < px(700.0);
        let content = match self.page {
            Page::History => Self::render_history().into_any_element(),
            Page::Settings => self.render_settings(compact).into_any_element(),
        };

        div()
            .flex()
            .size_full()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .track_focus(&self.focus_handle)
            .tab_stop(false)
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(self.render_sidebar(compact, cx))
            .child(
                div()
                    .id("main-content")
                    .debug_selector(|| "main-content".into())
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px(px(if compact { 16.0 } else { 36.0 }))
                    .py(px(if compact { 18.0 } else { 34.0 }))
                    .child(content),
            )
    }
}

fn page_heading(title: &'static str, description: &'static str, compact: bool) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(title),
        )
        .child(
            div()
                .text_size(px(if compact { 13.0 } else { 14.0 }))
                .line_height(px(if compact { 18.0 } else { 20.0 }))
                .text_color(rgb(MUTED_TEXT))
                .child(description),
        )
}

fn spectrum_mark(id: &'static str, size: f32) -> impl IntoElement {
    img(Arc::clone(&DICTATE_MARK))
        .id(id)
        .debug_selector(move || id.to_owned())
        .size(px(size))
        .flex_none()
}

fn settings_section(
    title: &'static str,
    description: &'static str,
    compact: bool,
    rows: Vec<AnyElement>,
) -> gpui::Div {
    let heading = div().flex().flex_col().gap_2().child(
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(VALUE_TEXT))
            .child(title.to_uppercase()),
    );
    let heading = if compact {
        heading
    } else {
        heading.child(
            div()
                .text_sm()
                .text_color(rgb(MUTED_TEXT))
                .child(description),
        )
    };

    div()
        .flex()
        .flex_col()
        .gap(px(if compact { 8.0 } else { 12.0 }))
        .child(heading)
        .child(div().children(rows))
}

fn setting_row(
    label: &'static str,
    description: &'static str,
    value: SharedString,
    compact: bool,
) -> AnyElement {
    let row = div()
        .flex()
        .items_center()
        .py(px(if compact { 8.0 } else { 13.0 }))
        .border_t_1()
        .border_color(rgb(BORDER));
    let row = if compact {
        row.flex_col().items_start().gap_2()
    } else {
        row.justify_between().gap_6()
    };

    let label = div().min_w_0().flex_1().flex().flex_col().gap_1().child(
        div()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(TEXT))
            .child(label),
    );
    let label = if compact {
        label
    } else {
        label.child(
            div()
                .text_xs()
                .text_color(rgb(FAINT_TEXT))
                .child(description),
        )
    };

    let value = div()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(VALUE_TEXT))
        .child(value);
    let value = if compact {
        value.w_full().text_xs().whitespace_normal()
    } else {
        value.max_w(px(300.0)).truncate()
    };

    row.child(label).child(value).into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use gpui::point;
    use gpui::size;

    use super::*;

    fn settings() -> MainWindowSettings {
        MainWindowSettings {
            model: "final-model".into(),
            partials_model: "live-model".into(),
            mode: "Technical".into(),
            delivery: "Insert at cursor".into(),
            microphone: "System default".into(),
            shortcut: "Not configured".into(),
        }
    }

    #[gpui::test]
    fn settings_is_default_and_history_can_be_selected(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let view = cx.new(|cx| MainWindow::new(settings(), cx));

        cx.draw(
            point(px(0.0), px(0.0)),
            size(px(584.0), px(928.0)),
            |_, _| view.clone().into_any_element(),
        );
        assert_eq!(view.update(cx, |view, _| view.page), Page::Settings);
        assert!(cx.debug_bounds("main-content").is_some());
        assert!(cx.debug_bounds("nav-settings").is_some());
        assert!(cx.debug_bounds("brand-mark").is_some_and(|bounds| {
            bounds.size.width == px(28.0) && bounds.size.height == px(28.0)
        }));
        assert!(
            cx.debug_bounds("settings-page")
                .is_some_and(|bounds| bounds.size.width >= px(300.0))
        );

        view.update(cx, |view, cx| view.select_page(Page::History, cx));
        cx.draw(
            point(px(0.0), px(0.0)),
            size(px(584.0), px(928.0)),
            |_, _| view.clone().into_any_element(),
        );
        assert_eq!(view.update(cx, |view, _| view.page), Page::History);
        assert!(cx.debug_bounds("nav-history").is_some());
        assert!(
            cx.debug_bounds("history-empty-mark")
                .is_some_and(|bounds| bounds.size.width == bounds.size.height)
        );
        assert!(
            cx.debug_bounds("history-page")
                .is_some_and(|bounds| bounds.size.width >= px(300.0))
        );
    }

    #[gpui::test]
    fn keyboard_can_navigate_and_activate_pages(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| MainWindow::new(settings(), cx));
        cx.focus(&view);

        assert_eq!(view.update(cx, |view, _| view.page), Page::Settings);

        cx.simulate_keystrokes("tab enter");
        assert_eq!(view.update(cx, |view, _| view.page), Page::History);

        cx.simulate_keystrokes("tab space");
        assert_eq!(view.update(cx, |view, _| view.page), Page::Settings);
    }
}
