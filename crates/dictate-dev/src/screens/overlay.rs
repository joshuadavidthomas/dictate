use std::cell::RefCell;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

use dictate_signal::SPECTRUM_BANDS;
use dictate_signal::SpectrumLevels;
use dictate_speech::CaptureHandler;
use dictate_speech::DICTATION_SAMPLE_RATE;
use dictate_speech::Mic;
use dictate_speech::MicrophoneStreamError;
use dictate_speech::SpectrumUpdate;
use dictate_speech::capture;
use dictate_ui::OVERLAY_WINDOW_HEIGHT;
use dictate_ui::OVERLAY_WINDOW_WIDTH;
use dictate_ui::OverlayState;
use dictate_ui::OverlayView;
use gpui::AnyElement;
use gpui::App;
use gpui::AppContext;
use gpui::Entity;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::rgb;

use crate::feeders::RECORDED_SPECTRUM_FRAMES;
use crate::feeders::SpectrumSource;
use crate::registry::DebugComponent;
use crate::registry::PreviewClock;
use crate::registry::ScenarioChip;
use crate::registry::ScenarioRow;
use crate::stats::FrameRecord;

static SCENARIO_IDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    OverlayScenario::ALL
        .iter()
        .map(|scenario| scenario.id())
        .collect()
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayScenario {
    OpeningMicrophone,
    RecordingSine,
    RecordingConstant,
    RecordingFrames,
    RecordingLive,
    Transcribing,
    PendingTranscript,
    InsertionUncertain,
    DeliveryFailed,
}

