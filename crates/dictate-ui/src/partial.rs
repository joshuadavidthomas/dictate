use std::time::Duration;
use std::time::Instant;

use gpui::Context;
use gpui::IntoElement;
use gpui::Render;
use gpui::ScrollHandle;
use gpui::Window;

use crate::components;
use crate::overlay::MORPH_DURATION;
use crate::overlay::ease_out_quart;
use crate::overlay::morph_progress;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Clone, Debug, PartialEq)]
pub struct PartialTextStyle {
    font_family: Option<String>,
    font_size: f32,
}

impl PartialTextStyle {
    #[must_use]
    pub fn new(font_family: Option<String>, font_size: f32) -> Self {
        Self {
            font_family,
            font_size,
        }
    }

    pub(crate) fn font_family(&self) -> Option<&str> {
        self.font_family.as_deref()
    }

    pub(crate) fn font_size(&self) -> f32 {
        self.font_size
    }
}

impl Default for PartialTextStyle {
    fn default() -> Self {
        Self::new(None, 14.0)
    }
}

pub struct PartialView {
    text: String,
    style: PartialTextStyle,
    scroll_handle: ScrollHandle,
    fade_started_at: Instant,
}

impl PartialView {
    pub fn new(text: &str, style: PartialTextStyle, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FRAME_INTERVAL).await;

                let Ok(fading) = this.update(cx, |view, cx| {
                    let fading = view.fade_started_at.elapsed() < MORPH_DURATION;
                    cx.notify();
                    fading
                }) else {
                    break;
                };

                if !fading {
                    break;
                }
            }
        })
        .detach();

        let scroll_handle = ScrollHandle::new();
        scroll_handle.scroll_to_bottom();
        Self {
            text: text.to_owned(),
            style,
            scroll_handle,
            fade_started_at: Instant::now(),
        }
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.text == text {
            return;
        }

        text.clone_into(&mut self.text);
        self.scroll_handle.scroll_to_bottom();
        cx.notify();
    }
}

impl Render for PartialView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let opacity = ease_out_quart(morph_progress(self.fade_started_at.elapsed()));
        components::PartialCard::new(
            self.text.clone(),
            self.style.clone(),
            self.scroll_handle.clone(),
            opacity,
        )
    }
}
