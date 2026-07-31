use std::io;
use std::os::fd::AsFd as _;
use std::time::Duration;
use std::time::Instant;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::event::poll;
use rustix::fs::OFlags;
use rustix::fs::fcntl_getfl;
use rustix::fs::fcntl_setfl;
use rustix::io::Errno;
use wl_clipboard_rs::copy;
use wl_clipboard_rs::copy::MimeSource;
use wl_clipboard_rs::copy::MimeType as CopyMimeType;
use wl_clipboard_rs::copy::Source;
use wl_clipboard_rs::paste;
use wl_clipboard_rs::paste::MimeType as PasteMimeType;

use super::ClipboardFailureKind;
use super::ClipboardOperation;
use super::ClipboardTransactionFailure;
use super::ClipboardTransport;
use super::TemporaryOwnership;
use super::TransactionMarker;

const TEXT_MIME: &str = "text/plain;charset=utf-8";
const MARKER_MIME: &str = "application/x-dictate-clipboard-transaction";
const MAX_MIME_TYPES: usize = 64;
const MAX_MIME_METADATA_BYTES: usize = 64 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(1_200);
const TRANSFER_TIMEOUT: Duration = Duration::from_millis(350);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClipboardSnapshot {
    Empty,
    Contents(Vec<MimeRepresentation>),
}

