use std::cell::RefCell;
use std::sync::LazyLock;
use std::time::Duration;

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

use crate::app::OVERLAY_WINDOW_HEIGHT;
use crate::app::OVERLAY_WINDOW_WIDTH;
use crate::debug::feeders::RECORDED_SPECTRUM_FRAMES;
use crate::debug::feeders::SpectrumSource;
use crate::debug::registry::DebugComponent;
use crate::debug::registry::PreviewClock;
use crate::debug::registry::ScenarioChip;
use crate::debug::registry::ScenarioRow;
use crate::debug::stats::FrameRecord;
use crate::mic::SpectrumMic;
use crate::mic::capture_spectrum;
use crate::overlay::OverlayView;
use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::SpectrumLevels;

static SCENARIO_IDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    OverlayScenario::ALL
        .iter()
        .map(|scenario| scenario.id())
        .collect()
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayScenario {
    Idle,
    RecordingSine,
    RecordingConstant,
    RecordingFrames,
    RecordingLive,
    Transcribing,
    Unavailable,
}

impl OverlayScenario {
    const ALL: [Self; 7] = [
        Self::Idle,
        Self::RecordingSine,
        Self::RecordingConstant,
        Self::RecordingFrames,
        Self::RecordingLive,
        Self::Transcribing,
        Self::Unavailable,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::RecordingSine => "recording-sine",
            Self::RecordingConstant => "recording-constant",
            Self::RecordingFrames => "recording-frames",
            Self::RecordingLive => "recording-live",
            Self::Transcribing => "transcribing",
            Self::Unavailable => "unavailable",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scenario| scenario.id() == id)
    }

    const fn spectrum(self) -> SpectrumPlan {
        match self {
            Self::Idle | Self::Transcribing | Self::Unavailable => {
                SpectrumPlan::Deterministic(SpectrumSource::Silent)
            }
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

pub(in crate::debug) struct OverlayPreviewState {
    levels: SpectrumLevels,
    overlay: Entity<OverlayView>,
    live_mic: Option<SpectrumMic>,
    live_error: Option<String>,
}

impl OverlayPreviewState {
    pub(in crate::debug) fn new(
        scenario_id: &str,
        clock: PreviewClock,
        cx: &mut impl AppContext,
    ) -> Self {
        let levels = SpectrumLevels::new();
        let scenario = OverlayScenario::from_id(scenario_id)
            .expect("debug selection should validate overlay scenarios");

        levels.set(target_bands_for_scenario(scenario, clock));

        let overlay = cx.new(|cx| OverlayView::new(levels.clone(), cx));

        Self {
            levels,
            overlay,
            live_mic: None,
            live_error: None,
        }
    }

    pub(in crate::debug) fn reset(
        &mut self,
        scenario_id: &str,
        clock: PreviewClock,
        cx: &mut impl AppContext,
    ) {
        *self = Self::new(scenario_id, clock, cx);
    }

    pub(in crate::debug) fn advance(
        &mut self,
        scenario_id: &str,
        clock: PreviewClock,
        frame_delta: std::time::Duration,
        cx: &mut impl AppContext,
    ) -> FrameRecord {
        let scenario = OverlayScenario::from_id(scenario_id)
            .expect("debug selection should validate overlay scenarios");

        match scenario.spectrum() {
            SpectrumPlan::Deterministic(_) => {
                drop(self.live_mic.take());
                self.live_error = None;
                self.levels.set(target_bands_for_scenario(scenario, clock));
            }
            SpectrumPlan::LiveMic => self.ensure_live_mic(),
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
            match capture_spectrum(self.levels.clone()) {
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

pub(in crate::debug) struct OverlayPreview {
    state: RefCell<Option<OverlayPreviewState>>,
}

impl OverlayPreview {
    pub(in crate::debug) fn new() -> Self {
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
                        label: "idle",
                        activates: "idle",
                        matches: vec!["idle"],
                    },
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
                        label: "transcribing",
                        activates: "transcribing",
                        matches: vec!["transcribing"],
                    },
                    ScenarioChip {
                        label: "unavailable",
                        activates: "unavailable",
                        matches: vec!["unavailable"],
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

        Some(
            self.state
                .borrow_mut()
                .as_mut()
                .expect("overlay preview state should exist")
                .advance(scenario, clock, frame_delta, cx),
        )
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
            let state = state.as_ref().expect("overlay preview state should exist");

            (state.overlay(), state.live_error().map(str::to_string))
        };

        div()
            .id("debug-overlay-preview")
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x1f2937))
            .bg(rgb(0x0b1020))
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
                        .text_color(rgb(0x9ca3af))
                        .child(format!("live mic: {error}")),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                OverlayScenario::Idle,
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
                OverlayScenario::Unavailable,
                SpectrumPlan::Deterministic(SpectrumSource::Silent),
            ),
        ];

        for (scenario, source) in expected {
            assert_eq!(scenario.spectrum(), source);
        }
    }
}
