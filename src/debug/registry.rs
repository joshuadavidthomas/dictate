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

pub(in crate::debug) trait DebugComponent {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn scenarios(&self) -> &'static [&'static str];
    fn preview(
        &self,
        scenario: &str,
        clock: PreviewClock,
        latest_frame: Option<&FrameRecord>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}

pub(in crate::debug) fn registry() -> Vec<Box<dyn DebugComponent>> {
    vec![Box::new(OverlayPreview), Box::new(BenchPreview::new())]
}