impl ClipboardSnapshot {
    #[cfg(test)]
    pub(super) fn empty() -> Self {
        Self::Empty
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MimeRepresentation {
    mime_type: String,
    data: Box<[u8]>,
}

#[derive(Debug, Default)]
pub(super) struct WaylandClipboard;

impl ClipboardTransport for WaylandClipboard {
    fn snapshot(&mut self) -> Result<ClipboardSnapshot, ClipboardTransactionFailure> {
        capture_snapshot()
    }

    fn snapshot_is_current(
        &mut self,
        snapshot: &ClipboardSnapshot,
    ) -> Result<bool, ClipboardTransactionFailure> {
        snapshot_matches_current(
            snapshot,
            ClipboardOperation::RevalidateSnapshot,
            Instant::now() + SNAPSHOT_TIMEOUT,
        )
    }

    fn publish(
        &mut self,
        text: &str,
        marker: &TransactionMarker,
    ) -> Result<(), ClipboardTransactionFailure> {
        let sources = vec![
            mime_source(MARKER_MIME, marker.as_bytes()),
            mime_source(TEXT_MIME, text.as_bytes()),
        ];
        let mut options = copy_options();
        options.omit_additional_text_mime_types(false);
        options
            .copy_multi(sources)
            .map_err(|error| copy_failure(ClipboardOperation::PublishTranscript, &error))
    }

    fn temporary_ownership(
        &mut self,
        text: &str,
        marker: &TransactionMarker,
    ) -> Result<TemporaryOwnership, ClipboardTransactionFailure> {
        let marker_matches = matches!(
            read_identity(MARKER_MIME, ClipboardOperation::VerifyMarker)?,
            Some(contents) if contents == marker.as_bytes()
        );
        let transcript_matches = matches!(
            read_identity(TEXT_MIME, ClipboardOperation::VerifyTranscript)?,
            Some(contents) if contents == text.as_bytes()
        );

        match (marker_matches, transcript_matches) {
            (true, true) => Ok(TemporaryOwnership::Marker),
            (false, true) => Ok(TemporaryOwnership::Transcript),
            (true | false, false) => Ok(TemporaryOwnership::Changed),
        }
    }

    fn restore(&mut self, snapshot: ClipboardSnapshot) -> Result<(), ClipboardTransactionFailure> {
        match snapshot {
            ClipboardSnapshot::Empty => copy::clear(copy::ClipboardType::Regular, copy::Seat::All)
                .map_err(|error| copy_failure(ClipboardOperation::RestoreSnapshot, &error)),
            ClipboardSnapshot::Contents(representations) => {
                let sources = representations
                    .into_iter()
                    .map(|representation| MimeSource {
                        source: Source::Bytes(representation.data),
                        mime_type: CopyMimeType::Specific(representation.mime_type),
                    })
                    .collect();
                let mut options = copy_options();
                options.omit_additional_text_mime_types(true);
                options
                    .copy_multi(sources)
                    .map_err(|error| copy_failure(ClipboardOperation::RestoreSnapshot, &error))
            }
        }
    }
}

fn capture_snapshot() -> Result<ClipboardSnapshot, ClipboardTransactionFailure> {
    let deadline = Instant::now() + SNAPSHOT_TIMEOUT;
    let advertised = match list_mime_types(ClipboardOperation::ListMimeTypes) {
        Ok(mime_types) => mime_types,
        Err(ClipboardTransactionFailure::Access {
            kind: ClipboardFailureKind::Empty,
            ..
        }) => {
            let snapshot = ClipboardSnapshot::Empty;
            return if snapshot_matches_current(
                &snapshot,
                ClipboardOperation::ConfirmSnapshot,
                deadline,
            )? {
                Ok(snapshot)
            } else {
                Err(ClipboardTransactionFailure::ChangedDuringSnapshot)
            };
        }
        Err(failure) => return Err(failure),
    };

    validate_mime_metadata(&advertised)?;
    let advertised = distinct_mime_types(advertised);
    let mut representations = Vec::with_capacity(advertised.len());
    let mut total_bytes = 0_usize;
    for mime_type in advertised {
        let remaining = MAX_SNAPSHOT_BYTES.saturating_sub(total_bytes);
        let data = read_specific(
            &mime_type,
            remaining,
            ClipboardOperation::ReadSnapshot,
            deadline,
        )?
        .ok_or(ClipboardTransactionFailure::AdvertisedMimeUnavailable)?;
        total_bytes = total_bytes.saturating_add(data.len());
        representations.push(MimeRepresentation {
            mime_type,
            data: data.into_boxed_slice(),
        });
    }

    let snapshot = ClipboardSnapshot::Contents(representations);
    if snapshot_matches_current(&snapshot, ClipboardOperation::ConfirmSnapshot, deadline)? {
        Ok(snapshot)
    } else {
        Err(ClipboardTransactionFailure::ChangedDuringSnapshot)
    }
}

fn snapshot_matches_current(
    snapshot: &ClipboardSnapshot,
    operation: ClipboardOperation,
    deadline: Instant,
) -> Result<bool, ClipboardTransactionFailure> {
    let current_mime_types = match list_mime_types(operation) {
        Ok(mime_types) => mime_types,
        Err(ClipboardTransactionFailure::Access {
            kind: ClipboardFailureKind::Empty,
            ..
        }) => return Ok(matches!(snapshot, ClipboardSnapshot::Empty)),
        Err(failure) => return Err(failure),
    };

    let ClipboardSnapshot::Contents(representations) = snapshot else {
        return Ok(false);
    };
    validate_mime_metadata(&current_mime_types)?;
    let current_mime_types = distinct_mime_types(current_mime_types);
    if !current_mime_types.iter().eq(representations
        .iter()
        .map(|representation| &representation.mime_type))
    {
        return Ok(false);
    }

    confirm_representations(representations, |representation| {
        read_specific(
            &representation.mime_type,
            representation.data.len().saturating_add(1),
            operation,
            deadline,
        )
    })
}

fn confirm_representations(
    representations: &[MimeRepresentation],
    mut read: impl FnMut(&MimeRepresentation) -> Result<Option<Vec<u8>>, ClipboardTransactionFailure>,
) -> Result<bool, ClipboardTransactionFailure> {
    for representation in representations {
        let confirmation = match read(representation) {
            Ok(Some(confirmation)) => confirmation,
            Ok(None) | Err(ClipboardTransactionFailure::SnapshotTooLarge { .. }) => {
                return Ok(false);
            }
            Err(failure) => return Err(failure),
        };
        if confirmation.as_slice() != representation.data.as_ref() {
            return Ok(false);
        }
    }

    Ok(true)
}

fn distinct_mime_types(mime_types: Vec<String>) -> Vec<String> {
    let mut distinct = Vec::with_capacity(mime_types.len());
    for mime_type in mime_types {
        if !distinct.contains(&mime_type) {
            distinct.push(mime_type);
        }
    }
    distinct
}

fn validate_mime_metadata(mime_types: &[String]) -> Result<(), ClipboardTransactionFailure> {
    if mime_types.len() > MAX_MIME_TYPES {
        return Err(ClipboardTransactionFailure::TooManyMimeTypes {
            count: mime_types.len(),
            limit: MAX_MIME_TYPES,
        });
    }
    let metadata_bytes = mime_types.iter().fold(0_usize, |total, mime_type| {
        total.saturating_add(mime_type.len())
    });
    if metadata_bytes > MAX_MIME_METADATA_BYTES {
        return Err(ClipboardTransactionFailure::MimeMetadataTooLarge {
            limit: MAX_MIME_METADATA_BYTES,
        });
    }
    Ok(())
}

fn list_mime_types(
    operation: ClipboardOperation,
) -> Result<Vec<String>, ClipboardTransactionFailure> {
    paste::get_mime_types_ordered(paste::ClipboardType::Regular, paste::Seat::Unspecified)
        .map_err(|error| paste_failure(operation, &error))
}

fn read_identity(
    mime_type: &str,
    operation: ClipboardOperation,
) -> Result<Option<Vec<u8>>, ClipboardTransactionFailure> {
    let deadline = Instant::now() + TRANSFER_TIMEOUT;
    read_specific(mime_type, MAX_SNAPSHOT_BYTES, operation, deadline)
}

fn read_specific(
    mime_type: &str,
    limit: usize,
    operation: ClipboardOperation,
    overall_deadline: Instant,
) -> Result<Option<Vec<u8>>, ClipboardTransactionFailure> {
    ensure_before_deadline(overall_deadline, operation)?;
    let transfer_deadline = (Instant::now() + TRANSFER_TIMEOUT).min(overall_deadline);
    let (mut reader, actual_mime) = match paste::get_contents(
        paste::ClipboardType::Regular,
        paste::Seat::Unspecified,
        PasteMimeType::Specific(mime_type),
    ) {
        Ok(contents) => contents,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
        Err(error) => return Err(paste_failure(operation, &error)),
    };
    if actual_mime != mime_type {
        return Ok(None);
    }

    make_nonblocking(&reader).map_err(|error| ClipboardTransactionFailure::Access {
        operation,
        kind: ClipboardFailureKind::DataTransfer(error.kind()),
    })?;
    read_bounded(&mut reader, limit, transfer_deadline, operation).map(Some)
}

fn read_bounded(
    reader: &mut (impl io::Read + std::os::fd::AsFd),
    limit: usize,
    deadline: Instant,
    operation: ClipboardOperation,
) -> Result<Vec<u8>, ClipboardTransactionFailure> {
    let mut contents = Vec::new();
    let mut buffer = [0_u8; 8192];

    loop {
        ensure_before_deadline(deadline, operation)?;
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(contents),
            Ok(read) => {
                if contents.len().saturating_add(read) > limit {
                    return Err(ClipboardTransactionFailure::SnapshotTooLarge { limit });
                }
                contents.extend_from_slice(&buffer[..read]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_readable(reader.as_fd(), deadline, operation)?;
            }
            Err(error) => {
                return Err(ClipboardTransactionFailure::Access {
                    operation,
                    kind: ClipboardFailureKind::DataTransfer(error.kind()),
                });
            }
        }
    }
}

fn ensure_before_deadline(
    deadline: Instant,
    operation: ClipboardOperation,
) -> Result<(), ClipboardTransactionFailure> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .map(|_remaining| ())
        .ok_or(ClipboardTransactionFailure::TransferTimedOut { operation })
}

fn wait_readable(
    fd: std::os::fd::BorrowedFd<'_>,
    deadline: Instant,
    operation: ClipboardOperation,
) -> Result<(), ClipboardTransactionFailure> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(ClipboardTransactionFailure::TransferTimedOut { operation })?;
        let timeout = Timespec::try_from(remaining).map_err(|_error| {
            ClipboardTransactionFailure::Access {
                operation,
                kind: ClipboardFailureKind::DataTransfer(io::ErrorKind::InvalidInput),
            }
        })?;
        let mut poll_fds = [PollFd::from_borrowed_fd(fd, PollFlags::IN)];
        match poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => return Err(ClipboardTransactionFailure::TransferTimedOut { operation }),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => {}
            Err(error) => {
                return Err(ClipboardTransactionFailure::Access {
                    operation,
                    kind: ClipboardFailureKind::DataTransfer(errno_to_io(error).kind()),
                });
            }
        }
    }
}

