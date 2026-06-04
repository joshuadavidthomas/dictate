use crate::prelude::*;
use crate::spectrum::SPECTRUM_BANDS;

const BAR_WIDTH: f32 = 3.0;
const BAR_GAP: f32 = 2.0;
const TOTAL_HEIGHT: f32 = 20.0;
const MAX_BAR_HEIGHT: f32 = 18.0;
const MIN_BAR_HEIGHT: f32 = 3.0;
const AMPLIFICATION: f32 = 2.0;
const NORMALIZATION_CURVE: f32 = 0.6;

#[derive(IntoElement)]
pub struct Waveform {
    bands: [f32; SPECTRUM_BANDS],
}

impl Waveform {
    pub fn new(bands: [f32; SPECTRUM_BANDS]) -> Self {
        Self { bands }
    }
}

impl RenderOnce for Waveform {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        h_flex()
            .h(px(TOTAL_HEIGHT))
            .gap(px(BAR_GAP))
            .items_center()
            .children(self.bands.into_iter().map(|level| {
                div()
                    .w(px(BAR_WIDTH))
                    .h(px(bar_height(level)))
                    .rounded_full()
                    .bg(hsla(0.0, 0.0, 0.90, 0.75))
            }))
    }
}

fn bar_height(level: f32) -> f32 {
    let amplified = (level * AMPLIFICATION)
        .clamp(0.0, 1.0)
        .powf(NORMALIZATION_CURVE);
    MIN_BAR_HEIGHT + (amplified * (MAX_BAR_HEIGHT - MIN_BAR_HEIGHT))
}
