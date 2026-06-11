# Plan 006: Spike — live partial transcripts without leaving sherpa-onnx

> **Executor instructions**: This is a **spike**, not a feature build. The
> deliverable is a findings document plus throwaway prototypes under
> `examples/` — no daemon or overlay changes. Follow the steps, run every
> verification, and honor STOP conditions. When done, update this plan's
> status row in the effort README.
>
> **Drift check (run first)**:
> `jj diff --from e65b4661cfcf -- Cargo.toml src/models.rs src/transcription.rs`
> Only `Cargo.toml` (dev-dependencies, if needed) may be modified; the
> source excerpts below are context, not change sites.

## Status

- **Effort**: S–M
- **Risk**: LOW to the codebase (examples + a document only); the open
  question is product-shaped, not technical
- **Depends on**: 004 (the eval there proves the Parakeet catalog entries
  actually work end-to-end in this codebase; this spike reuses that)
- **Planned at**: revision `pkzmprvzlnsn` (git `e65b4661cfcf`), 2026-06-11

## Why this matters

Words appearing while you speak is the single biggest *perceived-speed*
differentiator in the 2026 survey (Aqua Voice's streaming display is cited
as feeling faster than competitors at equivalent real latency). The naive
route — a true streaming model — is blocked: sherpa-onnx's online API only
serves Zipformer/Paraformer/older-FastConformer models (an accuracy step
down), Parakeet has no streaming export, and Moonshine v2 is streaming by
architecture but wrapped offline-only in sherpa-onnx today. Cobbling a
second inference runtime (Kyutai via candle, hand-rolled Moonshine on
`ort`) was considered and rejected (see effort README).

But Dictate's delivery contract makes the problem easier: final text is
always produced at stop by the offline decode, so partials are **overlay
cosmetics** — they only need to be plausible, never correct. Two
sherpa-only tricks can supply that, and this spike measures whether either
is good enough to justify redesigning the overlay into a text surface:

1. **Periodic offline re-decode**: re-run offline Parakeet on the growing
   sample buffer every ~1s, display the latest hypothesis. (This is what
   Superwhisper ships as "Parakeet realtime" — there is no streaming
   Parakeet anywhere; they re-decode.) Parakeet-quality partials; cost
   grows with buffer length.
2. **Two-pass hybrid**: a sherpa *online* stream (streaming Zipformer or
   NeMo cache-aware FastConformer) feeds the display; offline Parakeet
   still produces the final text. Constant cost; lower partial quality.

## Current state

- ASR is offline/batch only: `src/transcription.rs` decodes a complete
  `CapturedUtterance` after stop; `src/models.rs:110-119`
  (`create_recognizer`) builds a sherpa `OfflineRecognizer` from a catalog
  entry; Parakeet entries at `src/models.rs:290-309`.
- The catalog's public API (`model_by_id`, `ensure_downloaded`,
  `create_recognizer`) is callable from an example. If
  `CapturedUtterance` can't be constructed outside the crate, drive the
  sherpa-onnx `OfflineRecognizer` directly in the example instead —
  do not add `pub` surface to `src/` for a spike.
- The sherpa-onnx Rust crate (1.13) exposes `OnlineRecognizer` /
  `OnlineStream` for trick 2 (streaming API added in 1.12.26); streaming
  model archives (Zipformer, NeMo streaming FastConformer) download from
  the same k2-fsa release bucket as the existing catalog
  (`src/models.rs:79-81` shows the URL shape).
- The overlay is a 72×40px waveform pill (`src/app.rs:23-25`) — displaying
  text is a product redesign, deliberately **out of scope** here; the
  spike renders partials to the terminal.
- Research leads (2026-06-11, verify against the sherpa-onnx CHANGELOG at
  execution time): Moonshine v2 offline-only in sherpa-onnx; no streaming
  Parakeet export from NVIDIA. If streaming Moonshine has landed in
  sherpa-onnx since, that changes the whole calculus — check first.

## Commands you will need

| Purpose            | Command                          | Expected on success |
|--------------------|----------------------------------|---------------------|
| Check (w/ examples)| `cargo check --all-targets`      | exit 0              |
| Run a prototype    | `cargo run --release --example <name>` | partials stream to terminal |

Use `--release` for all measurements — debug-build decode timings are
meaningless (see the dev-profile finding in
`plans/gpui-rewrite-hardening/006-overlay-frame-pacing.md`).

## Scope

**In scope**:
- `examples/` (prototype binaries; throwaway quality is fine)
- `Cargo.toml` (`[dev-dependencies]` only, if anything is needed)
- `plans/product-direction/spike-live-partials-findings.md` (the deliverable)

**Out of scope** (do NOT touch):
- `src/` — no production code, no new `pub` items.
- Overlay/UI rendering of partials — the display-surface redesign is the
  follow-up product decision this spike informs.