fn make_nonblocking(fd: &impl std::os::fd::AsFd) -> io::Result<()> {
    let flags = fcntl_getfl(fd).map_err(errno_to_io)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(errno_to_io)
}

fn copy_options() -> copy::Options {
    let mut options = copy::Options::new();
    options
        .clipboard(copy::ClipboardType::Regular)
        .seat(copy::Seat::All);
    options
}

fn mime_source(mime_type: &str, data: &[u8]) -> MimeSource {
    MimeSource {
        source: Source::Bytes(data.to_vec().into_boxed_slice()),
        mime_type: CopyMimeType::Specific(mime_type.to_owned()),
    }
}

fn paste_failure(
    operation: ClipboardOperation,
    error: &paste::Error,
) -> ClipboardTransactionFailure {
    let kind = match error {
        paste::Error::NoSeats => ClipboardFailureKind::NoSeats,
        paste::Error::ClipboardEmpty => ClipboardFailureKind::Empty,
        paste::Error::NoMimeType => ClipboardFailureKind::NoMimeType,
        paste::Error::SocketOpenError(error) => ClipboardFailureKind::SocketOpen(error.kind()),
        paste::Error::WaylandConnection(_) => ClipboardFailureKind::WaylandConnection,
        paste::Error::WaylandCommunication(_) => ClipboardFailureKind::WaylandCommunication,
        paste::Error::MissingProtocol { name, version } => ClipboardFailureKind::MissingProtocol {
            name: (*name).to_owned(),
            version: *version,
        },
        paste::Error::PrimarySelectionUnsupported => {
            ClipboardFailureKind::PrimarySelectionUnsupported
        }
        paste::Error::SeatNotFound => ClipboardFailureKind::SeatNotFound,
        paste::Error::PipeCreation(error) => ClipboardFailureKind::PipeCreation(error.kind()),
    };
    ClipboardTransactionFailure::Access { operation, kind }
}

