use std::array;
use std::time::Duration;
use std::time::Instant;

use futures::FutureExt;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::pin_mut;
use futures::select_biased;
use gpui::AnyElement;
use gpui::Context;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Window;
use gpui::div;
use gpui::hsla;
use gpui::prelude::*;
use gpui::px;

use crate::components;
use crate::spectrum::DEFAULT_WAVEFORM_SMOOTHING;
use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::SpectrumLevels;
use crate::spectrum::WaveformGateState;
use crate::spectrum::advance_waveform_bands;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const MORPH_DURATION: Duration = Duration::from_millis(180);
const SUBMISSION_MOTION_DURATION: Duration = Duration::from_millis(700);
const SURFACE_WIDTH: f32 = 62.0;
const SIGNAL_WIDTH: f32 = 38.0;
const SIGNAL_HEIGHT: f32 = 20.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OverlayState {
    Recording,
    Transcribing,
    PendingTranscript,
    InsertSubmitted,
    InsertionUncertain,
    DeliveryFailed,
    NoTranscript,
    NothingToPaste,
}

impl OverlayState {
    const fn accessible_label(self) -> &'static str {
        match self {
            Self::Recording => "Dictation recording",
            Self::Transcribing => "Dictation transcribing",
            Self::PendingTranscript => "Transcript ready to paste",
            Self::InsertSubmitted => "Insertion submitted",
            Self::InsertionUncertain => "Check whether dictation was inserted",
            Self::DeliveryFailed => "Dictation delivery failed",
            Self::NoTranscript => "No transcript produced",
            Self::NothingToPaste => "No retained transcript",
        }
    }
}

pub struct OverlayView {
    state: OverlayState,
    state_started_at: Instant,
    morph_started_at: Instant,
    animation_waker: mpsc::UnboundedSender<()>,
    spectrum: SpectrumLevels,
    displayed_bands: [f32; SPECTRUM_BANDS],
    last_frame: Instant,
    gate_state: WaveformGateState,
}

impl OverlayView {
    pub(crate) fn new(
        spectrum: SpectrumLevels,
        state: OverlayState,
        cx: &mut Context<Self>,
    ) -> Self {
        let (animation_waker, mut animation_wake) = mpsc::unbounded();

        cx.spawn(async move |this, cx| {
            // Do not replace this with GPUI's frame callbacks: at rev 50d001f,
            // gpui/src/window.rs:1436-1449 caps inactive windows at ~30fps, and
            // this non-focusable layer-shell overlay is never active.
            loop {
                let Ok(animate) = this.update(cx, |overlay, cx| {
                    let animate = overlay.advance_animation();
                    if animate {
                        cx.notify();
                    }
                    animate
                }) else {
                    break;
                };

                if !animate {
                    if animation_wake.next().await.is_none() {
                        break;
                    }
                    continue;
                }

                let timer = cx.background_executor().timer(FRAME_INTERVAL).fuse();
                let wake = animation_wake.next().fuse();
                pin_mut!(timer, wake);
                select_biased! {
                    signal = wake => {
                        if signal.is_none() {
                            break;
                        }
                    }
                    () = timer => {}
                }
            }
        })
        .detach();

        let now = Instant::now();
        Self {
            state,
            state_started_at: now,
            morph_started_at: now,
            animation_waker,
            displayed_bands: spectrum.bands(),
            spectrum,
            last_frame: now,
            gate_state: WaveformGateState::Closed,
        }
    }

    pub(crate) fn set_state(&mut self, state: OverlayState, cx: &mut Context<Self>) {
        if self.state == state {
            return;
        }

        let now = Instant::now();
        self.morph_started_at = now;
        self.state_started_at = now;
        self.state = state;

        if state == OverlayState::Recording {
            self.displayed_bands = self.spectrum.bands();
            self.last_frame = now;
        }

        drop(self.animation_waker.unbounded_send(()));
        cx.notify();
    }

    pub(crate) fn displayed_bands(&self) -> [f32; SPECTRUM_BANDS] {
        self.displayed_bands
    }

    pub(crate) fn gate_state(&self) -> WaveformGateState {
        self.gate_state
    }

    fn advance_animation(&mut self) -> bool {
        let now = Instant::now();
        if self.state == OverlayState::Recording {
            self.advance_waveform(now);
        }

        self.state == OverlayState::Recording
            || self.state == OverlayState::Transcribing
            || (self.state == OverlayState::InsertSubmitted
                && now.duration_since(self.state_started_at) < SUBMISSION_MOTION_DURATION)
            || now.duration_since(self.morph_started_at) < MORPH_DURATION
    }

    fn advance_waveform(&mut self, now: Instant) {
        let frame_time = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let advance = advance_waveform_bands(
            self.displayed_bands,
            self.gate_state.is_open(),
            self.spectrum.bands(),
            frame_time,
            DEFAULT_WAVEFORM_SMOOTHING,
        );
        self.displayed_bands = advance.smoothed_bands;
        self.gate_state = advance.gate_state;
    }

