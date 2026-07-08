use std::io;
use std::sync::LazyLock;

use anyhow::anyhow;
use gpui::AnyElement;
use gpui::App;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::rgb;

use crate::debug::chrome::StatBlockOptions;
use crate::debug::chrome::stat_block;
use crate::debug::chrome::stats_row;
use crate::debug::registry::DebugComponent;
use crate::delivery::ClipboardFailure;
use crate::delivery::ClipboardSink;
use crate::delivery::ConfirmedDeliveryTarget;
use crate::delivery::DeliveryAttemptFailure;
use crate::delivery::DeliveryReport;
use crate::delivery::DeliveryTarget;
use crate::delivery::deliver_with_effects;
use crate::insertion::InsertFailure;
use crate::insertion::InsertOutcome;
use crate::insertion::TextInsertionBackend;

const SAMPLE_TEXT: &str = "Ship the insert delivery path.";
static SCENARIO_IDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    InsertScenario::ALL
        .iter()
        .map(|scenario| scenario.id())
        .collect()
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InsertScenario {
    Inserted,
    FallbackNoTextInput,
    FallbackNoWayland,
    FallbackClipboardFailed,
    FallbackStdoutFailed,
    BackendFailed,
}

impl InsertScenario {
    const ALL: [Self; 6] = [
        Self::Inserted,
        Self::FallbackNoTextInput,
        Self::FallbackNoWayland,
        Self::FallbackClipboardFailed,
        Self::FallbackStdoutFailed,
        Self::BackendFailed,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Inserted => "inserted",
            Self::FallbackNoTextInput => "fallback-no-text-input",
            Self::FallbackNoWayland => "fallback-no-wayland",
            Self::FallbackClipboardFailed => "fallback-clipboard-failed",
            Self::FallbackStdoutFailed => "fallback-stdout-failed",
            Self::BackendFailed => "backend-failed",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scenario| scenario.id() == id)
    }

    fn insertion_outcome(self, text_len: usize) -> InsertOutcome {
        match self {
            Self::Inserted => InsertOutcome::SentToInputMethod {
                sent_bytes: text_len,
            },
            Self::FallbackNoTextInput
            | Self::FallbackClipboardFailed
            | Self::FallbackStdoutFailed => {
                InsertOutcome::NotInserted(InsertFailure::InputMethodRejected)
            }
            Self::FallbackNoWayland => {
                InsertOutcome::NotInserted(InsertFailure::NoWaylandDisplay {
                    message: "WAYLAND_DISPLAY is not set".to_owned(),
                })
            }
            Self::BackendFailed => InsertOutcome::DeliveryUncertain {
                maybe_sent_bytes: text_len,
                failure: InsertFailure::ProtocolFailed {
                    message: "input method protocol failed after a commit request was queued"
                        .to_owned(),
                },
            },
        }
    }

    const fn clipboard_fails(self) -> bool {
        matches!(
            self,
            Self::FallbackClipboardFailed | Self::FallbackStdoutFailed
        )
    }

    const fn stdout_fails(self) -> bool {
        matches!(self, Self::FallbackStdoutFailed)
    }
}

pub(in crate::debug) struct InsertPreview;

impl InsertPreview {
    pub(in crate::debug) const fn new() -> Self {
        Self
    }
}

impl DebugComponent for InsertPreview {
    fn name(&self) -> &'static str {
        "insert"
    }

    fn description(&self) -> &'static str {
        "Simulate insert delivery outcomes without touching Wayland, clipboard, or focused apps."
    }

    fn scenarios(&self) -> &'static [&'static str] {
        SCENARIO_IDS.as_slice()
    }

    fn preview(&self, scenario: &str, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let preview = preview_report_for_scenario(scenario)
            .unwrap_or_else(|| PreviewReport::unknown_scenario(scenario));

        div()
            .id("debug-insert-preview")
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x1f2937))
            .bg(rgb(0x0b1020))
            .p(px(18.0))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Insert delivery simulator"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x9ca3af))
                            .child("Uses the real delivery policy with fake effects; no live insertion or clipboard writes occur."),
                    ),
            )
            .child(
                stats_row()
                    .child(stat_block(
                        "scenario",
                        preview.scenario.clone(),
                        StatBlockOptions::fixed(220.0).truncate(),
                    ))
                    .child(stat_block(
                        "requested",
                        preview.requested_target.clone(),
                        StatBlockOptions::fixed(120.0),
                    ))
                    .child(stat_block(
                        "outcome",
                        preview.final_outcome.clone(),
                        StatBlockOptions::flexible()
                            .value_color(preview.outcome_color)
                            .truncate(),
                    )),
            )
            .child(detail_panel(&preview))
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x1f2937))
                    .bg(rgb(0x111827))
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6b7280))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("MESSAGE TEXT"),
                    )
                    .child(div().text_color(rgb(0xf9fafb)).child(SAMPLE_TEXT)),
            )
            .into_any_element()
    }
}

