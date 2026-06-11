# Plan 002: Spike — pick the text-insertion mechanism for Wayland

> **Executor instructions**: This is a **spike**, not a feature build. The
> deliverable is a findings document plus throwaway prototypes under
> `examples/` — no daemon changes. Follow the steps, run every verification,
> and honor STOP conditions. When done, update this plan's status row in
> the effort README.
>
> **Drift check (run first)**:
> `jj diff --from e65b4661cfcf -- Cargo.toml src/daemon.rs`
> Only `Cargo.toml` (dev-dependencies) may be modified by this plan; the
> daemon excerpt below is context, not a change site.

## Status

- **Effort**: M
- **Risk**: LOW to the codebase (examples + a document only); HIGH
  uncertainty in the findings — that is the point of the spike
- **Depends on**: none (can run in parallel with 001/003/004)
- **Planned at**: revision `pkzmprvzlnsn` (git `e65b4661cfcf`), 2026-06-11

## Why this matters

"Type the dictation into whatever app has focus" is the defining feature of
every polished dictation app, and the single hardest thing to do well on
Wayland — the 2026 survey of Linux dictation tools found that **every**
existing app (Handy, Voxtype, OpenWhispr, hyprvoice, Speech Note) fumbles
it with fragile fallback chains, silent failures on GNOME/KDE, or
ydotool-daemon setup friction that dominates their issue trackers
(e.g. OpenWhispr #240, Voxtype #306, Handy PR #1395). Dictate must pick its
mechanism deliberately, against measured local evidence, before building
plan 001's `DeliveryTarget::Insert` variant on top of it. Web research
(2026-06-11) produced a support matrix, but those findings are **leads, not
facts** — this spike verifies them on real compositors.

## Current state

- Delivery today is stdout-only (`src/daemon.rs:128`); plan 001 adds a
  `DeliveryTarget` enum in `src/delivery.rs` that insertion will extend.
- The maintainer's compositor is **niri** (PLAN.md:55 — the overlay was
  proven there). Treat niri as the must-work target; GNOME/KDE/wlroots as
  should-work.
- `Cargo.toml` has `wayland`-stack access only transitively through GPUI;
  no direct `wayland-client` dependency. PLAN.md:16 sets the precedent:
  "use examples only for major isolated risks" — this is one.
- Research leads to verify (sources: wayland.app protocol pages, June 2026):

  | Mechanism | niri | GNOME | KDE | Sway/Hyprland | Setup friction |
  |---|---|---|---|---|---|
  | `zwp_virtual_keyboard_manager_v1` (wtype-style) | **no** | claimed yes (verify — historically no) | no | yes | none |
  | `zwp_input_method_v2` `commit_string` | **yes** | yes | no | no | none, but conflicts with a real IME |
  | uinput (ydotool/dotool) | yes | yes | yes | yes | daemon + input group + distro socket paths |
  | clipboard + synthetic paste keypress | needs a key-injection mechanism anyway | same | same | same | inherits the above |
  | xdg-portal RemoteDesktop / libei (`reis` crate, unstable) | no | yes | partial | no | portal consent prompt |

- Candidate crates: `wayland-client` + `wayland-protocols-misc` (protocol
  bindings), `reis` (libei, no stable release), `enigo` (wraps several
  paths; Handy's documented failure point — sessions die after
  lock/sleep, see Handy PR #1395).

## Commands you will need

| Purpose            | Command                                   | Expected on success |
|--------------------|-------------------------------------------|---------------------|
| Check (with examples) | `cargo check --all-targets`            | exit 0              |
| Protocol inventory | `wayland-info \| rg -i 'virtual_keyboard\|input_method\|data_control\|ei'` | list of globals |
| Run a prototype    | `cargo run --example <name>`              | text lands in a focused app |

## Scope

**In scope**:
- `examples/` (new prototype binaries; throwaway quality is fine)
- `Cargo.toml` (`[dev-dependencies]` only)
- `plans/product-direction/spike-insertion-findings.md` (the deliverable)

**Out of scope** (do NOT touch):
- `src/` — no production code in a spike.
- ydotool *integration* — measure it as a user would (install, configure,
  run), but do not write code that shells out to it.

## Steps

### Step 1: Verify the protocol matrix locally

On niri (and any other compositor available — a nested `sway` or GNOME
session in a VM counts), run `wayland-info` and record which of these
globals exist: `zwp_virtual_keyboard_manager_v1`,
`zwp_input_method_manager_v2`, `zwlr_data_control_manager_v1` /
`ext_data_control_manager_v1`. Correct the research matrix where reality
disagrees — especially the "GNOME supports virtual-keyboard" claim, which
contradicts other sources.

**Verify**: the findings doc contains a per-compositor table with actual
`wayland-info` output excerpts.

### Step 2: Prototype `zwp_input_method_v2` commit_string on niri

Build `examples/insert_input_method.rs` with `wayland-client` +
`wayland-protocols-misc`: acquire `zwp_input_method_v2`, wait for an
`activate` (focus a text field), then `commit_string("hello from dictate ")`
+ `commit`. Answer specifically:

- Does text arrive in a GTK app, a terminal (foot/alacritty), and a browser?
- What happens when fcitx5/IBus is also running — who wins the IM seat, and
  does grabbing it break the user's real input method? (This is the known
  design risk of the route.)
