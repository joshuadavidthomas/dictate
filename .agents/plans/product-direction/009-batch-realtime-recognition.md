# Plan 009: Make Batch and Realtime separate recognition pipelines

> **Executor instructions**: Follow this plan step by step. Run each
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback;
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 256fb711b983 -- README.md crates/dictate/src/cli.rs crates/dictate/src/daemon.rs crates/dictate/src/settings.rs crates/dictate-dev/src/lib.rs crates/dictate-speech/src crates/dictate-speech/tests crates/dictate-ui/src`
> This plan describes working-copy change `szlnzqvn` at git snapshot
> `256fb711b983`, including the completed live-text surface and audio
> ducking work. Changes beyond the 008/009 plan-file and effort-index
> reconciliation edits made after that snapshot are drift. Compare the
> live code with "Current state" before proceeding; a material mismatch
> is a STOP condition.

## Status

- **Effort**: L
- **Risk**: HIGH (Realtime's live session becomes the only source of
  delivered text; sample ownership and stop ordering must be exact)
- **Depends on**: 007 live text surface; 008 audio ducking
- **Planned at**: git `256fb711b983` / change `szlnzqvn`, 2026-08-13

## Why this matters

Dictate currently shows text from Fast Conformer while recording, then
throws that work away and decodes the full recording with offline
Parakeet. The live card looks like a preview of the delivered text, but
it comes from a different recognizer and can disagree sharply.

Recognition will have two honest modes:

- **Batch** is the default. One non-streaming model decodes the complete
  recording after capture. It sends no live hypotheses and never opens
  the text card.
- **Realtime** uses one streaming model and one streaming session. That
  session emits live hypotheses and its finished result becomes the
  delivered transcript.

There is no mixed mode, second recognizer, rescore pass, or silent
fallback. Both modes run locally. "Batch" and "Realtime" describe the
recognition method; the existing `mode = "message" | "email" | ...`
continues to describe text formatting.

## Settled product and model decisions

- Config field: `recognition_mode = "batch" | "realtime"`.
- Default mode: `batch`.
- Batch default model: `parakeet-tdt-0.6b-v2-int8`.
- Realtime default model:
  `parakeet-unified-0.6b-int8-streaming-560ms`.
- `model` remains the optional model override. Its default depends on
  `recognition_mode`.
- Remove `partials_model`; do not keep an alias or migration parser.
- Rename `partials_font_family` / `partials_font_size` to
  `realtime_font_family` / `realtime_font_size`; do not accept both.
- Batch rejects streaming models. Realtime rejects non-streaming models.
- Realtime formats each displayed hypothesis with the same
  `DictationContext` used for delivery. The recognizer may still revise
  words while speech is live, but stopping must not swap in text from a
  different model or formatting path.

Example settings:

```toml
# Omit this field for Batch.
recognition_mode = "realtime"

# Optional; omitted Realtime model resolves to Parakeet Unified.
model = "parakeet-unified-0.6b-int8-streaming-560ms"

realtime_font_family = "Inter"
realtime_font_size = 14

