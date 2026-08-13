# Conversation session ownership and retention

## Decision

Conversation review is an explicit session, separate from ordinary dictation. Starting a conversation tells the user that Dictate will keep local audio and a transcript until the session is deleted. Ordinary dictation keeps its current insert-and-discard behavior.

A session owns its recording, raw transcript, anonymous speaker analysis, and session-only labels. Speaker identity and device-level voice profiles are outside this session boundary; see `memo-remembered-voice-ownership.md`.

## State model

A conversation session moves through these states:

1. `Recording`: Dictate is writing one local audio stream after an explicit start action.
2. `Analyzing`: recording has stopped; the immutable audio is available for ASR and anonymous diarization.
3. `Ready`: raw transcript and the latest analysis are available for review.
4. `AnalysisFailed`: audio remains available, the failure is visible, and analysis can be retried.
5. `Interrupted`: capture ended without a normal stop, such as after a crash. The partial audio can be reviewed, retried, or deleted.
6. Deleted: all session-owned files are removed. This is a terminal action rather than a stored state.

Recording never waits for diarization. Analysis never edits the source audio. A failed or uncertain analysis leaves the recording and raw transcript intact.

## Ownership

`dictate` owns the session lifecycle, local paths, atomic metadata updates, and retention actions. `dictate-speech` accepts a captured utterance and returns a raw transcript plus anonymous speaker analysis. It has no file-retention policy. `dictate-ui` presents review, session-speaker rename, retry, export, and delete actions. `dictate-dev` keeps model and alignment diagnostics.

The daemon is the sole writer for a live session. Review clients send typed commands to it. A session ID is an opaque random value; filenames, labels, and timestamps do not act as identity.

## Local layout

Sessions live under the platform data directory in a private directory:

```text
sessions/
  <session-id>/
    session.json
    audio.wav
    transcript.json
```

`session.json` records the state, creation and stop times, audio duration, analysis model identities, and session-speaker labels. `transcript.json` stores the raw text, every timed token, anonymous segments, and attribution. Model vectors do not belong in this file.

The directory is created with user-only permissions before audio is written. Metadata writes use a temporary sibling and atomic rename. Audio is append-only while recording and immutable after stop. A crash may leave a valid partial WAV and an `Interrupted` state; recovery must not guess that the session completed.

## Retention

Starting a conversation states that local audio will be kept. The default is to keep the session until the user deletes it. Dictate shows the retained-audio status and session size in review and settings. It does not create a hidden rolling history, copy ordinary dictations into sessions, or upload sessions.

Users can:

- delete the whole session;
- delete retained audio after accepting the transcript, while keeping the transcript and labels;
- export audio or the attributed transcript;
- rerun analysis while audio and compatible models remain available.

Deleting audio disables re-analysis. A session delete concerns only session-owned files and labels; device-level identity data follows its separate ownership contract.

Automatic expiry is deferred. Adding it later requires an explicit setting and a visible next-deletion time. Dictate must not invent expiry behavior that can erase a conversation without notice.

## Speaker labels

Anonymous IDs are scoped to one analysis revision. A label attaches to the session's speaker record, not directly to a numeric cluster ID. Re-analysis proposes a mapping from old to new segments and asks for confirmation when the mapping is uncertain. It never silently moves a person's label to a different voice.

Renaming `Speaker 2` to `Josh` changes only that session. It never creates or updates device-level identity data.

## Headless contract

Every production review action needs a machine-readable equivalent with meaningful exit codes. The intended command families are:

```text
dictate conversation start
dictate conversation stop <session-id>
dictate conversation list --json
dictate conversation show <session-id> --json
dictate conversation analyze <session-id> --json
dictate conversation label <session-id> <speaker-id> <label>
dictate conversation export <session-id> --format <audio|json|text>
dictate conversation delete-audio <session-id>
dictate conversation delete <session-id>
```

These names record the session boundary; they are not an instruction to add all commands in one change. The first production slice should create, stop, show, and delete a session. Labeling follows only after re-analysis mapping has tests. Device-level voice commands belong to `memo-remembered-voice-ownership.md`.

## Failure rules

- Capture failure leaves no `Ready` session and never substitutes another pinned microphone.
- ASR failure keeps audio and records an analysis error.
- Missing or invalid timestamps keeps the raw transcript but omits attribution.
- Diarization failure keeps the raw transcript and audio.
- Unknown, ambiguous, and overlapping tokens remain in the transcript.
- Delete reports any file it could not remove and leaves the session visible until cleanup succeeds.
- No cleanup relies only on `Drop` or process termination.

## Gates before implementation

1. Review the state model and retention copy in a production UI sketch.
2. Define the session file schema and crash-recovery tests before writing live audio.
3. Measure long-session disk growth and analysis time.
4. Clear model redistribution and supported-hardware CPU gates.
5. Keep ordinary `dictate transcribe <wav>` and immediate dictation outside the session store.
