mod clipboard;
mod wtype;

use std::fmt;
use std::io;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use clipboard::ClipboardSnapshot;
use clipboard::WaylandClipboard;
use wtype::ClipboardPasteChordBackend;
use wtype::ClipboardPasteChordOutcome;
use wtype::WtypeOutcome;

const MAX_TRANSCRIPT_BYTES: usize = 1024 * 1024;
const CLIPBOARD_SETTLE_INTERVAL: Duration = Duration::from_millis(40);
const CLIPBOARD_SETTLE_CHECKS: usize = 4;
const PASTE_GRACE_PERIOD: Duration = Duration::from_millis(180);

static INSERT_TRANSACTION: Mutex<()> = Mutex::new(());
static MARKER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) trait InsertionBackend {
    fn insert(&mut self, text: InsertionText<'_>) -> InsertionOutcome;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InsertionText<'a> {
    text: &'a str,
}

impl<'a> InsertionText<'a> {
    pub(crate) fn new(text: &'a str) -> Option<Self> {
        if text.is_empty() {
            None
        } else {
            Some(Self { text })
        }
    }

    pub(crate) fn as_str(self) -> &'a str {
        self.text
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InsertionOutcome {
    Completed(CompletedInsertion),
    NotInserted(InsertionFailure),
    DeliveryUncertain(UncertainInsertion),
}

#[derive(Debug, Eq, PartialEq)]
pub enum CompletedInsertion {
    ClipboardPaste {
        transcript_bytes: usize,
        restoration: ClipboardRestoration,
    },
    DirectTyping {
        input_bytes: usize,
        fallback_reason: PrePasteFailure,
        clipboard: DirectTypingClipboard,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum UncertainInsertion {
    ClipboardPaste {
        transcript_bytes: usize,
        failure: WtypeFailure,
        restoration: ClipboardRestoration,
    },
    DirectTyping {
        maybe_input_bytes: usize,
        fallback_reason: PrePasteFailure,
        failure: WtypeFailure,
        clipboard: DirectTypingClipboard,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum DirectTypingClipboard {
    NotPublished,
    Published { restoration: ClipboardRestoration },
}

#[derive(Debug, Eq, PartialEq)]
pub enum ClipboardRestoration {
    Restored,
    SkippedNewerClipboard,
    Failed(ClipboardTransactionFailure),
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InsertionFailure {
    #[error(
        "clipboard paste setup failed ({fallback_reason}); direct wtype fallback could not start: {failure}"
    )]
    DirectFallbackUnavailable {
        fallback_reason: PrePasteFailure,
        failure: WtypeFailure,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PrePasteFailure {
    #[error("transcript exceeds the {limit}-byte clipboard transaction limit")]
    TranscriptTooLarge { limit: usize },
    #[error("clipboard snapshot failed: {0}")]
    Snapshot(ClipboardTransactionFailure),
    #[error("clipboard snapshot revalidation failed: {0}")]
    SnapshotRevalidation(ClipboardTransactionFailure),
    #[error("clipboard changed after capture and before temporary publication")]
    ClipboardChangedBeforePublication,
    #[error("temporary clipboard publication failed: {0}")]
    Publication(ClipboardTransactionFailure),
    #[error("temporary clipboard ownership could not be verified: {0}")]
    Verification(ClipboardTransactionFailure),
    #[error("clipboard changed before the paste chord")]
    ClipboardChanged,
    #[error("paste chord could not start: {0}")]
    PasteChordUnavailable(WtypeFailure),
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ClipboardTransactionFailure {
    #[error("clipboard is unavailable while {operation}: {kind}")]
    Access {
        operation: ClipboardOperation,
        kind: ClipboardFailureKind,
    },
    #[error("clipboard advertised {count} MIME types; the limit is {limit}")]
    TooManyMimeTypes { count: usize, limit: usize },
    #[error("clipboard MIME metadata exceeds the {limit}-byte limit")]
    MimeMetadataTooLarge { limit: usize },
    #[error("clipboard data exceeds the {limit}-byte snapshot limit")]
    SnapshotTooLarge { limit: usize },
    #[error("clipboard content transfer timed out while {operation}")]
    TransferTimedOut { operation: ClipboardOperation },
    #[error("clipboard changed while it was being captured")]
    ChangedDuringSnapshot,
    #[error("clipboard did not provide its advertised MIME type")]
    AdvertisedMimeUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardOperation {
    ListMimeTypes,
    ReadSnapshot,
    ConfirmSnapshot,
    RevalidateSnapshot,
    PublishTranscript,
    VerifyMarker,
    VerifyTranscript,
    RestoreSnapshot,
}

impl fmt::Display for ClipboardOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ListMimeTypes => formatter.write_str("listing MIME types"),
            Self::ReadSnapshot => formatter.write_str("reading the snapshot"),
            Self::ConfirmSnapshot => formatter.write_str("confirming the snapshot"),
            Self::RevalidateSnapshot => formatter.write_str("revalidating the snapshot"),
            Self::PublishTranscript => formatter.write_str("publishing the transcript"),
            Self::VerifyMarker => formatter.write_str("checking the transaction marker"),
            Self::VerifyTranscript => formatter.write_str("checking transcript ownership"),
            Self::RestoreSnapshot => formatter.write_str("restoring the snapshot"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipboardFailureKind {
    Empty,
    NoSeats,
    NoMimeType,
    SocketOpen(io::ErrorKind),
    WaylandConnection,
    WaylandCommunication,
    MissingProtocol { name: String, version: u32 },
    PrimarySelectionUnsupported,
    SeatNotFound,
    PipeCreation(io::ErrorKind),
    DataTransfer(io::ErrorKind),
    TemporaryStorage(io::ErrorKind),
}

impl fmt::Display for ClipboardFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("clipboard is empty"),
            Self::NoSeats => formatter.write_str("no Wayland seats"),
            Self::NoMimeType => formatter.write_str("requested MIME type is unavailable"),
            Self::SocketOpen(kind) => write!(formatter, "Wayland socket open failed ({kind:?})"),
            Self::WaylandConnection => formatter.write_str("Wayland connection failed"),
            Self::WaylandCommunication => formatter.write_str("Wayland communication failed"),
            Self::MissingProtocol { name, version } => {
                write!(formatter, "missing Wayland protocol {name} v{version}")
            }
            Self::PrimarySelectionUnsupported => {
                formatter.write_str("primary selection unsupported")
            }
            Self::SeatNotFound => formatter.write_str("Wayland seat not found"),
            Self::PipeCreation(kind) => write!(formatter, "pipe creation failed ({kind:?})"),
            Self::DataTransfer(kind) => write!(formatter, "data transfer failed ({kind:?})"),
            Self::TemporaryStorage(kind) => {
                write!(formatter, "temporary storage failed ({kind:?})")
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum WtypeFailure {
    #[error("failed to start wtype: {message} ({kind:?})")]
    Spawn {
        kind: io::ErrorKind,
        message: String,
    },
    #[error("wtype started without a writable stdin pipe")]
    StdinUnavailable,
    #[error(
        "failed to write text to wtype stdin after {written_bytes} bytes: {message} ({kind:?})"
    )]
    WriteStdin {
        written_bytes: usize,
        kind: io::ErrorKind,
        message: String,
    },
    #[error("wtype did not finish within the bounded wait")]
    TimedOut,
    #[error("failed to wait for wtype: {message} ({kind:?})")]
    Wait {
        kind: io::ErrorKind,
        message: String,
    },
    #[error("wtype exited unsuccessfully: {status}")]
    Exited { status: WtypeExitStatus },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WtypeExitStatus {
    Code(i32),
    Signal(i32),
    Unknown,
}

impl fmt::Display for WtypeExitStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(formatter, "exit code {code}"),
            Self::Signal(signal) => write!(formatter, "signal {signal}"),
            Self::Unknown => formatter.write_str("unknown exit status"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TransactionMarker(String);

impl TransactionMarker {
    fn new() -> Self {
        let sequence = MARKER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Self(format!("{}-{nanos}-{sequence}", std::process::id()))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TemporaryOwnership {
    Marker,
    Transcript,
    Changed,
}

trait ClipboardTransport {
    fn snapshot(&mut self) -> Result<ClipboardSnapshot, ClipboardTransactionFailure>;

    fn snapshot_is_current(
        &mut self,
        snapshot: &ClipboardSnapshot,
    ) -> Result<bool, ClipboardTransactionFailure>;

    fn publish(
        &mut self,
        text: &str,
        marker: &TransactionMarker,
    ) -> Result<(), ClipboardTransactionFailure>;

    fn temporary_ownership(
        &mut self,
        text: &str,
        marker: &TransactionMarker,
    ) -> Result<TemporaryOwnership, ClipboardTransactionFailure>;

    fn restore(&mut self, snapshot: ClipboardSnapshot) -> Result<(), ClipboardTransactionFailure>;
}

trait ClipboardPasteChord {
    fn send_clipboard_paste_chord(&mut self) -> ClipboardPasteChordOutcome;
}

trait DirectTyper {
    fn type_text(&mut self, text: InsertionText<'_>) -> WtypeOutcome;
}

#[derive(Debug, Default)]
pub(crate) struct ClipboardPasteBackend {
    clipboard: WaylandClipboard,
    paste_chord: ClipboardPasteChordBackend,
    direct: WtypeBackend,
}

impl InsertionBackend for ClipboardPasteBackend {
    fn insert(&mut self, text: InsertionText<'_>) -> InsertionOutcome {
        let _transaction = INSERT_TRANSACTION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        insert_transaction(
            &mut self.clipboard,
            &mut self.paste_chord,
            &mut self.direct,
            text,
            thread::sleep,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WtypeBackend;

impl DirectTyper for WtypeBackend {
    fn type_text(&mut self, text: InsertionText<'_>) -> WtypeOutcome {
        wtype::type_text(text)
    }
}

fn insert_transaction(
    clipboard: &mut impl ClipboardTransport,
    paste_chord: &mut impl ClipboardPasteChord,
    direct: &mut impl DirectTyper,
    text: InsertionText<'_>,
    mut sleep: impl FnMut(Duration),
) -> InsertionOutcome {
    let transcript = text.as_str();
    if transcript.len() > MAX_TRANSCRIPT_BYTES {
        return direct_fallback(
            direct,
            text,
            PrePasteFailure::TranscriptTooLarge {
                limit: MAX_TRANSCRIPT_BYTES,
            },
            DirectTypingClipboard::NotPublished,
        );
    }

    let snapshot = match clipboard.snapshot() {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            return direct_fallback(
                direct,
                text,
                PrePasteFailure::Snapshot(failure),
                DirectTypingClipboard::NotPublished,
            );
        }
    };
    let marker = TransactionMarker::new();
    match clipboard.snapshot_is_current(&snapshot) {
        Ok(true) => {}
        Ok(false) => {
            return direct_fallback(
                direct,
                text,
                PrePasteFailure::ClipboardChangedBeforePublication,
                DirectTypingClipboard::NotPublished,
            );
        }
        Err(failure) => {
            return direct_fallback(
                direct,
                text,
                PrePasteFailure::SnapshotRevalidation(failure),
                DirectTypingClipboard::NotPublished,
            );
        }
    }

    if let Err(failure) = clipboard.publish(transcript, &marker) {
        return direct_fallback(
            direct,
            text,
            PrePasteFailure::Publication(failure),
            DirectTypingClipboard::NotPublished,
        );
    }

    if let Err(failure) = stabilize_temporary_clipboard(clipboard, transcript, &marker, &mut sleep)
    {
        let clipboard = DirectTypingClipboard::Published {
            restoration: restore_temporary_clipboard(clipboard, snapshot, transcript, &marker),
        };
        return direct_fallback(direct, text, failure, clipboard);
    }

    match paste_chord.send_clipboard_paste_chord() {
        ClipboardPasteChordOutcome::NotSent(failure) => {
            let clipboard = DirectTypingClipboard::Published {
                restoration: restore_temporary_clipboard(clipboard, snapshot, transcript, &marker),
            };
            direct_fallback(
                direct,
                text,
                PrePasteFailure::PasteChordUnavailable(failure),
                clipboard,
            )
        }
        ClipboardPasteChordOutcome::Sent => {
            sleep(PASTE_GRACE_PERIOD);
            InsertionOutcome::Completed(CompletedInsertion::ClipboardPaste {
                transcript_bytes: transcript.len(),
                restoration: restore_temporary_clipboard(clipboard, snapshot, transcript, &marker),
            })
        }
        ClipboardPasteChordOutcome::DeliveryUncertain(failure) => {
            sleep(PASTE_GRACE_PERIOD);
            InsertionOutcome::DeliveryUncertain(UncertainInsertion::ClipboardPaste {
                transcript_bytes: transcript.len(),
                failure,
                restoration: restore_temporary_clipboard(clipboard, snapshot, transcript, &marker),
            })
        }
    }
}

fn stabilize_temporary_clipboard(
    clipboard: &mut impl ClipboardTransport,
    text: &str,
    marker: &TransactionMarker,
    sleep: &mut impl FnMut(Duration),
) -> Result<(), PrePasteFailure> {
    for check in 0..CLIPBOARD_SETTLE_CHECKS {
        if check > 0 {
            sleep(CLIPBOARD_SETTLE_INTERVAL);
        }

        match clipboard.temporary_ownership(text, marker) {
            Ok(TemporaryOwnership::Marker | TemporaryOwnership::Transcript) => {}
            Ok(TemporaryOwnership::Changed) => {
                return Err(PrePasteFailure::ClipboardChanged);
            }
            Err(failure) => return Err(PrePasteFailure::Verification(failure)),
        }
    }

    Ok(())
}

fn direct_fallback(
    direct: &mut impl DirectTyper,
    text: InsertionText<'_>,
    fallback_reason: PrePasteFailure,
    clipboard: DirectTypingClipboard,
) -> InsertionOutcome {
    match direct.type_text(text) {
        WtypeOutcome::Completed { input_bytes } => {
            InsertionOutcome::Completed(CompletedInsertion::DirectTyping {
                input_bytes,
                fallback_reason,
                clipboard,
            })
        }
        WtypeOutcome::NotStarted(failure) => {
            InsertionOutcome::NotInserted(InsertionFailure::DirectFallbackUnavailable {
                fallback_reason,
                failure,
            })
        }
        WtypeOutcome::DeliveryUncertain {
            maybe_input_bytes,
            failure,
        } => InsertionOutcome::DeliveryUncertain(UncertainInsertion::DirectTyping {
            maybe_input_bytes,
            fallback_reason,
            failure,
            clipboard,
        }),
    }
}

fn restore_temporary_clipboard(
    clipboard: &mut impl ClipboardTransport,
    snapshot: ClipboardSnapshot,
    text: &str,
    marker: &TransactionMarker,
) -> ClipboardRestoration {
    match clipboard.temporary_ownership(text, marker) {
        Ok(TemporaryOwnership::Marker | TemporaryOwnership::Transcript) => {
            match clipboard.restore(snapshot) {
                Ok(()) => ClipboardRestoration::Restored,
                Err(failure) => ClipboardRestoration::Failed(failure),
            }
        }
        Ok(TemporaryOwnership::Changed) => ClipboardRestoration::SkippedNewerClipboard,
        Err(failure) => ClipboardRestoration::Failed(failure),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn text() -> InsertionText<'static> {
        InsertionText::new("hello").expect("fixture should be non-empty")
    }

    #[derive(Debug)]
    struct FakeClipboard {
        snapshot: Result<ClipboardSnapshot, ClipboardTransactionFailure>,
        snapshot_is_current: Result<bool, ClipboardTransactionFailure>,
        publish: Result<(), ClipboardTransactionFailure>,
        publish_attempts: usize,
        ownership: VecDeque<Result<TemporaryOwnership, ClipboardTransactionFailure>>,
        restore: Result<(), ClipboardTransactionFailure>,
        restore_attempts: usize,
    }

    impl FakeClipboard {
        fn stable() -> Self {
            Self {
                snapshot: Ok(ClipboardSnapshot::empty()),
                snapshot_is_current: Ok(true),
                publish: Ok(()),
                publish_attempts: 0,
                ownership: VecDeque::from([
                    Ok(TemporaryOwnership::Marker),
                    Ok(TemporaryOwnership::Marker),
                    Ok(TemporaryOwnership::Marker),
                    Ok(TemporaryOwnership::Marker),
                    Ok(TemporaryOwnership::Marker),
                ]),
                restore: Ok(()),
                restore_attempts: 0,
            }
        }
    }

    impl ClipboardTransport for FakeClipboard {
        fn snapshot(&mut self) -> Result<ClipboardSnapshot, ClipboardTransactionFailure> {
            self.snapshot.clone()
        }

        fn snapshot_is_current(
            &mut self,
            _snapshot: &ClipboardSnapshot,
        ) -> Result<bool, ClipboardTransactionFailure> {
            self.snapshot_is_current.clone()
        }

        fn publish(
            &mut self,
            _text: &str,
            _marker: &TransactionMarker,
        ) -> Result<(), ClipboardTransactionFailure> {
            self.publish_attempts += 1;
            self.publish.clone()
        }

        fn temporary_ownership(
            &mut self,
            _text: &str,
            _marker: &TransactionMarker,
        ) -> Result<TemporaryOwnership, ClipboardTransactionFailure> {
            self.ownership
                .pop_front()
                .expect("fixture should provide every ownership result")
        }

        fn restore(
            &mut self,
            _snapshot: ClipboardSnapshot,
        ) -> Result<(), ClipboardTransactionFailure> {
            self.restore_attempts += 1;
            self.restore.clone()
        }
    }

    #[derive(Debug)]
    struct FakePasteChord {
        outcome: ClipboardPasteChordOutcome,
        attempts: usize,
    }

    impl FakePasteChord {
        fn new(outcome: ClipboardPasteChordOutcome) -> Self {
            Self {
                outcome,
                attempts: 0,
            }
        }
    }

    impl ClipboardPasteChord for FakePasteChord {
        fn send_clipboard_paste_chord(&mut self) -> ClipboardPasteChordOutcome {
            self.attempts += 1;
            self.outcome.clone()
        }
    }

    #[derive(Debug)]
    struct FakeDirect {
        outcome: WtypeOutcome,
        attempts: usize,
    }

    impl FakeDirect {
        fn completed() -> Self {
            Self {
                outcome: WtypeOutcome::Completed { input_bytes: 5 },
                attempts: 0,
            }
        }
    }

    impl DirectTyper for FakeDirect {
        fn type_text(&mut self, _text: InsertionText<'_>) -> WtypeOutcome {
            self.attempts += 1;
            self.outcome.clone()
        }
    }

    fn spawn_failure() -> WtypeFailure {
        WtypeFailure::Spawn {
            kind: io::ErrorKind::NotFound,
            message: "wtype not found".to_owned(),
        }
    }

    #[test]
    fn stable_transaction_settles_before_one_paste_chord_then_restores() {
        let mut clipboard = FakeClipboard::stable();
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect::completed();
        let mut sleeps = Vec::new();

        let outcome = insert_transaction(
            &mut clipboard,
            &mut chord,
            &mut direct,
            text(),
            |duration| sleeps.push(duration),
        );

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::ClipboardPaste {
                transcript_bytes: 5,
                restoration: ClipboardRestoration::Restored,
            })
        );
        assert_eq!(clipboard.restore_attempts, 1);
        assert_eq!(chord.attempts, 1);
        assert_eq!(direct.attempts, 0);
        assert_eq!(
            sleeps,
            vec![
                CLIPBOARD_SETTLE_INTERVAL,
                CLIPBOARD_SETTLE_INTERVAL,
                CLIPBOARD_SETTLE_INTERVAL,
                PASTE_GRACE_PERIOD,
            ]
        );
    }

    #[test]
    fn marker_stripping_uses_exact_transcript_identity() {
        let mut clipboard = FakeClipboard::stable();
        clipboard.ownership = VecDeque::from([
            Ok(TemporaryOwnership::Transcript),
            Ok(TemporaryOwnership::Transcript),
            Ok(TemporaryOwnership::Transcript),
            Ok(TemporaryOwnership::Transcript),
            Ok(TemporaryOwnership::Transcript),
        ]);
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect::completed();

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert!(matches!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::ClipboardPaste {
                restoration: ClipboardRestoration::Restored,
                ..
            })
        ));
        assert_eq!(chord.attempts, 1);
        assert_eq!(direct.attempts, 0);
    }

    #[test]
    fn snapshot_change_before_publication_direct_types_without_publishing() {
        let mut clipboard = FakeClipboard::stable();
        clipboard.snapshot_is_current = Ok(false);
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect::completed();

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::DirectTyping {
                input_bytes: 5,
                fallback_reason: PrePasteFailure::ClipboardChangedBeforePublication,
                clipboard: DirectTypingClipboard::NotPublished,
            })
        );
        assert_eq!(clipboard.publish_attempts, 0);
        assert_eq!(clipboard.restore_attempts, 0);
        assert_eq!(chord.attempts, 0);
        assert_eq!(direct.attempts, 1);
    }

    #[test]
    fn ownership_change_during_stabilization_direct_types_without_paste_or_restore() {
        let mut clipboard = FakeClipboard::stable();
        clipboard.ownership = VecDeque::from([
            Ok(TemporaryOwnership::Marker),
            Ok(TemporaryOwnership::Changed),
            Ok(TemporaryOwnership::Changed),
        ]);
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect::completed();
        let mut sleeps = Vec::new();

        let outcome = insert_transaction(
            &mut clipboard,
            &mut chord,
            &mut direct,
            text(),
            |duration| sleeps.push(duration),
        );

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::DirectTyping {
                input_bytes: 5,
                fallback_reason: PrePasteFailure::ClipboardChanged,
                clipboard: DirectTypingClipboard::Published {
                    restoration: ClipboardRestoration::SkippedNewerClipboard,
                },
            })
        );
        assert_eq!(clipboard.restore_attempts, 0);
        assert_eq!(chord.attempts, 0);
        assert_eq!(direct.attempts, 1);
        assert_eq!(sleeps, vec![CLIPBOARD_SETTLE_INTERVAL]);
    }

    #[test]
    fn stabilization_error_restores_owned_clipboard_before_direct_typing() {
        let failure = ClipboardTransactionFailure::TransferTimedOut {
            operation: ClipboardOperation::VerifyTranscript,
        };
        let mut clipboard = FakeClipboard::stable();
        clipboard.ownership = VecDeque::from([
            Ok(TemporaryOwnership::Marker),
            Err(failure.clone()),
            Ok(TemporaryOwnership::Transcript),
        ]);
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect::completed();

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::DirectTyping {
                input_bytes: 5,
                fallback_reason: PrePasteFailure::Verification(failure),
                clipboard: DirectTypingClipboard::Published {
                    restoration: ClipboardRestoration::Restored,
                },
            })
        );
        assert_eq!(clipboard.restore_attempts, 1);
        assert_eq!(chord.attempts, 0);
        assert_eq!(direct.attempts, 1);
    }

    #[test]
    fn restoration_ownership_error_is_preserved_when_direct_typing_completes() {
        let setup_failure = ClipboardTransactionFailure::TransferTimedOut {
            operation: ClipboardOperation::VerifyTranscript,
        };
        let restoration_failure = ClipboardTransactionFailure::TransferTimedOut {
            operation: ClipboardOperation::VerifyMarker,
        };
        let mut clipboard = FakeClipboard::stable();
        clipboard.ownership = VecDeque::from([
            Ok(TemporaryOwnership::Marker),
            Err(setup_failure.clone()),
            Err(restoration_failure.clone()),
        ]);
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect::completed();

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::DirectTyping {
                input_bytes: 5,
                fallback_reason: PrePasteFailure::Verification(setup_failure),
                clipboard: DirectTypingClipboard::Published {
                    restoration: ClipboardRestoration::Failed(restoration_failure),
                },
            })
        );
        assert_eq!(clipboard.restore_attempts, 0);
        assert_eq!(chord.attempts, 0);
        assert_eq!(direct.attempts, 1);
    }

    #[test]
    fn restore_error_is_preserved_when_direct_typing_is_uncertain() {
        let restoration_failure = ClipboardTransactionFailure::Access {
            operation: ClipboardOperation::RestoreSnapshot,
            kind: ClipboardFailureKind::WaylandCommunication,
        };
        let mut clipboard = FakeClipboard::stable();
        clipboard.restore = Err(restoration_failure.clone());
        let chord_failure = spawn_failure();
        let mut chord =
            FakePasteChord::new(ClipboardPasteChordOutcome::NotSent(chord_failure.clone()));
        let mut direct = FakeDirect {
            outcome: WtypeOutcome::DeliveryUncertain {
                maybe_input_bytes: 2,
                failure: WtypeFailure::TimedOut,
            },
            attempts: 0,
        };

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::DeliveryUncertain(UncertainInsertion::DirectTyping {
                maybe_input_bytes: 2,
                fallback_reason: PrePasteFailure::PasteChordUnavailable(chord_failure),
                failure: WtypeFailure::TimedOut,
                clipboard: DirectTypingClipboard::Published {
                    restoration: ClipboardRestoration::Failed(restoration_failure),
                },
            })
        );
        assert_eq!(clipboard.restore_attempts, 1);
        assert_eq!(chord.attempts, 1);
        assert_eq!(direct.attempts, 1);
    }

    #[test]
    fn ownership_change_observed_before_restore_skips_restoration() {
        let mut clipboard = FakeClipboard::stable();
        clipboard.ownership = VecDeque::from([
            Ok(TemporaryOwnership::Marker),
            Ok(TemporaryOwnership::Marker),
            Ok(TemporaryOwnership::Marker),
            Ok(TemporaryOwnership::Marker),
            Ok(TemporaryOwnership::Changed),
        ]);
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect::completed();

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::ClipboardPaste {
                transcript_bytes: 5,
                restoration: ClipboardRestoration::SkippedNewerClipboard,
            })
        );
        assert_eq!(clipboard.restore_attempts, 0);
        assert_eq!(chord.attempts, 1);
        assert_eq!(direct.attempts, 0);
    }

    #[test]
    fn chord_uncertainty_never_direct_types_or_retries() {
        let mut clipboard = FakeClipboard::stable();
        let failure = WtypeFailure::TimedOut;
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::DeliveryUncertain(
            failure.clone(),
        ));
        let mut direct = FakeDirect::completed();

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::DeliveryUncertain(UncertainInsertion::ClipboardPaste {
                transcript_bytes: 5,
                failure,
                restoration: ClipboardRestoration::Restored,
            })
        );
        assert_eq!(chord.attempts, 1);
        assert_eq!(direct.attempts, 0);
        assert_eq!(clipboard.restore_attempts, 1);
    }

    #[test]
    fn chord_spawn_failure_restores_then_direct_types() {
        let mut clipboard = FakeClipboard::stable();
        let failure = spawn_failure();
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::NotSent(failure.clone()));
        let mut direct = FakeDirect::completed();

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::DirectTyping {
                input_bytes: 5,
                fallback_reason: PrePasteFailure::PasteChordUnavailable(failure),
                clipboard: DirectTypingClipboard::Published {
                    restoration: ClipboardRestoration::Restored,
                },
            })
        );
        assert_eq!(clipboard.restore_attempts, 1);
        assert_eq!(chord.attempts, 1);
        assert_eq!(direct.attempts, 1);
    }

    #[test]
    fn direct_fallback_spawn_failure_is_terminal() {
        let snapshot_failure = ClipboardTransactionFailure::ChangedDuringSnapshot;
        let mut clipboard = FakeClipboard::stable();
        clipboard.snapshot = Err(snapshot_failure.clone());
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect {
            outcome: WtypeOutcome::NotStarted(spawn_failure()),
            attempts: 0,
        };

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::NotInserted(InsertionFailure::DirectFallbackUnavailable {
                fallback_reason: PrePasteFailure::Snapshot(snapshot_failure),
                failure: spawn_failure(),
            })
        );
        assert_eq!(chord.attempts, 0);
        assert_eq!(direct.attempts, 1);
    }

    #[test]
    fn direct_typing_uncertainty_is_terminal() {
        let snapshot_failure = ClipboardTransactionFailure::ChangedDuringSnapshot;
        let mut clipboard = FakeClipboard::stable();
        clipboard.snapshot = Err(snapshot_failure.clone());
        let mut chord = FakePasteChord::new(ClipboardPasteChordOutcome::Sent);
        let mut direct = FakeDirect {
            outcome: WtypeOutcome::DeliveryUncertain {
                maybe_input_bytes: 2,
                failure: WtypeFailure::TimedOut,
            },
            attempts: 0,
        };

        let outcome = insert_transaction(&mut clipboard, &mut chord, &mut direct, text(), |_| {});

        assert_eq!(
            outcome,
            InsertionOutcome::DeliveryUncertain(UncertainInsertion::DirectTyping {
                maybe_input_bytes: 2,
                fallback_reason: PrePasteFailure::Snapshot(snapshot_failure),
                failure: WtypeFailure::TimedOut,
                clipboard: DirectTypingClipboard::NotPublished,
            })
        );
        assert_eq!(direct.attempts, 1);
    }
}