# Existing formatter mode; unrelated to recognition_mode.
mode = "technical"
delivery = "insert"
```

## Current state

These references describe the live working tree at the planned snapshot.
Line numbers may shift after formatting; use the named symbols.

### Settings and defaults

- `crates/dictate/src/settings.rs:46-65` stores `model: String`,
  `partials_model: Option<String>`, `partials_font_family`, and
  `partials_font_size` as separate top-level fields.
- `Settings::partials_model` at `settings.rs:75-97` resolves the preview
  model and rejects non-streaming entries.
- `Settings::transcription_plan` at `settings.rs:115-130` resolves the
  final model independently.
- `Settings::default` at `settings.rs:192-208` fixes one default final
  model before any recognition mode exists.
- `crates/dictate-speech/src/models.rs:129-169` exposes
  `is_streaming`, `default_model`, and `default_partials_model`. The
  latter chooses Fast Conformer for speed rather than final accuracy.
- `crates/dictate-speech/src/models.rs:388-400` contains both streaming
  catalog entries. Parakeet Unified is the Realtime default chosen by
  this plan; Fast Conformer remains an explicit Realtime option.

### Two recognizers and two finalization paths

- `daemon::run` at `crates/dictate/src/daemon.rs:172-201` creates a
  `TranscriptionPlan` for the final model and resolves
  `partials_model` separately.
- `Daemon`, `Daemon::start`, and `spawn_microphone_worker` carry both
  model choices through `daemon.rs:379-416` and `531-560`.
- `initialize_worker_recognizers` at `daemon.rs:769-787` constructs two
  recognizers at worker startup.
- `StreamingState` at `daemon.rs:764-767` holds a session borrowed from
  the preview recognizer plus an unbounded sample receiver.
- `feed_streaming_batches` at `daemon.rs:790-802` feeds that session and
  forwards each changed raw hypothesis to the overlay.
- `finalize_ready_dictation` at `daemon.rs:804-833` calls
  `transcribe` with the other recognizer and the complete utterance.
  The preview session is discarded without `finish()`.

### Speech API

- `crates/dictate-speech/src/transcription.rs:16-34` defines a
  `TranscriptionPlan` with a model and formatting context but no
  recognition mode.
- `Recognizer` contains either `OfflineRecognizer` or
  `OnlineRecognizer` at `transcription.rs:36-82`.
- `Recognizer::streaming_session` returns `Option` because callers can
  ask an offline recognizer for a streaming session.
- `StreamingSession::feed` and `finish` at `transcription.rs:99-136`
  already encode the right Realtime lifecycle. `finish` is documented
  as authoritative, but the daemon never calls it.
- `Recognizer::decode` sends online models through
  `decode_simulated_streaming`, creating a second online stream and
  replaying the completed utterance in 4,096-sample chunks
  (`transcription.rs:145-184`).
- `classify_transcript` at `transcription.rs:256-290` applies the common
  short/empty/noise rules after recognition.
- `crates/dictate-speech/src/lib.rs:29-55` publicly exports the mixed
  pipeline's defaults and `StreamingSession`.

### Sample ownership hazard

- The `CaptureHandler::samples` implementation for
  `DictationCaptureHandler` in `crates/dictate/src/daemon.rs:562-608`
  copies every microphone batch
  into the live channel before `DictationControl::record_samples`
  reports whether that recording accepted the samples.
- `DictationControl::record_samples` at
  `crates/dictate-speech/src/dictation.rs:378-436` may ignore stale
  batches or accept only a prefix at the ten-minute cap, but
  `RecordSamplesUpdate` does not report the accepted count.
- This ordering was tolerable while live text was cosmetic. It is wrong
  when the live session supplies the final transcript: the recognizer
  and `ReadyDictation::utterance` must receive the same samples.
- `Mic::drop` at `crates/dictate-speech/src/mic.rs:83-90` stops the
  stream and joins the audio worker. The worker drains its ring and
  flushes the resampler before exit (`mic.rs:473-545`). This is the
  ordering seam for a complete Realtime finish.

### UI and formatter

- `Overlay::send_partial`, `OverlayMessage::Partial`, and the
  Recording-only apply rule live in
  `crates/dictate-ui/src/app.rs:55-147`. Show/Hide revision rules prevent
  a late hypothesis from reopening the card.
- The partial window remains absent until accepted text exists
  (`app.rs:275-302`). Batch can therefore hide live text by sending no
  hypotheses; the UI does not need to know the speech mode.
- The working tree already uses GPUI width-aware wrapping, a four-line
  scroll viewport, a passive scrollbar, and configurable text style in
  `crates/dictate-ui/src/partial.rs` and
  `components/partial_card.rs`. Preserve that behavior.
- `DictationFormatter` processes the final transcript in
  `daemon.rs:884-905`. Realtime hypotheses currently bypass it.

### Unrelated in-flight behavior to preserve

- Completed Plan 008 audio ducking holds a `DuckGuard` for the live
  microphone span in `daemon.rs`. Socket ownership is acquired before
  startup recovery. Recognition refactoring must preserve restore on
  stop, cancel, empty capture, stream error, and worker exit.
- The libpulse connection field order in
  `crates/dictate-desktop/src/audio_ducking.rs` is deliberate: the Pulse
  context must drop before its main loop. This plan does not edit that
  module.

## Commands you will need

Use targeted checks while implementing. Run broad gates once at the end.
Do not run `cargo clean`. Run Hawk once, after normal checks pass; its
separate target directory is expensive.

| Purpose | Command | Expected on success |
|---|---|---|
| Format check | `just fmt --check` | exit 0 |
| Workspace check | `just check` | exit 0 |
| Unit tests | `just test` | all pass |
| Lint | `cargo clippy --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Visibility | `just hawk` | zero accepted findings; run once |
| Speech integration | `just test-integration` | all integration tests pass |
| Dev install for live check | `just install-dev` | service restarts successfully |

