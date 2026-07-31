use dictate_signal::SPECTRUM_BANDS;
use gpui::App;
use gpui::IntoElement;
use gpui::RenderOnce;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;

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
    color: gpui::Hsla,
}

impl Waveform {
    #[must_use]
    pub fn new(bands: [f32; SPECTRUM_BANDS], color: gpui::Hsla) -> Self {
        Self { bands, color }
    }
}

impl RenderOnce for Waveform {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .h(px(TOTAL_HEIGHT))
            .gap(px(BAR_GAP))
            .items_center()
            .children(self.bands.into_iter().map(|level| {
                div()
                    .w(px(BAR_WIDTH))
                    .h(px(bar_height(level)))
                    .rounded_full()
                    .bg(self.color)
            }))
    }
}

fn bar_height(level: f32) -> f32 {
    let amplified = (level * AMPLIFICATION)
        .clamp(0.0, 1.0)
        .powf(NORMALIZATION_CURVE);
    MIN_BAR_HEIGHT + (amplified * (MAX_BAR_HEIGHT - MIN_BAR_HEIGHT))
}
