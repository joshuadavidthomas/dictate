use std::time::Duration;

use gpui::AnyElement;
use gpui::App;
use gpui::Window;

use crate::debug::screens::bench::BenchPreview;
use crate::debug::screens::overlay::OverlayPreview;
use crate::debug::stats::FrameRecord;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::debug) struct PreviewClock {
    pub(in crate::debug) elapsed: Duration,
    pub(in crate::debug) frame_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::debug) struct ScenarioChip {
    pub(in crate::debug) label: &'static str,
    pub(in crate::debug) activates: &'static str,
    pub(in crate::debug) matches: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::debug) struct ScenarioRow {
    pub(in crate::debug) label: &'static str,
    pub(in crate::debug) chips: Vec<ScenarioChip>,
}

pub(in crate::debug) trait DebugComponent {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn scenarios(&self) -> &'static [&'static str];

    fn scenario_rows(&self) -> Vec<ScenarioRow> {
        vec![ScenarioRow {
            label: "scenario",
            chips: self
                .scenarios()
                .iter()
                .map(|&scenario| ScenarioChip {
                    label: scenario,
                    activates: scenario,
                    matches: vec![scenario],
                })
                .collect(),
        }]
    }

    fn produces_stats(&self) -> bool {
        false
    }

    fn reset(&self, _scenario: &str, _cx: &mut App) {}

    fn deactivate(&self) {}

    fn advance(
        &self,
        _scenario: &str,
        _clock: PreviewClock,
        _frame_delta: Duration,
        _cx: &mut App,
    ) -> Option<FrameRecord> {
        None
    }

    fn preview(&self, scenario: &str, window: &mut Window, cx: &mut App) -> AnyElement;
}

pub(in crate::debug) fn registry() -> Vec<Box<dyn DebugComponent>> {
    vec![
        Box::new(OverlayPreview::new()),
        Box::new(BenchPreview::new()),
    ]
}