## Scope

**In scope**:

- `crates/dictate-speech/src/models.rs`
- `crates/dictate-speech/src/transcription.rs`
- `crates/dictate-speech/src/dictation.rs`
- `crates/dictate-speech/src/eval.rs`
- `crates/dictate-speech/src/lib.rs`
- `crates/dictate-speech/tests/integration.rs`
- `crates/dictate/src/settings.rs`
- `crates/dictate/src/daemon.rs`
- `crates/dictate/src/cli.rs` only where headless mode/model reporting or
  CLI tests need adjustment
- `crates/dictate-dev/src/lib.rs` for the changed plan/session API
- `crates/dictate-ui/src/app.rs`
- `crates/dictate-ui/src/overlay.rs`
- `crates/dictate-ui/src/partial.rs`
- `crates/dictate-ui/src/components/partial_card.rs`
- `crates/dictate-ui/src/lib.rs`
- `README.md`
- this plan and `.agents/plans/product-direction/README.md`

**Out of scope**:

- `crates/dictate-desktop/` — audio ducking behavior is preserved, not
  redesigned here.
- New recognizer runtimes or model downloads outside the existing
  catalog.
- A third/hybrid mode, offline rescoring, a second recognizer, or an
  automatic Batch fallback from Realtime.
- Compatibility aliases for `partials_model` or `partials_font_*`.
- Changing formatter rules, dictionaries, replacements, or spoken
  command semantics.
- Persisting the Realtime card into the Transcribing state. It still
  closes when Recording ends.
- App-aware profiles, history, and LLM cleanup.

## Target domain model

Put the recognition invariant in `dictate-speech`, not only in TOML
validation.

```text
RecognitionMode = Batch | Realtime

ModelCatalogEntry --reports--> RecognitionMode
TranscriptionPlan = valid (RecognitionMode, ModelCatalogEntry, DictationContext)
Recognizer = one model instance matching the plan
RecognitionSession = Batch | Realtime

Batch session:
    feed(samples) -> no hypothesis
    finish(complete utterance) -> one batch decode -> classified result

Realtime session:
    feed(accepted samples) -> changed raw hypothesis
    finish(same complete utterance) -> drain same online stream
                                   -> classified result
```

`RecognitionSession::finish` consumes the session. This makes feed after
finish and a second finish impossible. Its Realtime variant owns one
`OnlineStream` borrowed from one `OnlineRecognizer`; its Batch variant
borrows the offline recognizer and decodes only in `finish`.

The complete utterance passed to `finish` supplies Batch audio and common
signal classification. Realtime has already consumed the same samples;
it must not replay the utterance or create another stream.

## Steps

### Step 1: Model Batch and Realtime in `dictate-speech`

In `models.rs`, add an exhaustive `RecognitionMode::{Batch, Realtime}`
and make every `ModelCatalogEntry` report one mode. Derive this from the
closed recognizer kind; do not add a parallel mutable flag.

Replace the single/mixed defaults with:

- `DEFAULT_BATCH_MODEL_ID` / `default_batch_model()` for
  `parakeet-tdt-0.6b-v2-int8`.
- `DEFAULT_REALTIME_MODEL_ID` / `default_realtime_model()` for
  `parakeet-unified-0.6b-int8-streaming-560ms`.