fn detail_panel(preview: &PreviewReport) -> gpui::Div {
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x1f2937))
        .bg(rgb(0x111827))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(detail_row("insert", &preview.insert_detail))
        .child(detail_row("clipboard", &preview.clipboard_detail))
        .child(detail_row("stdout", &preview.stdout_detail))
        .child(
            div()
                .mt(px(4.0))
                .text_sm()
                .text_color(rgb(0xd1d5db))
                .child(preview.message.clone()),
        )
}

fn detail_row(label: &'static str, value: &str) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(86.0))
                .text_xs()
                .text_color(rgb(0x6b7280))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(rgb(0xf9fafb))
                .child(value.to_owned()),
        )
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewReport {
    scenario: String,
    requested_target: String,
    final_outcome: String,
    outcome_color: u32,
    insert_detail: String,
    clipboard_detail: String,
    stdout_detail: String,
    message: String,
}

impl PreviewReport {
    fn unknown_scenario(scenario: &str) -> Self {
        Self {
            scenario: scenario.to_owned(),
            requested_target: "insert".to_owned(),
            final_outcome: "unknown scenario".to_owned(),
            outcome_color: 0xf87171,
            insert_detail: "not run".to_owned(),
            clipboard_detail: "not run".to_owned(),
            stdout_detail: "not run".to_owned(),
            message: format!("unknown insert debug scenario {scenario:?}"),
        }
    }
}

fn preview_report_for_scenario(scenario_id: &str) -> Option<PreviewReport> {
    let scenario = InsertScenario::from_id(scenario_id)?;
    let report = delivery_report_for_scenario(scenario);
    Some(summarize_report(scenario_id, &report))
}

fn delivery_report_for_scenario(scenario: InsertScenario) -> DeliveryReport {
    let mut insertion = FakeInsertion { scenario };
    let mut clipboard = FakeClipboard {
        fail: scenario.clipboard_fails(),
    };

    deliver_with_effects(
        DeliveryTarget::Insert,
        SAMPLE_TEXT,
        &mut insertion,
        &mut clipboard,
        || FailingWriter {
            fail: scenario.stdout_fails(),
            buffer: Vec::new(),
        },
    )
}

fn summarize_report(scenario_id: &str, report: &DeliveryReport) -> PreviewReport {
    match report {
        DeliveryReport::InsertRequestSent { sent_bytes } => PreviewReport {
            scenario: scenario_id.to_owned(),
            requested_target: "insert".to_owned(),
            final_outcome: "sent to input method".to_owned(),
            outcome_color: 0x34d399,
            insert_detail: format!("{sent_bytes} bytes sent to the Wayland input method"),
            clipboard_detail: "not used".to_owned(),
            stdout_detail: "not used".to_owned(),
            message: "Focused app insertion is requested, but Wayland does not confirm that text appeared.".to_owned(),
        },
        DeliveryReport::InsertUncertain {
            maybe_sent_bytes,
            failure,
        } => PreviewReport {
            scenario: scenario_id.to_owned(),
            requested_target: "insert".to_owned(),
            final_outcome: "uncertain; fallback skipped".to_owned(),
            outcome_color: 0xfbbf24,
            insert_detail: format!(
                "{maybe_sent_bytes} bytes may have been sent before failure: {failure}"
            ),
            clipboard_detail: "not used".to_owned(),
            stdout_detail: "not used".to_owned(),
            message: "Fallback is skipped because retrying through clipboard/stdout could duplicate text.".to_owned(),
        },
        DeliveryReport::Delivered {
            target,
            preceding_failures,
        } => summarize_delivered_report(scenario_id, *target, preceding_failures),
        DeliveryReport::NotDelivered { failures } => {
            let failure_text = failures
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            PreviewReport {
                scenario: scenario_id.to_owned(),
                requested_target: "insert".to_owned(),
                final_outcome: "not delivered".to_owned(),
                outcome_color: 0xf87171,
                insert_detail: failure_detail(failures.iter(), FailureKind::Insert),
                clipboard_detail: failure_detail(failures.iter(), FailureKind::Clipboard),
                stdout_detail: failure_detail(failures.iter(), FailureKind::Stdout),
                message: format!("All delivery paths failed: {failure_text}"),
            }
        }
    }
}