- Can the app hold the IM seat passively and only commit during delivery,
  or must it acquire/release around each dictation?

**Verify**: `cargo run --example insert_input_method` → text appears in at
least two app classes; behaviors recorded in the findings doc.

### Step 3: Prototype the virtual-keyboard path where it exists

`examples/insert_virtual_keyboard.rs`: same shape using
`zwp_virtual_keyboard_v1` with a generated keymap (wtype's approach).
Expected to fail on niri (no global) — confirm, and if another compositor
is available, confirm it works there. Record Unicode handling (type a
string with “smart quotes”, an em-dash, and an emoji).

**Verify**: documented pass/fail per compositor in the findings doc.

### Step 4: Measure the user-facing alternatives

Time-boxed (≤1h each), as a user, no code:

- **ydotool**: install, set up the daemon and permissions on this machine,
  and type into a focused app. Record every setup step needed and whether
  non-ASCII survives.
- **clipboard + manual paste** (plan 001's behavior): how bad is the UX gap
  really, for the maintainer's own daily use? This is the honest baseline
  any injection mechanism must beat.

**Verify**: both have a paragraph in the findings doc.

### Step 5: Write the findings and recommendation

`plans/product-direction/spike-insertion-findings.md`, verdict first:

- Recommended primary mechanism for `DeliveryTarget::Insert` on the
  maintainer's setup (niri), the fallback order for other compositors, and
  what Dictate should do when no mechanism is available (degrade to
  clipboard with a clear stderr message — never silent failure, the
  cardinal sin of the surveyed apps).
- Crate choices with maturity notes; explicitly assess whether `enigo`'s
  session-death failure mode (Handy PR #1395) rules it out.
- Open questions for the maintainer (e.g. is conflicting with fcitx5
  acceptable; is an `input`-group requirement acceptable as an opt-in).
- Keep prototypes in `examples/` if they compile under
  `cargo check --all-targets`; delete anything broken.

**Verify**: findings doc exists, leads with a verdict, and every claim has
either a local observation or a cited source.

## Done criteria

- [ ] `spike-insertion-findings.md` exists with verdict, matrix, and open
      questions
- [ ] Step 1 matrix backed by real `wayland-info` output
- [ ] `cargo check --all-targets` → exit 0 (examples compile or were removed)
- [ ] `jj st` shows only `examples/`, `Cargo.toml`, and the findings doc

## STOP conditions

Stop if:

- No mechanism types text into a focused app on niri at all — handback
  immediately; the product fallback (clipboard-only on niri) is a
  maintainer decision.
- Holding `zwp_input_method_v2` breaks the user's real IME with no
  acquire/release workaround — that fork (IM route vs uinput route) is the
  maintainer's call; present both costs in the handback.
- Prototyping requires patching or forking the protocol-binding crates.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- The follow-up implementation plan (insert as a `DeliveryTarget`) should
  be written from the findings doc — do not start it inside this spike.
- Re-run Step 1 when the compositor updates: niri's protocol surface moves
  fast, and GlobalShortcuts/virtual-keyboard support may land later.
- The findings doc should record the date and compositor versions tested —
  it will go stale and someone must be able to tell.