fn copy_failure(operation: ClipboardOperation, error: &copy::Error) -> ClipboardTransactionFailure {
    let kind = match error {
        copy::Error::NoSeats => ClipboardFailureKind::NoSeats,
        copy::Error::SocketOpenError(error) => ClipboardFailureKind::SocketOpen(error.kind()),
        copy::Error::WaylandConnection(_) => ClipboardFailureKind::WaylandConnection,
        copy::Error::WaylandCommunication(_) => ClipboardFailureKind::WaylandCommunication,
        copy::Error::MissingProtocol { name, version } => ClipboardFailureKind::MissingProtocol {
            name: (*name).to_owned(),
            version: *version,
        },
        copy::Error::PrimarySelectionUnsupported => {
            ClipboardFailureKind::PrimarySelectionUnsupported
        }
        copy::Error::SeatNotFound => ClipboardFailureKind::SeatNotFound,
        copy::Error::TempCopy(error) => {
            ClipboardFailureKind::TemporaryStorage(source_creation_error_kind(error))
        }
        copy::Error::TempFileRemove(error) | copy::Error::TempDirRemove(error) => {
            ClipboardFailureKind::TemporaryStorage(error.kind())
        }
        copy::Error::Paste(
            copy::DataSourceError::FileOpen(error) | copy::DataSourceError::Copy(error),
        ) => ClipboardFailureKind::DataTransfer(error.kind()),
    };
    ClipboardTransactionFailure::Access { operation, kind }
}

fn source_creation_error_kind(error: &copy::SourceCreationError) -> io::ErrorKind {
    match error {
        copy::SourceCreationError::TempDirCreate(error)
        | copy::SourceCreationError::TempFileCreate(error)
        | copy::SourceCreationError::DataCopy(error)
        | copy::SourceCreationError::TempFileWrite(error)
        | copy::SourceCreationError::TempFileOpen(error)
        | copy::SourceCreationError::TempFileMetadata(error)
        | copy::SourceCreationError::TempFileSeek(error)
        | copy::SourceCreationError::TempFileRead(error)
        | copy::SourceCreationError::TempFileTruncate(error) => error.kind(),
    }
}

fn errno_to_io(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_checks_each_captured_representation() {
        let representations = vec![
            MimeRepresentation {
                mime_type: "text/plain".to_owned(),
                data: b"plain".to_vec().into_boxed_slice(),
            },
            MimeRepresentation {
                mime_type: "text/html".to_owned(),
                data: b"<p>plain</p>".to_vec().into_boxed_slice(),
            },
        ];
        let mut checked = Vec::new();

        let matches = confirm_representations(&representations, |representation| {
            checked.push(representation.mime_type.clone());
            let contents = if representation.mime_type == "text/html" {
                b"<p>changed</p>".to_vec()
            } else {
                representation.data.to_vec()
            };
            Ok(Some(contents))
        })
        .expect("controlled confirmation should not fail");

        assert!(!matches);
        assert_eq!(checked, vec!["text/plain", "text/html"]);
    }

    #[test]
    fn mime_metadata_rejects_too_many_representations() {
        let mime_types = (0..=MAX_MIME_TYPES)
            .map(|index| format!("application/x-test-{index}"))
            .collect::<Vec<_>>();

        assert_eq!(
            validate_mime_metadata(&mime_types),
            Err(ClipboardTransactionFailure::TooManyMimeTypes {
                count: MAX_MIME_TYPES + 1,
                limit: MAX_MIME_TYPES,
            })
        );
    }

    #[test]
    fn mime_metadata_rejects_oversized_names() {
        let mime_types = vec!["x".repeat(MAX_MIME_METADATA_BYTES + 1)];

        assert_eq!(
            validate_mime_metadata(&mime_types),
            Err(ClipboardTransactionFailure::MimeMetadataTooLarge {
                limit: MAX_MIME_METADATA_BYTES,
            })
        );
    }
}