fn summarize_delivered_report(
    scenario_id: &str,
    target: ConfirmedDeliveryTarget,
    preceding_failures: &[DeliveryAttemptFailure],
) -> PreviewReport {
    let final_outcome = match target {
        ConfirmedDeliveryTarget::Stdout => "stdout fallback",
        ConfirmedDeliveryTarget::Clipboard => {
            if preceding_failures.is_empty() {
                "copied to clipboard"
            } else {
                "clipboard fallback"
            }
        }
    };
    let stdout_detail = match target {
        ConfirmedDeliveryTarget::Stdout => "wrote text to stdout".to_owned(),
        ConfirmedDeliveryTarget::Clipboard => "not used".to_owned(),
    };
    let clipboard_detail = match target {
        ConfirmedDeliveryTarget::Clipboard => "copied text to clipboard".to_owned(),
        ConfirmedDeliveryTarget::Stdout => {
            failure_detail(preceding_failures.iter(), FailureKind::Clipboard)
        }
    };

    PreviewReport {
        scenario: scenario_id.to_owned(),
        requested_target: "insert".to_owned(),
        final_outcome: final_outcome.to_owned(),
        outcome_color: 0x34d399,
        insert_detail: failure_detail(preceding_failures.iter(), FailureKind::Insert),
        clipboard_detail,
        stdout_detail,
        message: format!("Delivery completed via {final_outcome}."),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureKind {
    Insert,
    Clipboard,
    Stdout,
}

fn failure_detail<'a>(
    failures: impl Iterator<Item = &'a DeliveryAttemptFailure>,
    kind: FailureKind,
) -> String {
    failures
        .filter_map(|failure| match (kind, failure) {
            (FailureKind::Insert, DeliveryAttemptFailure::Insert(failure)) => {
                Some(failure.to_string())
            }
            (FailureKind::Clipboard, DeliveryAttemptFailure::Clipboard(failure)) => {
                Some(failure.to_string())
            }
            (FailureKind::Stdout, DeliveryAttemptFailure::Stdout(failure)) => {
                Some(failure.to_string())
            }
            _ => None,
        })
        .next()
        .unwrap_or_else(|| "not used".to_owned())
}

struct FakeInsertion {
    scenario: InsertScenario,
}

impl TextInsertionBackend for FakeInsertion {
    fn insert(&mut self, text: &str) -> InsertOutcome {
        self.scenario.insertion_outcome(text.len())
    }
}

struct FakeClipboard {
    fail: bool,
}

impl ClipboardSink for FakeClipboard {
    fn copy(&mut self, _text: &str) -> std::result::Result<(), ClipboardFailure> {
        if self.fail {
            Err(ClipboardFailure::CopyFailed {
                source: anyhow!("simulated clipboard failure"),
            })
        } else {
            Ok(())
        }
    }
}

struct FailingWriter {
    fail: bool,
    buffer: Vec<u8>,
}

impl io::Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.fail {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "simulated stdout failure",
            ))
        } else {
            self.buffer.write(buf)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_ids_are_derived_from_supported_scenarios() {
        let expected = InsertScenario::ALL
            .iter()
            .map(|scenario| scenario.id())
            .collect::<Vec<_>>();

        assert_eq!(SCENARIO_IDS.as_slice(), expected.as_slice());
        for id in SCENARIO_IDS.iter() {
            assert!(InsertScenario::from_id(id).is_some());
        }
    }

    #[test]
    fn scenarios_map_through_delivery_policy() {
        let cases = [
            ("inserted", "sent to input method"),
            ("fallback-no-text-input", "clipboard fallback"),
            ("fallback-no-wayland", "clipboard fallback"),
            ("fallback-clipboard-failed", "stdout fallback"),
            ("fallback-stdout-failed", "not delivered"),
            ("backend-failed", "uncertain; fallback skipped"),
        ];

        for (scenario, expected_outcome) in cases {
            let preview = preview_report_for_scenario(scenario).unwrap();

            assert_eq!(preview.final_outcome, expected_outcome);
        }
    }

    #[test]
    fn clipboard_failure_scenario_reports_both_fallback_failures() {
        let preview = preview_report_for_scenario("fallback-clipboard-failed").unwrap();

        assert!(preview.insert_detail.contains("rejected"));
        assert!(
            preview
                .clipboard_detail
                .contains("simulated clipboard failure")
        );
        assert_eq!(preview.stdout_detail, "wrote text to stdout");
    }

    #[test]
    fn stdout_failure_scenario_reports_all_failures() {
        let preview = preview_report_for_scenario("fallback-stdout-failed").unwrap();

        assert_eq!(preview.final_outcome, "not delivered");
        assert!(preview.insert_detail.contains("rejected"));
        assert!(
            preview
                .clipboard_detail
                .contains("simulated clipboard failure")
        );
        assert!(preview.stdout_detail.contains("simulated stdout failure"));
    }

    #[test]
    fn backend_failed_scenario_skips_fallback() {
        let preview = preview_report_for_scenario("backend-failed").unwrap();

        assert_eq!(preview.clipboard_detail, "not used");
        assert!(preview.message.contains("could duplicate text"));
    }
}