Remove `DEFAULT_MODEL_ID`, `default_model`, and
`default_partials_model` from the final public API. Replace
`is_streaming()` call sites with recognition-mode checks. Exhaustive
matches must list both modes.

Make `TranscriptionPlan` carry `RecognitionMode`. Its constructor must
reject a model from the other mode through a typed `thiserror` error;
settings will add application context at the boundary. A valid plan
must expose `mode()`, `model()`, and `context()`.

Add the end-state recognition-session API in `transcription.rs`. Names
may differ, but these properties may not:

- `Recognizer::start_session` always returns a session because the
  recognizer's kind is known.
- Batch `feed` returns no hypothesis.
- Realtime `feed` uses the existing online stream and emits only changed,
  non-empty hypotheses.
- Consuming `finish(self, utterance)` returns `TranscriptionResult` and
  uses `classify_transcript` in both modes.
- Realtime `finish` calls `input_finished`, drains that same stream, and
  never decodes the utterance again.
- The public headless `transcribe` path uses this session abstraction.
  For Realtime files, feed fixed chunks then call `finish`; remove
  `decode_simulated_streaming` once no caller needs it.

Keep Sherpa types private. Do not expose separate offline/online
recognizer APIs to the daemon.

**Verify**:
`cargo test -p dictate-speech --lib transcription:: && cargo test -p dictate-speech --lib models::`
→ both selected test groups pass.

### Step 2: Replace the mixed settings contract

In `crates/dictate/src/settings.rs`:

- Add serde-backed `recognition_mode`, defaulting to Batch.
- Change stored `model` to `Option<String>` so an omitted model resolves
  after the mode is known.
- Remove `partials_model`.
- Rename the live-card settings and typed accessor to Realtime terms.
- Keep `deny_unknown_fields`; removed keys must produce an unknown-field
  error.
- Resolve a mode-specific default model, then build the fallible
  `TranscriptionPlan`. Fail before model download or recognizer creation.
- When a configured or CLI-overridden model has the wrong mode, report
  the selected mode, bad model ID, valid model IDs for that mode, and a
  valid example.
- Preserve the existing formatter `mode`; do not rename it.

Update `README.md`'s settings example and model table. State plainly:
Batch is the default and has no live transcript; Realtime shows live
text and delivers the finished result from that same session.

Tests in `settings.rs` must cover:

- Empty config resolves Batch and its Parakeet TDT default.
- Realtime with no model resolves Parakeet Unified.
- Explicit valid model for each mode succeeds.
- Batch plus a Realtime model fails.
- Realtime plus a Batch model fails.
- CLI model override is checked against the configured mode.
- `dictate transcribe` accepts `--recognition-mode batch|realtime` as a
  headless override; an omitted model then uses that mode's default.
- Unknown model errors list only models valid for the selected mode.
- `partials_model` is rejected as unknown.
- `partials_font_family` and `partials_font_size` are rejected as unknown.
- `realtime_font_family` and `realtime_font_size` parse and validate.
- Partial settings still inherit all unrelated defaults.

**Verify**: `cargo test -p dictate settings::tests` → all settings tests
pass.

### Step 3: Make accepted microphone samples explicit

Realtime's final stream and `ReadyDictation::utterance` must contain the
same sample sequence.

Change `RecordSamplesUpdate` in `dictation.rs` so Recording and
AutoStopped report the number of samples accepted from the input batch.
A private append result may carry `{ accepted, reached_limit }`; keep the
public states named and exhaustive. Pending-stop and stopping states
must report their accepted count too. Ignored and stale batches report
zero by construction, not through a sentinel.

In the `CaptureHandler::samples` implementation for
`DictationCaptureHandler`:

1. Ask `DictationControl::record_samples` what it accepted.
2. If Realtime owns a sample sender, copy only
   `samples[..accepted_count]` into it.
3. Batch owns no sender and performs no live sample copy.
4. Keep spectrum updates and stream-error reporting unchanged.

Tests in `dictation.rs` and a small daemon helper must pin:

- A normal accepted batch reports its full length.
- A stale or ignored batch forwards nothing.
- The cap-crossing batch reports only the accepted prefix.
- No samples after auto-stop can reach the Realtime session.
- Batch creates no live sample channel.

