use std::time::Duration;

use serde::Serialize;

use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::WaveformGateState;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::debug) enum StatsRecordKind {
    Frame,
    Aggregates,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(in crate::debug) struct FrameRecord {
    #[serde(rename = "type")]
    pub(in crate::debug) kind: StatsRecordKind,
    pub(in crate::debug) scenario_id: String,
    pub(in crate::debug) frame_index: u64,
    pub(in crate::debug) frame_delta_ms: f32,
    pub(in crate::debug) target_bands: [f32; SPECTRUM_BANDS],
    pub(in crate::debug) smoothed_bands: [f32; SPECTRUM_BANDS],
    pub(in crate::debug) gate_state: WaveformGateState,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(in crate::debug) struct AggregateRecord {
    #[serde(rename = "type")]
    pub(in crate::debug) kind: StatsRecordKind,
    pub(in crate::debug) frame_count: u64,
    pub(in crate::debug) elapsed_ms: f32,
    pub(in crate::debug) measured_fps: f32,
    pub(in crate::debug) dropped_tick_count: u64,
}

#[derive(Clone, Debug)]
pub(in crate::debug) struct StatsSession {
    expected_frame_interval: Duration,
    frame_count: u64,
    elapsed: Duration,
    dropped_tick_count: u64,
    latest_frame: Option<FrameRecord>,
}

impl StatsSession {
    pub(in crate::debug) fn new(expected_frame_interval: Duration) -> Self {
        Self {
            expected_frame_interval,
            frame_count: 0,
            elapsed: Duration::ZERO,
            dropped_tick_count: 0,
            latest_frame: None,
        }
    }

    pub(in crate::debug) fn record_frame(&mut self, mut frame: FrameRecord) -> FrameRecord {
        self.frame_count += 1;
        frame.frame_index = self.frame_count;
        self.elapsed += Duration::from_secs_f32(frame.frame_delta_ms / 1_000.0);
        self.dropped_tick_count +=
            dropped_ticks(frame.frame_delta_ms, self.expected_frame_interval);
        self.latest_frame = Some(frame.clone());

        frame
    }

    pub(in crate::debug) fn latest_frame(&self) -> Option<&FrameRecord> {
        self.latest_frame.as_ref()
    }

    pub(in crate::debug) fn aggregates(&self) -> AggregateRecord {
        aggregates_from_parts(self.frame_count, self.elapsed, self.dropped_tick_count)
    }

    pub(in crate::debug) fn frame_count(&self) -> u64 {
        self.frame_count
    }

    pub(in crate::debug) fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

impl FrameRecord {
    pub(in crate::debug) fn new(
        scenario_id: impl Into<String>,
        frame_index: u64,
        frame_delta: Duration,
        target_bands: [f32; SPECTRUM_BANDS],
        smoothed_bands: [f32; SPECTRUM_BANDS],
        gate_state: WaveformGateState,
    ) -> Self {
        Self {
            kind: StatsRecordKind::Frame,
            scenario_id: scenario_id.into(),
            frame_index,
            frame_delta_ms: frame_delta.as_secs_f32() * 1_000.0,
            target_bands,
            smoothed_bands,
            gate_state,
        }
    }
}

fn aggregates_from_parts(
    frame_count: u64,
    elapsed: Duration,
    dropped_tick_count: u64,
) -> AggregateRecord {
    let elapsed_secs = elapsed.as_secs_f32();

    AggregateRecord {
        kind: StatsRecordKind::Aggregates,
        frame_count,
        elapsed_ms: elapsed_secs * 1_000.0,
        measured_fps: if elapsed_secs > 0.0 {
            frame_count as f32 / elapsed_secs
        } else {
            0.0
        },
        dropped_tick_count,
    }
}

fn dropped_ticks(frame_delta_ms: f32, expected_frame_interval: Duration) -> u64 {
    let expected_ms = expected_frame_interval.as_secs_f32() * 1_000.0;
    if expected_ms <= 0.0 || frame_delta_ms <= expected_ms * 1.5 {
        return 0;
    }

    (frame_delta_ms / expected_ms).floor().max(1.0) as u64 - 1
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn aggregation_math_reports_elapsed_fps_and_dropped_ticks() {
        let mut session = StatsSession::new(Duration::from_millis(16));
        session.record_frame(FrameRecord::new(
            "recording-sine",
            0,
            Duration::from_millis(16),
            [0.2; SPECTRUM_BANDS],
            [0.1; SPECTRUM_BANDS],
            WaveformGateState::Open,
        ));
        session.record_frame(FrameRecord::new(
            "recording-sine",
            0,
            Duration::from_millis(48),
            [0.4; SPECTRUM_BANDS],
            [0.3; SPECTRUM_BANDS],
            WaveformGateState::Open,
        ));

        let aggregates = session.aggregates();

        assert_eq!(aggregates.frame_count, 2);
        assert!((aggregates.elapsed_ms - 64.0).abs() < 0.001);
        assert!((aggregates.measured_fps - 31.25).abs() < 0.001);
        assert_eq!(aggregates.dropped_tick_count, 2);
    }

    #[test]
    fn frame_record_json_shape_is_stable() {
        let record = FrameRecord::new(
            "recording-sine",
            7,
            Duration::from_millis(16),
            [0.25; SPECTRUM_BANDS],
            [0.125; SPECTRUM_BANDS],
            WaveformGateState::Closed,
        );
        let value: Value = serde_json::to_value(record).unwrap();

        assert_eq!(value["type"], "frame");
        assert_eq!(value["scenario_id"], "recording-sine");
        assert_eq!(value["frame_index"], 7);
        assert_eq!(value["frame_delta_ms"], 16.0);
        assert_eq!(
            value["target_bands"].as_array().unwrap().len(),
            SPECTRUM_BANDS
        );
        assert_eq!(
            value["smoothed_bands"].as_array().unwrap().len(),
            SPECTRUM_BANDS
        );
        assert_eq!(value["gate_state"], "closed");
    }

    #[test]
    fn aggregate_record_json_shape_is_stable() {
        let record = aggregates_from_parts(3, Duration::from_millis(48), 1);
        let value: Value = serde_json::to_value(record).unwrap();

        assert_eq!(value["type"], "aggregates");
        assert_eq!(value["frame_count"], 3);
        assert_eq!(value["elapsed_ms"], 48.0);
        assert_eq!(value["measured_fps"], 62.5);
        assert_eq!(value["dropped_tick_count"], 1);
    }
}