- Second inference runtimes (candle/Kyutai, `ort`/Moonshine) — already
  rejected; do not re-litigate inside the spike.

## Steps

### Step 0: Check whether the premise still holds

Read the sherpa-onnx CHANGELOG/docs for the pinned version and the latest
release: has streaming Moonshine (or any streaming model at
Parakeet-class accuracy) landed? If yes, write that up as the finding and
skip to Step 4 — the tricks below exist only because of the gap.

**Verify**: the findings doc opens with the answer and the versions checked.

### Step 1: Prototype periodic re-decode

`examples/partials_redecode.rs`: load a prerecorded 16kHz WAV (record one
with the daemon or `arecord`; ~30s of natural dictation including
technical vocabulary), feed it into a buffer **paced in real time**
(sleep-driven chunks), and every ~1s decode the full buffer with offline
Parakeet (`parakeet-tdt-0.6b-v2-int8`), printing each hypothesis with a
timestamp. Measure and record:

1. Decode wall-time per pass at buffer lengths ~5s / 10s / 20s / 30s
   (the quadratic-total-work concern — does the last pass still feel
   instant?).
2. **Prefix stability**: between consecutive passes, how often does
   already-displayed text change? Count word-level rewrites of the first
   80% of the previous hypothesis. Wildly rewriting partials feel worse
   than no partials.
3. CPU utilization while re-decoding (rough `top`/`ps` observation is
   fine).

Suggested pass/fail intuition (not a hard gate — record the numbers):
last-pass decode ≤ ~500ms at a 30s buffer, and prefix rewrites rare after
a hypothesis has survived two passes.

**Verify**: the example runs and the findings doc has the three
measurements in a table.

### Step 2: Prototype the two-pass hybrid

`examples/partials_streaming.rs`: same paced WAV feed through a sherpa
`OnlineRecognizer` with a streaming model (pick the best-documented
English option — NeMo cache-aware streaming FastConformer or streaming
Zipformer; archive URLs in the k2-fsa docs). Print partials as they
arrive. Record: partial latency/cadence, subjective transcript quality on
the same WAV (especially the technical terms), model download size, and
added resident memory for keeping a second model loaded alongside
Parakeet.

**Verify**: the example runs and the findings doc compares both tricks on
the same recording.

### Step 3: Decide on a segmentation escape hatch (analysis only)

If Step 1's cost curve is bad at long buffers, note whether VAD-segmented
re-decode (only re-decode since the last silence boundary, prepending
already-final segments) would fix it — sherpa-onnx ships Silero VAD with
Rust bindings. Analysis paragraph only; do not build it.

**Verify**: paragraph exists in the findings doc.

### Step 4: Write the findings and recommendation

`plans/product-direction/spike-live-partials-findings.md`, verdict first:

- Which trick (if either) is good enough to ship behind the overlay, with
  the measured numbers.
- What it costs: CPU, memory, second-model download (hybrid only),
  hypothesis-flicker UX risk.
- The product question it tees up for the maintainer: partials require
  the overlay to become a text surface (Aqua-style box vs today's pill) —
  state the dependency, don't design it.
- Date, sherpa-onnx version, and hardware tested (findings go stale).
- Keep prototypes in `examples/` if they compile under
  `cargo check --all-targets`; delete anything broken.

**Verify**: findings doc exists, leads with a verdict, every claim backed
by a local measurement or cited source.

## Done criteria

- [ ] `spike-live-partials-findings.md` exists with verdict, measurement
      table, and the overlay-redesign question stated
- [ ] Step 0 answered with versions checked
- [ ] `cargo check --all-targets` → exit 0 (examples compile or were removed)
- [ ] `jj st` shows only `examples/`, `Cargo.toml`, and the findings doc

## STOP conditions

Stop if:

- Plan 004 hasn't landed and the Parakeet recognizer config fails at
  `create_recognizer` — that's 004's Step 1 problem; don't debug it here.
- Driving the prototypes requires new `pub` items in `src/` — handback;
  the seam design belongs to the follow-up plan.
- Both tricks measure clearly unusable (multi-second partial latency AND
  heavy flicker) — that's a valid verdict, not a failure; write it up and
  the rejected-for-now status stands with numbers attached.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- The follow-up (if the verdict is positive) is two plans: the overlay
  text-surface redesign (product/design-led) and the daemon partials
  pipeline (worker → overlay channel; the spectrum side-path at
  `src/app.rs:42-44` is the precedent for high-rate overlay data).
- Re-run Step 0 on sherpa-onnx upgrades — streaming Moonshine v2 landing
  upstream obsoletes both tricks and most of this spike.
- The prefix-stability metric is reusable for evaluating any future
  streaming model; keep its definition in the findings doc.
