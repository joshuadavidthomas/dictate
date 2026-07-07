use std::sync::LazyLock;

use gpui::AnyElement;
use gpui::App;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::rgb;

use crate::components;
use crate::debug::feeders::RECORDED_SPECTRUM_FRAMES;
use crate::debug::feeders::SpectrumSource;
use crate::debug::registry::DebugComponent;
use crate::debug::registry::PreviewClock;
use crate::debug::stats::FrameRecord;
use crate::dictation::DictationPhase;
use crate::spectrum::DEFAULT_WAVEFORM_SMOOTHING;
use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::advance_waveform_bands;

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
    Transcribing,
    Unavailable,
}

impl OverlayScenario {
    const ALL: [Self; 6] = [
        Self::Idle,
        Self::RecordingSine,
        Self::RecordingConstant,
        Self::RecordingFrames,
        Self::Transcribing,
        Self::Unavailable,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::RecordingSine => "recording-sine",
            Self::RecordingConstant => "recording-constant",
            Self::RecordingFrames => "recording-frames",
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

    const fn phase(self) -> DictationPhase {
        match self {
            Self::Idle => DictationPhase::Idle,
            Self::RecordingSine | Self::RecordingConstant | Self::RecordingFrames => {
                DictationPhase::Recording
            }
            Self::Transcribing => DictationPhase::Transcribing,
            Self::Unavailable => DictationPhase::Unavailable,
        }
    }

    const fn spectrum(self) -> SpectrumSource {
        match self {
            Self::Idle | Self::Transcribing | Self::Unavailable => SpectrumSource::Silent,
            Self::RecordingSine => SpectrumSource::SineSweep,
            Self::RecordingConstant => SpectrumSource::Constant(0.55),
            Self::RecordingFrames => SpectrumSource::Frames(&RECORDED_SPECTRUM_FRAMES),
        }
    }
}

pub(in crate::debug) struct OverlayPreviewState {
    displayed_bands: [f32; SPECTRUM_BANDS],
    visual_active: bool,
}

impl OverlayPreviewState {
    pub(in crate::debug) fn new(scenario_id: &str, clock: PreviewClock) -> Self {
        Self {
            displayed_bands: target_bands(scenario_id, clock),
            visual_active: false,
        }
    }

    pub(in crate::debug) fn reset(&mut self, scenario_id: &str, clock: PreviewClock) {
        *self = Self::new(scenario_id, clock);
    }

    pub(in crate::debug) fn advance(
        &mut self,
        scenario_id: &str,
        clock: PreviewClock,
        frame_delta: std::time::Duration,
    ) -> FrameRecord {
        let target_bands = target_bands(scenario_id, clock);
        let advance = advance_waveform_bands(
            self.displayed_bands,
            self.visual_active,
            target_bands,
            frame_delta.as_secs_f32(),
            DEFAULT_WAVEFORM_SMOOTHING,
        );

        self.displayed_bands = advance.smoothed_bands;
        self.visual_active = advance.gate_state.is_open();

        FrameRecord::new(
            scenario_id,
            clock.frame_index,
            frame_delta,
            target_bands,
            advance.smoothed_bands,
            advance.gate_state,
        )
    }
}

fn target_bands(scenario_id: &str, clock: PreviewClock) -> [f32; SPECTRUM_BANDS] {
    OverlayScenario::from_id(scenario_id)
        .expect("debug selection should validate overlay scenarios")
        .spectrum()
        .frame_at(clock.elapsed, clock.frame_index)
}

pub(in crate::debug) struct OverlayPreview;

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

    fn preview(
        &self,
        scenario: &str,
        clock: PreviewClock,
        latest_frame: Option<&FrameRecord>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let scenario = OverlayScenario::from_id(scenario)
            .expect("debug selection should validate overlay scenarios");
        let bands = latest_frame
            .filter(|frame| frame.scenario_id.as_str() == scenario.id())
            .map(|frame| frame.smoothed_bands)
            .unwrap_or_else(|| target_bands(scenario.id(), clock));

        div()
            .id("debug-overlay-preview")
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x1f2937))
            .bg(rgb(0x0b1020))
            .child(
                components::Panel::new("debug-overlay-panel")
                    .child(components::Waveform::new(bands))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xf9fafb))
                            .child(scenario.phase().label()),
                    ),
            )
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
    fn each_scenario_resolves_phase_and_spectrum_source() {
        let expected = [
            (
                OverlayScenario::Idle,
                DictationPhase::Idle,
                SpectrumSource::Silent,
            ),
            (
                OverlayScenario::RecordingSine,
                DictationPhase::Recording,
                SpectrumSource::SineSweep,
            ),
            (
                OverlayScenario::RecordingConstant,
                DictationPhase::Recording,
                SpectrumSource::Constant(0.55),
            ),
            (
                OverlayScenario::RecordingFrames,
                DictationPhase::Recording,
                SpectrumSource::Frames(&RECORDED_SPECTRUM_FRAMES),
            ),
            (
                OverlayScenario::Transcribing,
                DictationPhase::Transcribing,
                SpectrumSource::Silent,
            ),
            (
                OverlayScenario::Unavailable,
                DictationPhase::Unavailable,
                SpectrumSource::Silent,
            ),
        ];

        for (scenario, phase, source) in expected {
            assert_eq!(scenario.phase(), phase);
            assert_eq!(scenario.spectrum(), source);
        }
    }
}
