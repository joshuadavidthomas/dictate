use std::time::Duration;
use std::time::Instant;

use gpui::Context;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Window;

use crate::components;
use crate::spectrum::DEFAULT_WAVEFORM_SMOOTHING;
use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::SpectrumLevels;
use crate::spectrum::advance_waveform_bands;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub struct OverlayView {
    spectrum: SpectrumLevels,
    displayed_bands: [f32; SPECTRUM_BANDS],
    last_frame: Instant,
    visual_active: bool,
}

impl OverlayView {
    pub fn new(spectrum: SpectrumLevels, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            // Do not replace this with GPUI's frame callbacks: at rev 50d001f,
            // gpui/src/window.rs:1436-1449 caps inactive windows at ~30fps, and
            // this non-focusable layer-shell overlay is never active.
            loop {
                if this
                    .update(cx, |overlay, cx| {
                        overlay.advance_waveform();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }

                cx.background_executor().timer(FRAME_INTERVAL).await;
            }
        })
        .detach();

        Self {
            displayed_bands: spectrum.bands(),
            spectrum,
            last_frame: Instant::now(),
            visual_active: false,
        }
    }

    fn advance_waveform(&mut self) {
        let now = Instant::now();
        let frame_time = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let advance = advance_waveform_bands(
            self.displayed_bands,
            self.visual_active,
            self.spectrum.bands(),
            frame_time,
            DEFAULT_WAVEFORM_SMOOTHING,
        );
        self.displayed_bands = advance.smoothed_bands;
        self.visual_active = advance.gate_state.is_open();
    }
}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        components::Panel::new("dictate-overlay")
            .child(components::Waveform::new(self.displayed_bands))
    }
}
