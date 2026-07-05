# 003 — Fail Truncated Model Downloads Instead of Reporting Success

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` Now #2 "Daemon failure-contract hardening" — defect: "`src/models.rs` truncated download treated as success"
**Effort index:** [README.md](README.md)
**Planned at:** 2026-07-05, working copy `mqpsnkknoowr` / git `0830c547`
**Depends on:** none (isolated to `src/models.rs`; may run in parallel with 001/002)
**Executor target:** routine execution ready — yes
**Source type:** roadmap
**Audit category:** correctness
**Standards concern:** `coding-standards` `error-handling.md` — boundary code classifies external outcomes before translating them; a short HTTP body is a failure at the network boundary, not a success
**Impact:** a dropped connection during the first model download surfaces as a clear, retryable-by-restart error naming the byte counts, instead of a corrupt archive that fails later in extraction (or worse, extracts partially) with a misleading message
**Effort:** S
**Risk:** LOW — additive check on one function; the only behavior change is on the truncation path
**Confidence:** HIGH — code path fully read; ureq 3.3 `content_length()` semantics verified in-repo
**Source direction:** roadmap fix sketch: "verify download length"

## Purpose

`download_file` reads the response body to EOF and returns `Ok(())`
regardless of how much arrived; `content_length` is consulted only for
progress lines. A connection dropped mid-body looks identical to success.
Under the failure contract, download failure is fatal-but-honest: it may
land the daemon in `Unavailable`, but it must never masquerade as a ready
model.

## What Better Means

- A body shorter than the advertised `Content-Length` yields
  `Err`, with a message naming the URL and `{downloaded} of {total}` bytes,
  and leaves no partial archive behind to confuse a later attempt.
- Regression bar: healthy downloads (including servers that omit
  `Content-Length`, where no check is possible) behave exactly as today;
  progress reporting unchanged.

## Current-State Evidence

- `src/models.rs:141-183` — `download_file`: builds the ureq agent with
  connect/response/body timeouts (gpui-hardening plan 003's work), then
  loops `reader.read` → `file.write_all` until `read == 0` (:166) and
  returns `Ok(())` (:183) with no length comparison.
- `src/models.rs:155` — `let total = response.body().content_length().unwrap_or(0);`
  used only in the progress branch at :171-178.
- `src/models.rs:91-115` — `ensure_downloaded` treats `download_file`'s
  `Ok` as license to extract; extraction (`extract_tar_bz2`, :186-222) is
  the accidental backstop today — bz2 decode of a truncated file errors,
  but with a message about the archive, not the download, and only after
  the model dir check has already been passed.
- Error-path file handling: on `download_file` `Err`, `ensure_downloaded`
  propagates immediately; the partial `archive_path` is left on disk
  (`fs::remove_file(&archive_path).ok()` at :108 runs only on success).
  Benign today because `File::create` truncates on retry, but it wastes
  disk for a fatal-error state the user may sit in for a while.

## Desired End State

`download_file` compares bytes received against `Content-Length` when the
server provides one, errors on mismatch with the
`failed to download {url}: …` message shape, and best-effort removes the
partial output file on every error path.

## Scope

- `src/models.rs` — `download_file` plus a small pure helper and its tests

## Out of Scope

- Checksums (rejected in the effort index — no upstream manifest to pin).
- Resumable/range downloads.
- Retry logic (download failure remains fatal per the classification table).
- Extraction hardening or catalog validation (the roadmap's "verification
  upgrades batch" owns `models.rs` test coverage broadly).

## Design Claim

`coding-standards` `error-handling.md` boundary rule: classify the
provider's outcome (full body vs short body) at the boundary and translate
it into the local error contract, instead of letting a downstream module
(bz2 extraction) fail with a misattributed cause.

## Architecture Diagnosis

N/A (boundary correctness fix).

## Implementation Sequence

### Step 1 — Extract a pure length check

Add a small helper so the rule is unit-testable without network:

```rust
fn verify_download_length(expected: u64, downloaded: u64) -> Result<()>
```

`expected == 0` (absent/unknown `Content-Length`) → `Ok`. Mismatch → `Err`
with `truncated download: {downloaded} of {expected} bytes` (the caller
wraps with the URL context). Note `downloaded > expected` is also a
mismatch — a body longer than advertised is equally untrustworthy.

### Step 2 — Wire it into `download_file` and clean up on error

Call the helper after the read loop, wrapping with the existing
`failed to download {url}: …` shape. Restructure `download_file` minimally
so that **any** error path (read error, write error, length mismatch)
best-effort removes `output_path` before returning — e.g. run the loop in
an inner closure/function and `let _ = fs::remove_file(output_path);` on
its `Err` before propagating.

### Step 3 — Tests

In a new `#[cfg(test)] mod tests` in `src/models.rs` (none exists — this
starts the file's test module):

- `verify_download_length(0, n)` → `Ok` for any `n`.
- exact match → `Ok`.
- short body → `Err`, message contains both byte counts.
- long body → `Err`.

No network tests; the loop's plumbing is exercised by
`just test-integration` (which really downloads models) when someone runs
it.

## Verification

### Automated

- [ ] `just check` — exit 0
- [ ] `just test` — new `verify_download_length` tests pass
- [ ] `just clippy` — exit 0
- [ ] `cargo +nightly fmt --check` — exit 0

### Evals / Regression Checks

- [ ] `rg -n 'content_length' src/models.rs` — the value now feeds the
      check, not just progress lines.
- [ ] Message shape preserved: `rg -n '"failed to download' src/models.rs`
      still matches (other tooling/plans grep for it).

### Manual

- [ ] Optional (network): delete one small model dir
      (`~/.local/share/dictate/models/moonshine-tiny-en`) and run
      `dictate transcribe tests/fixtures/<any>.wav --model moonshine-tiny-en`
      — healthy download still completes and transcribes.

## Autonomy Boundary

Routine execution may include:

- Everything in the implementation sequence.

Design review is required for:

- Any urge to add retries, resume, or checksum fetching.

Human approval is required for:

- Nothing within scope.

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan and the effort index.
- [ ] `jj diff --from 0830c547 -- src/models.rs` — re-verify evidence on
      any change.
- [ ] Confirm `just test` passes before the first edit.

## STOP Conditions

Stop and hand back if:

- `download_file` has been restructured (e.g. streaming to a temp file or
  a download manager crate) since planning;
- ureq's `content_length()` turns out to report post-decompression sizes
  for this endpoint (GitHub releases serve `.tar.bz2` bytes as-is at
  planning time — if you observe gzip transfer-encoding interactions,
  hand back with the observed headers rather than guessing);
- validation commands fail before changes.

## Rejected Approaches

- **Checksum verification** — rejected at the effort level: sherpa-onnx
  publishes no stable per-archive checksum manifest (see
  `plans/gpui-rewrite-hardening/README.md` Considered-and-rejected).
- **Treating extraction failure as the detection point** — that is the
  status quo's accidental backstop; it misattributes the cause and runs
  after success has already been reported to stderr.
- **Retrying the download in-place** — download failure is classified fatal
  (init-time) in the effort's contract table; retry machinery belongs to a
  future supervisor story, not here.

## Standing Policy Updates

None beyond the effort index.

## Executor Notes

- This file has no test module yet; put the new one at the bottom following
  the repo pattern (`use super::*;`, one import per line).
- Don't touch `VadModel` (dead but deliberately kept — see gpui-hardening
  README) or the catalog constants.
- Update this plan's row in [README.md](README.md) when done.
