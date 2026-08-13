# Plan 001: Replace Tauri-era CI with a Rust CI pipeline

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from dd6db2c175a3 -- .github Justfile`
> If in-scope files have changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: LOW (CI-only; no source changes)
- **Depends on**: none — execute this first; it gives every later plan a CI gate
- **Planned at**: revision `mtnsrkmyruyz` (git `dd6db2c175a3`), 2026-06-11

## Why this matters

The repo was rewritten from a Tauri/Svelte app to a pure Rust/GPUI app (branch
`gpui-native-rewrite`), but all CI still targets the old stack: it installs Bun
and webkit2gtk, runs `bun run tauri build`, and watches paths (`src-tauri/**`,
`package.json`, `bun.lock`, `dist/aur/**`) that no longer exist. Once this
branch merges, the repo has no working CI at all — no automated check, test,
lint, or format gate. This plan deletes the dead pipeline and replaces it with
a Rust one, so every subsequent plan in this effort lands behind a real gate.

## Current state

- `.github/workflows/build.yml` — Tauri build: installs webkit2gtk/Bun, runs
  `bun install` + `bun run tauri build`, signs AppImage/RPM bundles. Trigger
  paths include `src-tauri/**`, `package.json`, `bun.lock`. **Dead** — none of
  those paths exist in the tree (verified: no `src-tauri/`, no `package.json`).
- `.github/workflows/test-aur.yml` — tests an AUR PKGBUILD under `dist/aur/**`.
  **Dead** — `dist/` does not exist.
- `.github/workflows/release.yml` — chains `build.yml` + `test-aur.yml` on
  release publish, uploads deb/AppImage/rpm bundles, pushes to AUR. **Dead**
  (depends on the two above).
- `.github/workflows/zizmor.yml` — lints workflows with zizmor, uploads SARIF.
  **Current and correct — keep it.** It is also the convention exemplar: every
  checkout uses `persist-credentials: false`, permissions are explicitly
  minimal per job.
- `.github/dependabot.yml` — three ecosystems: `bun` at `/` (dead),
  `cargo` at `/src-tauri` (wrong directory), `github-actions` at `/` (keep).
- `Justfile` (repo root) — the `clippy` recipe is mutating by default:

  ```just
  clippy *ARGS:
      cargo clippy --all-targets --all-features --benches --fix {{ ARGS }} -- -D warnings
  ```

  There is no check-only clippy recipe, which CI needs.
- `rust-toolchain.toml` pins `channel = "1.96"`. `.rustfmt.toml` uses unstable
  options (`imports_granularity`, `group_imports`), so format checking requires
  nightly rustfmt — the Justfile `fmt` recipe is `cargo +nightly fmt`.
- Toolchain facts verified locally at the planned-at revision:
  `cargo check --all-targets`, `cargo test` (25 tests), and
  `cargo clippy --all-targets` all pass clean.
- The crate depends on `gpui`/`gpui_platform` from the Zed git repo (wayland
  feature), `cpal` (ALSA), and `sherpa-onnx` — all of which need system
  libraries at build time. The old `build.yml` installed `libasound2-dev`,
  `libvulkan-dev`, `glslc` among its Tauri deps; the Zed project's Linux docs
  list the GPUI-side deps.

## Commands you will need

| Purpose            | Command                                          | Expected on success |
|--------------------|--------------------------------------------------|---------------------|
| Type check         | `just check`                                     | exit 0              |
| Tests              | `just test`                                      | all pass            |
| Lint (check-only)  | `cargo clippy --all-targets -- -D warnings`      | exit 0              |
| Format             | `just fmt`                                       | exit 0              |
| VCS status         | `jj st`                                          | shows your edits    |
| Push for CI        | `jj bookmark set <name> -r @- && jj git push`    | branch on origin    |
| Watch CI           | `gh run watch` / `gh run list --limit 5`         | runs visible        |

## Scope

**In scope** (the only files you should modify/delete/create):
- `.github/workflows/build.yml` (delete)
- `.github/workflows/test-aur.yml` (delete)
- `.github/workflows/release.yml` (delete)
- `.github/workflows/ci.yml` (create)
- `.github/dependabot.yml` (edit)
- `Justfile` (edit)

**Out of scope** (do NOT touch):
- `.github/workflows/zizmor.yml` — current and correct.
- `src/**`, `Cargo.toml` — no source changes in this plan.
- Release/packaging automation for the new binary — deliberately deferred
  (see effort README "Deferred"); do not write a new release.yml.

## Steps

### Step 1: Delete the dead workflows

Delete `build.yml`, `test-aur.yml`, and `release.yml` from
`.github/workflows/`. This is a deliberate clean break: the project owner
prefers removing obsolete shapes over keeping broken compatibility, and
release automation for the GPUI binary will be designed later.

**Verify**: `ls .github/workflows` → exactly `ci.yml` missing for now, only
`zizmor.yml` remains.

### Step 2: Create `.github/workflows/ci.yml`

A single workflow named `ci` that gives the repo its correctness gate:

- **Triggers**: `pull_request`, and `push` to `main` (paths-filter on
  `src/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
  `.rustfmt.toml`, `Justfile`, `.github/workflows/ci.yml`).
- **Concurrency**: group per ref with `cancel-in-progress: true` (same shape
  as the deleted `build.yml` used).
- **Jobs** (suggested split; merging lint+fmt into one job is fine):
  - `test`: install system deps, install the pinned toolchain, restore cargo
    cache, run `cargo check --all-targets` then `cargo test`.
  - `lint`: same setup, run `cargo clippy --all-targets -- -D warnings`.
  - `fmt`: install nightly rustfmt (`rustup toolchain install nightly
    --component rustfmt` or equivalent action input) and run
    `cargo +nightly fmt --check`. This job needs no system deps and no cache.
- **Toolchain/cache**: use `actions-rust-lang/setup-rust-toolchain@v1` (the
  action the old workflow already used; it reads `rust-toolchain.toml` and
  has built-in caching).
- **System deps** (starting list — iterate until the build is green):
  `build-essential cmake libasound2-dev libfontconfig1-dev libwayland-dev
  libxkbcommon-dev libxkbcommon-x11-dev libvulkan-dev libssl-dev glslc`.
  If a compile error names a missing native library, add its `-dev` package.
- **zizmor compliance** (the `zizmor.yml` workflow will lint this file on the
  PR): every `actions/checkout` step gets `persist-credentials: false`;
  declare a minimal top-level `permissions: contents: read`.

Note: the first uncached run compiles GPUI from the Zed git repo — expect a
long (30–60 min) first run. That is acceptable; the cache makes later runs
reasonable.

**Verify**: `cat .github/workflows/ci.yml` exists; YAML is well-formed
(`python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"`
→ exit 0, or any equivalent parser available on the machine).

### Step 3: Fix dependabot

In `.github/dependabot.yml`: remove the `bun` entry entirely, change the
`cargo` entry's `directory` from `/src-tauri` to `/`, keep the
`github-actions` entry unchanged. Preserve the existing schedule, labels, and
group structure of the entries you keep.

**Verify**: `grep -c "src-tauri\|bun" .github/dependabot.yml` → `0`.

### Step 4: Split the Justfile clippy recipe

Make `clippy` check-only and move the mutating behavior to `clippy-fix`:

```just
clippy *ARGS:
    cargo clippy --all-targets --all-features {{ ARGS }} -- -D warnings

clippy-fix *ARGS:
    cargo clippy --all-targets --all-features --fix {{ ARGS }} -- -D warnings
```

(The `--benches` flag in the old recipe is redundant with `--all-targets`;
drop it.)

**Verify**: `just clippy` → exit 0 and **no files modified** (`jj st` shows
only this plan's intended changes).

### Step 5: Push and confirm CI is green

Describe the change (`jj describe -m "Replace Tauri-era CI with Rust CI"`),
create/move a bookmark for it, `jj git push`, and open a PR against `main`
(`gh pr create`). Watch the `ci` and `zizmor` runs.

Iterate on system deps in `ci.yml` if the build fails on a missing native
library — each fix is a new push. Do not weaken the gates (no `continue-on-error`,
no dropping clippy `-D warnings`) to get to green.

**Verify**: `gh run list --branch <branch> --limit 5` → `ci` and `zizmor`
both `completed/success`.

## Test plan

No Rust tests change. The test is the pipeline itself: Step 5's green run on
a PR is the acceptance test.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `ls .github/workflows` → `ci.yml zizmor.yml` only
- [ ] `grep -rc "src-tauri\|bun\|tauri" .github/` → 0 matches
- [ ] `just clippy` → exit 0 with no working-copy modifications
- [ ] `gh run list` shows `ci` green on the PR branch
- [ ] No files outside the in-scope list are modified

## STOP conditions

Stop if:

- The CI build fails on the `sherpa-onnx` or `gpui` native build after three
  dependency-fix iterations — write a handback listing the exact compiler
  errors and the dep packages already tried.
- The GitHub runner consistently exceeds the job time limit compiling GPUI —
  handback; caching strategy may need design (e.g. prebuilt artifacts).
- `zizmor` flags the new workflow with findings you cannot resolve by adding
  `persist-credentials: false` / tightening `permissions`.
- The work appears to require touching an out-of-scope file.

On stopping, write a **handback**: current state, desired outcome, lingering
questions. Descriptive, not prescriptive.

## Maintenance notes

- When delivery features add new system deps (e.g. wl-clipboard), the CI dep
  list needs the matching `-dev` packages.
- Release automation (signed artifacts, AUR) was deleted, not ported — see
  effort README "Deferred". Whoever designs the new release flow should
  consult the deleted files in VCS history (`jj file show -r dd6db2c175a3
  .github/workflows/release.yml` or git history) for the signing/AUR shape
  that used to exist; `dictate-signing.pub` at the repo root is the old
  public key.