**Verify**:
`cargo test -p dictate-speech --lib dictation::tests && cargo test -p dictate daemon::tests`
→ all selected tests pass.

### Step 4: Give each recording one recognizer session

Refactor `daemon.rs` to initialize one recognizer from the validated
`TranscriptionPlan`. Remove `partials_model` from `daemon::run`,
`Daemon`, `Daemon::start`, `spawn_microphone_worker`, worker config, and
recognizer initialization. Delete `initialize_worker_recognizers`.

Keep the recognizer as a worker local declared before any borrowed
session. Do not put an owned recognizer and a session borrowing it in the
same struct. The active recording state may own:

- one `RecognitionSession<'_>`;
- an optional sample receiver, present only for Realtime.

At microphone-open success, start one session. Batch starts no live
channel and sends no hypotheses. Realtime drains accepted batches on the
existing worker poll. For each changed raw hypothesis:

- retain the diagnostic raw-hypothesis log;
- run `DictationFormatter` with `plan.context()`;
- send the resulting text to the UI only when non-empty.

Normal finalization order is required:

1. Stop/drop `Mic`; this joins capture and flushes resampling.
2. Release audio ducking. Playback returns before recognition work.
3. Measure and persist the captured utterance, preserving the current
   save-before-recognition diagnostic behavior.
4. In Realtime, drain every accepted queued batch into the active
   session.
5. Take and consume the active session with `finish(utterance)`.
6. Pass its `TranscriptionResult` into the existing formatting,
   delivery, last-transcript, and overlay outcome path.

Split capture measurement/persistence from recognition if needed so a
Batch decode failure cannot prevent the WAV diagnostic from being saved.

Batch `finish` performs the only decode. Realtime `finish` drains the
existing online stream. Neither mode creates a replacement recognizer or
session during finalization.

Cancel, microphone stream error, empty capture, superseded recording,
and worker exit must drop the active session and receiver. A non-empty
Realtime `ReadyDictation` without its active session is an invariant
failure: log it, show a new `OverlayState::RecognitionFailed` briefly,
and do not fall back to Batch or start a new stream. Add that exhaustive
state in `dictate-ui/src/overlay.rs` using the existing failure visual
language and add its label/render tests; do not call it a delivery
failure.

Refactor `finalize_ready_dictation` to accept an already-produced
`TranscriptionResult` rather than a recognizer. Keep capture persistence
and delivery behavior unchanged.

**Verify**: `cargo test -p dictate daemon::tests` → all daemon tests pass;
`just check` → exit 0.

### Step 5: Rename the UI seam without coupling crates

In `dictate-ui`, rename public and protocol terms from the old
second-model design:

- `PartialTextStyle` → `RealtimeTextStyle`.
- `Overlay::send_partial` → `send_hypothesis`.
- `OverlayMessage::Partial` → `Hypothesis`.

Internal `PartialView` and `partial.rs` may keep "partial" because a
partial hypothesis remains a valid Realtime concept. Do not make
`dictate-ui` import `dictate-speech` types. The daemon decides whether a
recognition event becomes a UI message.

Preserve:

- revision-load-only hypothesis messages;
- acceptance only while the pill is Recording;
- clearing on every Show and Hide;
- GPUI-native width wrapping;
- one-to-four-line growth, bottom scrolling, scrollbar, font family and
  font size;
- no text window in Batch because no hypotheses are sent.

Rename the existing protocol tests and keep the late-after-Transcribing
case. Add one daemon-level assertion that Batch never calls the UI
hypothesis seam.

**Verify**: `cargo test -p dictate-ui && cargo test -p dictate daemon::tests`
→ all selected tests pass.

### Step 6: Update headless evaluation and integration coverage

Update `dictate-speech/src/eval.rs`, `dictate-dev/src/lib.rs`, CLI parsing
and tests, and integration tests for the mode-bearing
`TranscriptionPlan` and one session API. Add the headless
`dictate transcribe --recognition-mode batch|realtime` override described
in Step 2.

Headless transcription must use the same path as daemon finalization:

