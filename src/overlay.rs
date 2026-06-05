use std::time::Duration;
use std::time::Instant;

use gpui::Context;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Window;

use crate::components;
use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::SpectrumLevels;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const MAX_FRAME_TIME: f32 = 0.05;
const RISE_SPEED: f32 = 90.0;
const FALL_SPEED: f32 = 50.0;

pub struct OverlayView {
    spectrum: SpectrumLevels,
    displayed_bands: [f32; SPECTRUM_BANDS],
    last_frame: Instant,
}

impl OverlayView {
    pub fn new(spectrum: SpectrumLevels, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
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
        }
    }

    fn advance_waveform(&mut self) {
        let now = Instant::now();
        let frame_time = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .min(MAX_FRAME_TIME);
        self.last_frame = now;

        for (displayed, target) in self.displayed_bands.iter_mut().zip(self.spectrum.bands()) {
            let speed = if target > *displayed {
                RISE_SPEED
            } else {
                FALL_SPEED
            };
            let blend = 1.0 - (-speed * frame_time).exp();
            *displayed += (target - *displayed) * blend;
        }
    }
}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        components::Panel::new("dictate-overlay")
            .child(components::Waveform::new(self.displayed_bands))
    }
}
