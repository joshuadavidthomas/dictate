use std::time::Duration;

use gpui::AnyElement;
use gpui::App;
use gpui::Window;

use crate::PlanFactory;
use crate::screens::bench::BenchPreview;
use crate::screens::overlay::OverlayPreview;
use crate::stats::FrameRecord;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreviewClock {
    pub(crate) elapsed: Duration,
    pub(crate) frame_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScenarioChip {
    pub(crate) label: &'static str,
    pub(crate) activates: &'static str,
    pub(crate) matches: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScenarioRow {
    pub(crate) label: &'static str,
    pub(crate) chips: Vec<ScenarioChip>,
}

pub(crate) trait DebugComponent {
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

pub(crate) fn registry(
    plan_factory: PlanFactory,
    fixture_root: std::path::PathBuf,
) -> Vec<Box<dyn DebugComponent>> {
    vec![
        Box::new(OverlayPreview::new()),
        Box::new(BenchPreview::new(plan_factory, fixture_root)),
    ]
}