- Batch decodes the complete WAV once.
- Realtime feeds chunks through one session and consumes its `finish`.

Add `recognition_mode` to machine-readable benchmark/transcription JSON
if that output currently identifies the selected model; update the
stable JSON-shape test with the explicit mode. Do not infer mode from a
model ID in callers.

Integration coverage must prove:

- the Batch default retains its existing fixture and degraded-audio
  quality gates;
- the Realtime default creates an online recognizer and emits at least
  one changed hypothesis on a speech fixture;
- Realtime session `finish` equals headless Realtime transcription for
  the same in-memory fixture samples;
- every catalog model belongs to exactly one mode and each default
  belongs to its named mode.

Prove the resampler tail by composition rather than adding a microphone
mock: retain the existing
`audio_worker_delivers_one_flush_tail_after_resampled_batches` unit test,
add daemon tests that drop/join the mic before draining the receiver, and
prove that Realtime `finish` consumes all drained in-memory batches.

Replace the test-only single-model `DICTATE_MODEL_DIR` override with
mode-specific `DICTATE_BATCH_MODEL_DIR` and
`DICTATE_REALTIME_MODEL_DIR`. Each helper first checks its matching env
var, then the model's normal catalog directory, and errors with the exact
path and provisioning command when absent. Do not retain the ambiguous
single-directory override.

Provision both defaults through the public headless path before the
integration gate (these commands download a missing model):

```sh
just run transcribe crates/dictate-speech/tests/fixtures/ljspeech/LJ001-0001.wav --raw --recognition-mode batch
just run transcribe crates/dictate-speech/tests/fixtures/ljspeech/LJ001-0001.wav --raw --recognition-mode realtime
```

**Verify**: both provisioning commands exit 0; `just test-integration` →
all integration tests pass.

### Step 7: Remove the mixed pipeline and run final gates once

Delete obsolete functions, fields, tests, docs, and imports. Current
source and user docs must contain no old configuration names or dual
recognizer initialization.

Run, in order, once:

1. `just fmt --check`
2. `just check`
3. `just test`
4. `cargo clippy --locked --all-targets --all-features -- -D warnings`
5. `just hawk`

Do not run `cargo clean`. Do not rerun Hawk unless fixing a Hawk finding
changed source visibility.

Then check removal:

```sh
rg -n "partials_model|partials_font_" crates README.md
rg -n "default_partials_model|initialize_worker_recognizers|decode_simulated_streaming" crates
```

Both searches must return no matches. Historical plan files are allowed
to retain old terms.

**Verify**: all five gates exit 0; both removal searches find nothing.

### Step 8: Verify both product modes live

Install once with `just install-dev`, then test:

1. No recognition settings: daemon chooses Batch and loads one offline
   Parakeet recognizer. Recording shows only the waveform pill. No text
   card opens. Stop performs one decode and delivers text.
2. `recognition_mode = "realtime"` with no model: daemon chooses
   Parakeet Unified and loads one recognizer.
3. Speak for several seconds: formatted hypotheses update in the live
   card, wrap at measured width, grow to four lines, scroll, and show the
   overflow indicator.
4. Stop: the card closes on Transcribing; delivered text comes from that
   same session's finish and differs only where the final hypothesis
   settled before formatting.
5. Cancel mid-speech: both surfaces close and no late hypothesis reopens
   the card.
6. Record silence in both modes: no delivered transcript; Realtime never
   opens an empty card.
7. With capture persistence enabled, confirm a Realtime recording still
   saves its WAV before recognition finishes. Exact final equality is
   covered by the in-memory integration test; the saved 16-bit PCM WAV
   is diagnostic data and may quantize samples.
8. Start with Batch plus a streaming model and Realtime plus an offline
   model. Both starts fail with specific settings errors before download.
9. Start with `partials_model` or `partials_font_size`. Both fail as
   unknown settings.
10. During stop, cancel, and no-transcript checks, system audio restores
    when recording ends rather than after recognition finishes.

Record the chosen configs and observations in the change description.

## Test plan

Tests belong beside the plain state and policy they cover. Do not add a
GPUI harness or mock Sherpa.