    fn signal(&self, now: Instant) -> AnyElement {
        let entered = ease_out_quart(morph_progress(now.duration_since(self.morph_started_at)));
        let elapsed = now.duration_since(self.state_started_at);

        match self.state {
            OverlayState::Recording => signal_frame(
                components::Waveform::new(self.displayed_bands, hsla(0.0, 0.0, 0.90, 0.78))
                    .into_any_element(),
                entered,
            ),
            OverlayState::Transcribing => signal_frame(
                components::Waveform::new(ocean_bands(elapsed), hsla(0.58, 0.58, 0.76, 0.88))
                    .into_any_element(),
                entered,
            ),
            OverlayState::InsertSubmitted => {
                let exit = 1.0
                    - normalized_progress(
                        elapsed,
                        Duration::from_millis(460),
                        Duration::from_millis(220),
                    );
                signal_frame(
                    components::Waveform::new(
                        outbound_bands(elapsed),
                        hsla(0.58, 0.72, 0.72, 0.92),
                    )
                    .into_any_element(),
                    entered * exit,
                )
            }
            OverlayState::PendingTranscript => signal_frame(
                components::Waveform::new(settled_bands(), hsla(0.0, 0.0, 0.82, 0.74))
                    .into_any_element(),
                entered,
            ),
            OverlayState::InsertionUncertain => signal_frame(
                components::Waveform::new(settled_bands(), hsla(0.12, 0.62, 0.68, 0.86))
                    .into_any_element(),
                entered,
            ),
            OverlayState::DeliveryFailed => {
                signal_frame(broken_signal(hsla(0.0, 0.72, 0.68, 0.96)), entered)
            }
            OverlayState::NoTranscript | OverlayState::NothingToPaste => {
                signal_frame(flat_signal(hsla(0.0, 0.0, 0.72, 0.84)), entered)
            }
        }
    }
}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        components::Panel::new(
            "dictate-overlay",
            SURFACE_WIDTH,
            self.state.accessible_label(),
        )
        .child(self.signal(now))
    }
}

fn signal_frame(signal: AnyElement, opacity: f32) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(SIGNAL_WIDTH))
        .h(px(SIGNAL_HEIGHT))
        .opacity(opacity)
        .child(signal)
        .into_any_element()
}

fn broken_signal(color: gpui::Hsla) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .w(px(20.0))
        .h(px(18.0))
        .child(div().w(px(3.0)).h(px(12.0)).rounded_full().bg(color))
        .child(div().w(px(3.0)).h(px(6.0)).rounded_full().bg(color))
        .child(div().w(px(4.0)).h(px(2.0)))
        .child(div().w(px(3.0)).h(px(6.0)).rounded_full().bg(color))
        .child(div().w(px(3.0)).h(px(12.0)).rounded_full().bg(color))
        .into_any_element()
}

fn flat_signal(color: gpui::Hsla) -> AnyElement {
    div()
        .w(px(18.0))
        .h(px(2.0))
        .rounded_full()
        .bg(color)
        .into_any_element()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "callers pass only the fixed band count and its indices"
)]
fn band_number(value: usize) -> f32 {
    value as f32
}

fn ocean_bands(elapsed: Duration) -> [f32; SPECTRUM_BANDS] {
    let phase = elapsed.as_secs_f32() * std::f32::consts::TAU / 2.4;
    let band_count = band_number(SPECTRUM_BANDS);

    array::from_fn(|index| {
        let position = band_number(index) / band_count;
        let wave = (phase - (position * std::f32::consts::TAU)).sin();

        // Waveform doubles live audio before drawing it. Keep this synthetic
        // signal within 0.0..=0.5 so its trough and crest use the full height.
        ((wave + 1.0) * 0.25).clamp(0.0, 0.5)
    })
}

const fn settled_bands() -> [f32; SPECTRUM_BANDS] {
    [0.08; SPECTRUM_BANDS]
}

fn outbound_bands(elapsed: Duration) -> [f32; SPECTRUM_BANDS] {
    let phase = (elapsed.as_secs_f32() / SUBMISSION_MOTION_DURATION.as_secs_f32())
        * (band_number(SPECTRUM_BANDS) + 2.0)
        - 1.0;

    array::from_fn(|index| (1.0 - ((band_number(index) - phase).abs() / 1.8)).clamp(0.0, 1.0))
}

fn normalized_progress(elapsed: Duration, delay: Duration, duration: Duration) -> f32 {
    (elapsed.saturating_sub(delay).as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

fn morph_progress(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() / MORPH_DURATION.as_secs_f32()).clamp(0.0, 1.0)
}

fn ease_out_quart(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processing_signal_is_a_slow_bounded_wave() {
        let start = ocean_bands(Duration::ZERO);
        let quarter_cycle = ocean_bands(Duration::from_millis(600));
        let full_cycle = ocean_bands(Duration::from_millis(2_400));

        assert!(
            start
                .iter()
                .zip(quarter_cycle)
                .any(|(start_level, quarter_level)| (start_level - quarter_level).abs() > 0.001)
        );
        assert!(start.iter().all(|level| (0.0..=0.5).contains(level)));
        assert!(
            quarter_cycle
                .iter()
                .all(|level| (0.0..=0.5).contains(level))
        );
        let range = quarter_cycle.iter().copied().fold(0.0_f32, f32::max)
            - quarter_cycle.iter().copied().fold(0.5_f32, f32::min);
        assert!(range > 0.45);
        for (start_level, full_cycle_level) in start.into_iter().zip(full_cycle) {
            assert!((start_level - full_cycle_level).abs() < 0.001);
        }
    }

    #[test]
    fn outbound_signal_leaves_the_surface() {
        let finished = outbound_bands(SUBMISSION_MOTION_DURATION);

        assert!(finished.iter().all(|level| *level == 0.0));
    }
}