impl OverlayScenario {
    const ALL: [Self; 9] = [
        Self::RecordingSine,
        Self::RecordingConstant,
        Self::RecordingFrames,
        Self::RecordingLive,
        Self::OpeningMicrophone,
        Self::Transcribing,
        Self::PendingTranscript,
        Self::InsertionUncertain,
        Self::DeliveryFailed,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::OpeningMicrophone => "opening-microphone",
            Self::RecordingSine => "recording-sine",
            Self::RecordingConstant => "recording-constant",
            Self::RecordingFrames => "recording-frames",
            Self::RecordingLive => "recording-live",
            Self::Transcribing => "transcribing",
            Self::PendingTranscript => "pending-transcript",
            Self::InsertionUncertain => "insertion-uncertain",
            Self::DeliveryFailed => "delivery-failed",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scenario| scenario.id() == id)
    }

    fn selected(id: &str) -> Self {
        Self::from_id(id).unwrap_or(Self::RecordingSine)
    }

    const fn overlay_state(self) -> OverlayState {
        match self {
            Self::OpeningMicrophone => OverlayState::OpeningMicrophone,
            Self::RecordingSine
            | Self::RecordingConstant
            | Self::RecordingFrames
            | Self::RecordingLive => OverlayState::Recording,
            Self::Transcribing => OverlayState::Transcribing,
            Self::PendingTranscript => OverlayState::PendingTranscript,
            Self::InsertionUncertain => OverlayState::InsertionUncertain,
            Self::DeliveryFailed => OverlayState::DeliveryFailed,
        }
    }

    const fn spectrum(self) -> SpectrumPlan {
        match self {
            Self::OpeningMicrophone
            | Self::Transcribing
            | Self::PendingTranscript
            | Self::InsertionUncertain
            | Self::DeliveryFailed => SpectrumPlan::Deterministic(SpectrumSource::Silent),
            Self::RecordingSine => SpectrumPlan::Deterministic(SpectrumSource::SineSweep),
            Self::RecordingConstant => SpectrumPlan::Deterministic(SpectrumSource::Constant(0.55)),
            Self::RecordingFrames => {
                SpectrumPlan::Deterministic(SpectrumSource::Frames(&RECORDED_SPECTRUM_FRAMES))
            }
            Self::RecordingLive => SpectrumPlan::LiveMic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SpectrumPlan {
    Deterministic(SpectrumSource),
    LiveMic,
}

struct SpectrumCaptureHandler {
    levels: SpectrumLevels,
    stream_error_slot: Arc<Mutex<Option<String>>>,
}

impl CaptureHandler for SpectrumCaptureHandler {
    fn samples(&self, _samples: &[f32]) -> SpectrumUpdate {
        SpectrumUpdate::Emit
    }

    fn spectrum(&self, bands: [f32; SPECTRUM_BANDS]) {
        self.levels.set(bands);
    }

    fn stream_error(&self, error: &MicrophoneStreamError) {
        eprintln!("spectrum recording error: {error}");
        *lock_or_recover(&self.stream_error_slot) = Some(format!("{error:#}"));
    }
}

pub(crate) struct OverlayPreviewState {
    levels: SpectrumLevels,
    overlay: Entity<OverlayView>,
    live_mic: Option<Mic>,
    live_error: Option<String>,
    stream_error_slot: Arc<Mutex<Option<String>>>,
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Drain a pending stream error reported by a `SpectrumCaptureHandler` and apply
/// it to the owning preview state.
///
/// When the slot holds an error, the stale `Mic` is dropped and `live_error` is
/// set to a `microphone unavailable: …` message matching the pre-open failure
/// path of `ensure_live_mic`. The slot is cleared so a subsequent
/// `ensure_live_mic` call sees a clean slate once the error is acknowledged
/// (e.g. via a manual `Reset`).
fn drain_stream_error_slot(
    live_mic: &mut Option<Mic>,
    live_error: &mut Option<String>,
    stream_error_slot: &Mutex<Option<String>>,
) {
    if let Some(error) = lock_or_recover(stream_error_slot).take() {
        drop(live_mic.take());
        *live_error = Some(format!("microphone unavailable: {error}"));
    }
}

impl OverlayPreviewState {
    pub(crate) fn new(scenario_id: &str, clock: PreviewClock, cx: &mut impl AppContext) -> Self {
        let levels = SpectrumLevels::new();
        let scenario = OverlayScenario::selected(scenario_id);

        levels.set(target_bands_for_scenario(scenario, clock));

        let overlay = cx.new(|cx| OverlayView::new(levels.clone(), scenario.overlay_state(), cx));

        Self {
            levels,
            overlay,
            live_mic: None,
            live_error: None,
            stream_error_slot: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn reset(
        &mut self,
        scenario_id: &str,
        clock: PreviewClock,
        cx: &mut impl AppContext,
    ) {
        *self = Self::new(scenario_id, clock, cx);
    }

    pub(crate) fn advance(
        &mut self,
        scenario_id: &str,
        clock: PreviewClock,
        frame_delta: std::time::Duration,
        cx: &mut impl AppContext,
    ) -> FrameRecord {
        let scenario = OverlayScenario::selected(scenario_id);
        self.overlay.update(cx, |overlay, cx| {
            overlay.set_state(scenario.overlay_state(), cx);
        });

        match scenario.spectrum() {
            SpectrumPlan::Deterministic(_) => {
                drop(self.live_mic.take());
                self.live_error = None;
                *lock_or_recover(&self.stream_error_slot) = None;
                self.levels.set(target_bands_for_scenario(scenario, clock));
            }
            SpectrumPlan::LiveMic => {
                drain_stream_error_slot(
                    &mut self.live_mic,
                    &mut self.live_error,
                    &self.stream_error_slot,
                );
                self.ensure_live_mic();
            }
        }

        let target_bands = self.levels.bands();
        let (smoothed_bands, gate_state) = self.overlay.read_with(cx, |overlay, _| {
            (overlay.displayed_bands(), overlay.gate_state())
        });

        FrameRecord::new(
            scenario_id,
            clock.frame_index,
            frame_delta,
            target_bands,
            smoothed_bands,
            gate_state,
        )
    }

    fn overlay(&self) -> Entity<OverlayView> {
        self.overlay.clone()
    }

    fn live_error(&self) -> Option<&str> {
        self.live_error.as_deref()
    }

    fn ensure_live_mic(&mut self) {
        if self.live_mic.is_none() && self.live_error.is_none() {
            match capture(
                DICTATION_SAMPLE_RATE.as_hz(),
                None,
                SpectrumCaptureHandler {
                    levels: self.levels.clone(),
                    stream_error_slot: Arc::clone(&self.stream_error_slot),
                },
            ) {
                Ok(mic) => self.live_mic = Some(mic),
                Err(error) => {
                    self.levels.set([0.0; SPECTRUM_BANDS]);
                    self.live_error = Some(format!("microphone unavailable: {error:#}"));
                }
            }
        }
    }
}

fn target_bands_for_scenario(
    scenario: OverlayScenario,
    clock: PreviewClock,
) -> [f32; SPECTRUM_BANDS] {
    match scenario.spectrum() {
        SpectrumPlan::Deterministic(source) => source.frame_at(clock.elapsed, clock.frame_index),
        SpectrumPlan::LiveMic => [0.0; SPECTRUM_BANDS],
    }
}

pub(crate) struct OverlayPreview {
    state: RefCell<Option<OverlayPreviewState>>,
}

impl OverlayPreview {
    pub(crate) fn new() -> Self {
        Self {
            state: RefCell::new(None),
        }
    }

    fn ensure_state(&self, scenario: &str, clock: PreviewClock, cx: &mut App) {
        let mut state = self.state.borrow_mut();
        if state.is_none() {
            *state = Some(OverlayPreviewState::new(scenario, clock, cx));
        }
    }
}

impl DebugComponent for OverlayPreview {
    fn name(&self) -> &'static str {
        "overlay"
    }

    fn description(&self) -> &'static str {
        "Preview the dictation overlay against deterministic phase and spectrum scenarios."
    }

    fn scenarios(&self) -> &'static [&'static str] {
        SCENARIO_IDS.as_slice()
    }

    fn scenario_rows(&self) -> Vec<ScenarioRow> {
        vec![
            ScenarioRow {
                label: "phase",
                chips: vec![
                    ScenarioChip {
                        label: "recording",
                        activates: "recording-sine",
                        matches: vec![
                            "recording-sine",
                            "recording-constant",
                            "recording-frames",
                            "recording-live",
                        ],
                    },
                    ScenarioChip {
                        label: "opening microphone",
                        activates: "opening-microphone",
                        matches: vec!["opening-microphone"],
                    },
                    ScenarioChip {
                        label: "transcribing",
                        activates: "transcribing",
                        matches: vec!["transcribing"],
                    },
                    ScenarioChip {
                        label: "pending transcript",
                        activates: "pending-transcript",
                        matches: vec!["pending-transcript"],
                    },
                    ScenarioChip {
                        label: "insertion uncertain",
                        activates: "insertion-uncertain",
                        matches: vec!["insertion-uncertain"],
                    },
                    ScenarioChip {
                        label: "delivery failed",
                        activates: "delivery-failed",
                        matches: vec!["delivery-failed"],
                    },
                ],
            },
            ScenarioRow {
                label: "source",
                chips: vec![
                    ScenarioChip {
                        label: "sine",
                        activates: "recording-sine",
                        matches: vec!["recording-sine"],
                    },
                    ScenarioChip {
                        label: "constant",
                        activates: "recording-constant",
                        matches: vec!["recording-constant"],
                    },
                    ScenarioChip {
                        label: "frames",
                        activates: "recording-frames",
                        matches: vec!["recording-frames"],
                    },
                    ScenarioChip {
                        label: "live mic",
                        activates: "recording-live",
                        matches: vec!["recording-live"],
                    },
                ],
            },
        ]
    }

    fn produces_stats(&self) -> bool {
        true
    }

    fn reset(&self, scenario: &str, cx: &mut App) {
        let clock = PreviewClock {
            elapsed: Duration::ZERO,
            frame_index: 0,
        };
        let mut state = self.state.borrow_mut();
        match state.as_mut() {
            Some(state) => state.reset(scenario, clock, cx),
            None => *state = Some(OverlayPreviewState::new(scenario, clock, cx)),
        }
    }

    fn deactivate(&self) {
        self.state.borrow_mut().take();
    }

    fn advance(
        &self,
        scenario: &str,
        clock: PreviewClock,
        frame_delta: Duration,
        cx: &mut App,
    ) -> Option<FrameRecord> {
        self.ensure_state(scenario, clock, cx);

        self.state
            .borrow_mut()
            .as_mut()
            .map(|state| state.advance(scenario, clock, frame_delta, cx))
    }

    fn preview(&self, scenario: &str, _window: &mut Window, cx: &mut App) -> AnyElement {
        self.ensure_state(
            scenario,
            PreviewClock {
                elapsed: Duration::ZERO,
                frame_index: 0,
            },
            cx,
        );

        let (overlay, live_error) = {
            let state = self.state.borrow();
            let Some(state) = state.as_ref() else {
                return div()
                    .id("debug-overlay-preview-missing-state")
                    .size_full()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x007f_1d1d))
                    .bg(rgb(0x000b_1020))
                    .p(px(16.0))
                    .text_color(rgb(0x00fc_a5a5))
                    .child("overlay preview state is unavailable")
                    .into_any_element();
            };

            (state.overlay(), state.live_error().map(str::to_string))
        };

        div()
            .id("debug-overlay-preview")
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x001f_2937))
            .bg(rgb(0x000b_1020))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .w(px(OVERLAY_WINDOW_WIDTH))
                    .h(px(OVERLAY_WINDOW_HEIGHT))
                    .child(overlay),
            )
            .when_some(live_error, |this, error| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x009c_a3af))
                        .child(format!("live mic: {error}")),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `MicrophoneStreamError` that mirrors the one cpal raises when the
    /// `PulseAudio` server disconnects mid-session.
    fn pulseaudio_disconnect_error() -> MicrophoneStreamError {
        MicrophoneStreamError::new(cpal::Error::with_message(
            cpal::ErrorKind::StreamInvalidated,
            "PulseAudio disconnected",
        ))
    }

    #[test]
    fn scenario_ids_round_trip_exhaustively() {
        assert_eq!(OverlayScenario::ALL.len(), SCENARIO_IDS.len());

        for scenario in OverlayScenario::ALL {
            let id = scenario.id();

            assert!(SCENARIO_IDS.contains(&id));
            assert_eq!(OverlayScenario::from_id(id), Some(scenario));
        }

        for id in SCENARIO_IDS.iter().copied() {
            let scenario = OverlayScenario::from_id(id).expect("listed id must parse");

            assert_eq!(scenario.id(), id);
        }
    }

    #[test]
    fn each_scenario_resolves_spectrum_plan() {
        let expected = [
            (
                OverlayScenario::OpeningMicrophone,
                SpectrumPlan::Deterministic(SpectrumSource::Silent),
            ),
            (
                OverlayScenario::RecordingSine,
                SpectrumPlan::Deterministic(SpectrumSource::SineSweep),
            ),
            (
                OverlayScenario::RecordingConstant,
                SpectrumPlan::Deterministic(SpectrumSource::Constant(0.55)),
            ),
            (
                OverlayScenario::RecordingFrames,
                SpectrumPlan::Deterministic(SpectrumSource::Frames(&RECORDED_SPECTRUM_FRAMES)),
            ),
            (OverlayScenario::RecordingLive, SpectrumPlan::LiveMic),
            (
                OverlayScenario::Transcribing,
                SpectrumPlan::Deterministic(SpectrumSource::Silent),
            ),
            (
                OverlayScenario::PendingTranscript,
                SpectrumPlan::Deterministic(SpectrumSource::Silent),
            ),
            (
                OverlayScenario::InsertionUncertain,
                SpectrumPlan::Deterministic(SpectrumSource::Silent),
            ),
            (
                OverlayScenario::DeliveryFailed,
                SpectrumPlan::Deterministic(SpectrumSource::Silent),
            ),
        ];

        for (scenario, source) in expected {
            assert_eq!(scenario.spectrum(), source);
        }
    }

    #[test]
    fn stream_error_propagates_to_shared_slot() {
        let levels = SpectrumLevels::new();
        let slot = Arc::new(Mutex::new(None));
        let handler = SpectrumCaptureHandler {
            levels: levels.clone(),
            stream_error_slot: Arc::clone(&slot),
        };

        handler.stream_error(&pulseaudio_disconnect_error());

        assert_eq!(
            lock_or_recover(&slot).as_deref(),
            Some("PulseAudio disconnected")
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn stream_error_preserves_spectrum_levels() {
        let levels = SpectrumLevels::new();
        let known_bands = [0.42; SPECTRUM_BANDS];
        levels.set(known_bands);
        let handler = SpectrumCaptureHandler {
            levels: levels.clone(),
            stream_error_slot: Arc::new(Mutex::new(None)),
        };

        handler.stream_error(&pulseaudio_disconnect_error());

        // `SpectrumLevels` stores and returns bit patterns via `to_bits`/`from_bits`,
        // so an exact comparison is the correct way to confirm the levels round-tripped.
        assert_eq!(levels.bands(), known_bands);
    }

    #[test]
    fn drain_stream_error_slot_surfaces_error_and_clears_live_mic() {
        let mut live_mic: Option<Mic> = None;
        let mut live_error: Option<String> = None;
        let slot = Mutex::new(Some("PulseAudio disconnected".to_string()));

        drain_stream_error_slot(&mut live_mic, &mut live_error, &slot);

        assert!(live_mic.is_none(), "the stale mic should be cleared");
        assert_eq!(
            live_error.as_deref(),
            Some("microphone unavailable: PulseAudio disconnected"),
            "the error should be surfaced with the pre-open path's prefix",
        );
        assert!(
            lock_or_recover(&slot).is_none(),
            "the slot should be cleared after draining",
        );
    }

    #[test]
    fn drain_stream_error_slot_is_noop_when_slot_empty() {
        let mut live_mic: Option<Mic> = None;
        let mut live_error: Option<String> = None;
        let slot = Mutex::new(None);

        drain_stream_error_slot(&mut live_mic, &mut live_error, &slot);

        assert!(live_mic.is_none());
        assert!(live_error.is_none(), "no spurious error should be surfaced");
        assert!(lock_or_recover(&slot).is_none());
    }
}
