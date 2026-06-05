use std::time::Duration;

use gpui::Context;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Window;

use crate::components;
use crate::state::DictationState;
use crate::transcription::ConsoleTranscriber;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub struct Overlay {
    state: DictationState,
    _transcription: ConsoleTranscriber,
}

impl Overlay {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                let _ = this.update(cx, |_, cx| cx.notify());
                cx.background_executor().timer(FRAME_INTERVAL).await;
            }
        })
        .detach();

        let state = DictationState::initial();
        let transcription = ConsoleTranscriber::start(state.spectrum());

        Self {
            state,
            _transcription: transcription,
        }
    }
}

impl Render for Overlay {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        components::Panel::new("dictate-overlay")
            .child(components::Waveform::new(self.state.spectrum_bands()))
    }
}