- `models.rs` / `transcription.rs`: mode classification, mode/model
  mismatch, Batch feed no-op, consuming finish semantics where pure.
- `dictation.rs`: exact accepted count for normal, ignored, pending-stop,
  stopping, and cap-truncated batches.
- `settings.rs`: default selection, mode-specific overrides and errors,
  removed-key rejection, Realtime font settings.
- `daemon.rs`: forwarding only accepted prefixes, no Batch sample
  channel, queue drain before Realtime finish, cancellation cleanup, no
  second session at finalization.
- `dictate-ui/src/app.rs`: hypotheses accepted only during Recording and
  cleared on Show/Hide.
- `dictate-ui/src/overlay.rs`: `RecognitionFailed` has an accurate label
  and uses the existing error-state visual language.
- `eval.rs`: stable JSON includes recognition mode when model metadata is
  emitted.
- Existing `mic.rs` tests plus daemon drain tests: the resampler flush
  reaches the session before finish.
- `tests/integration.rs`: one Batch quality path and one Realtime
  hypothesis-to-authoritative-finish path using identical in-memory
  samples.

## Done criteria

- [ ] Batch is the default and uses one non-streaming recognizer.
- [ ] Batch sends no live hypotheses and never opens the text card.
- [ ] Realtime uses one streaming recognizer and one session per
      recording.
- [ ] Realtime's consumed `finish` result is the only delivered raw
      transcript; no replay, rescore, or second stream exists.
- [ ] Realtime and its retained utterance receive exactly the same
      accepted samples, including cap truncation and resampler flush.
- [ ] Displayed Realtime hypotheses and the final transcript use the
      same formatter context.
- [ ] Model/mode mismatches fail during settings/plan construction.
- [ ] `partials_model` and `partials_font_*` are removed without aliases.
- [ ] Existing overlay wrapping, growth, scrolling, scrollbar, and font
      controls survive under Realtime names.
- [ ] Audio ducking still restores at recording end on all tested paths.
- [ ] `just fmt --check`, `just check`, `just test`, check-only clippy,
      one `just hawk`, and `just test-integration` all pass.
- [ ] Live checks cover both modes.
- [ ] `jj st` shows only in-scope files.

## STOP conditions

Stop and write a handback if:

- Realtime finalization appears to need a second recognizer, second
  online stream, offline fallback, or utterance replay.
- Holding the active session requires `unsafe`, leaked ownership, a
  self-referential struct, or `Arc<Mutex<OnlineRecognizer>>`.
- `Mic::drop` cannot close/join capture before the accepted sample queue
  is drained.
- Samples fed to Realtime can differ from samples retained in
  `ReadyDictation`, including stale post-cancel data, cap truncation, or
  the resampler tail.
- A model/mode mismatch reaches download or recognizer construction.
- Parakeet Unified cannot create a streaming recognizer, emit usable
  hypotheses, or return a stable finished result on the fixture test.
- Realtime `finish` differs from headless Realtime transcription after
  both use the same session abstraction and sample sequence.
- Batch still allocates a live sample channel or can open the live text
  surface.
- The change requires speech types in `dictate-ui` or desktop audio types
  outside `dictate-desktop`.
- Existing live text or audio ducking behavior is lost.
- Passing tests seems to require old settings aliases or dual-shape
  parsing.
- A verification command fails twice after a reasonable fix.

The handback must state the current code state, desired outcome, observed
evidence, and unresolved question. Describe the fork; do not choose it.

## Maintenance notes

- Realtime models may revise the unsettled tail. One recognizer removes
  the false cross-model jump; it does not promise monotonic hypotheses.
  Stable-prefix styling can be planned later if revisions remain noisy.
- Fast Conformer stays available as a lower-latency explicit Realtime
  model. Parakeet Unified is the default because Realtime final text now
  matters, not merely cosmetic preview speed.
- Future model catalog entries must declare Batch or Realtime. A model
  cannot belong to both without a new explicit recognizer capability.
- If a future model family supports streaming plus an intentional
  full-context rescore, plan that as a new named mode. Do not smuggle the
  mixed pipeline back under Realtime.
