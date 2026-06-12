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
const RISE_SPEED: f32 = 16.0;
const FALL_SPEED: f32 = 10.0;
const VISUAL_GATE_ON: f32 = 0.16;
const VISUAL_GATE_OFF: f32 = 0.08;

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
        let frame_time = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .min(MAX_FRAME_TIME);
        self.last_frame = now;

        let target_bands = self.spectrum.bands();
        let peak = target_bands.iter().copied().fold(0.0, f32::max);
        if self.visual_active {
            self.visual_active = peak >= VISUAL_GATE_OFF;
        } else {
            self.visual_active = peak >= VISUAL_GATE_ON;
        }
        let target_bands = if self.visual_active {
            target_bands
        } else {
            [0.0; SPECTRUM_BANDS]
        };

        for (displayed, target) in self.displayed_bands.iter_mut().zip(target_bands) {
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
