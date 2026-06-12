# Spike findings: Wayland text insertion

Tested 2026-06-11 on niri 26.04 (`8ed0da4`), `WAYLAND_DISPLAY=wayland-1`, `XDG_CURRENT_DESKTOP=niri`, `XDG_SESSION_TYPE=wayland`.

## Verdict

Use `zwp_input_method_v2` as Dictate's first insertion mechanism on niri, and fall back to clipboard with a clear stderr message when it does not activate quickly.

Why: it inserted semantic UTF-8 text, including smart quotes, an em dash, and an emoji, into GTK and Chromium text fields. It also reports activation/unavailability, so Dictate can fail loudly instead of pretending text was inserted. The main weakness is app coverage: WezTerm did not activate the input-method object in this test, so terminals and apps that do not use `zwp_text_input_v3` still need a fallback.

Keep `zwp_virtual_keyboard_v1` as an opt-in/key-emission fallback candidate, not the default text insertion path yet. niri advertises it and a prototype can type into Chromium, but it is a keystroke backend with XKB keymap/keycode traps and no acknowledgement from the target app. Smart quotes and em dash survived in the prototype; emoji was mangled to a private-use glyph (``) in Chromium. That is unacceptable for Dictate's default final-text delivery.

Recommended follow-up order for `DeliveryTarget::Insert`:

1. Try `zwp_input_method_v2` and wait briefly for `activate`/`done`.
2. If there is no activation or the compositor lacks the global, copy to clipboard and print a clear message such as `focused app did not accept Wayland text insertion; copied dictation to clipboard`.
3. Consider an explicit future `insert-keyboard`/terminal fallback using `zwp_virtual_keyboard_v1` only after a dedicated plan proves terminal coverage and Unicode behavior.

## Local protocol inventory

The plan asked for `wayland-info`, but `wayland-info` is not installed on this machine. I used the checked-in `examples/list_wayland_globals.rs` prototype to read the same Wayland registry globals.

Relevant niri globals observed:

```text
ext_data_control_manager_v1 v1 name=20
wl_seat v9 name=40
zwlr_data_control_manager_v1 v2 name=19
zwp_input_method_manager_v2 v1 name=24
zwp_text_input_manager_v3 v1 name=23
zwp_virtual_keyboard_manager_v1 v1 name=26
```

Full command used:

```sh
cargo run --example list_wayland_globals
```

No GNOME, KDE, Sway, or Hyprland session was tested locally in this spike.

## Prototype results

| Mechanism | Local result | Notes |
|---|---|---|
| `zwp_input_method_v2` | Works on niri for GTK/Chromium; WezTerm timed out | Best text fidelity; depends on focused app requesting text input |
| `zwp_virtual_keyboard_v1` | Works on niri for Chromium after using a readable XKB keymap fd | Keystroke backend; emoji mangled; no target acknowledgement |
| `wtype` baseline | Works for ASCII in Chromium | Installed locally; uses the same virtual-keyboard protocol |
| uinput (`ydotool`/`dotool`) | Not locally installed or configured | Setup friction is documented upstream: daemon or `/dev/uinput` permissions |
| Clipboard + manual paste | Works | Honest fallback baseline from plan 001; one extra paste action |

### `zwp_input_method_v2`

Prototype: `examples/insert_input_method.rs`.

Observed successes:

- Chromium textarea: `cargo run --example insert_input_method -- 'browser im ok '` changed the page title to `dictate-captured:browser im ok - Chromium`.
- Chromium Unicode: `cargo run --example insert_input_method -- 'unicode “smart” — emoji 😀 '` changed the page title to `dictate-captured:unicode “smart” — emoji 😀 - Chromium`.
- GTK/Zenity entry: the prototype activated and committed `gtk im ok`; submitting the dialog returned that text.
- An accidental live insertion also put `dictate input method test` into the currently focused browser/chat input, proving the route can affect the active workflow and must only be run against the intended focused target.

Observed failure:

- WezTerm running `read` never sent `activate`; the prototype timed out with `timed out before an active text input accepted the input method`.

IME conflict status:

- No `fcitx` or `ibus` process was running locally (`pgrep -a 'fcitx|ibus'` returned no matches), so this spike did not validate coexistence with a real input method.
- The protocol allows no more than one input method object per seat, and inactive requests are accepted but do not affect the next text input. Sources: wlroots `input-method-unstable-v2.xml` documents active/inactive state and the one-IM-per-seat rule, and `commit_string` + `commit` semantics: <https://github.com/swaywm/wlroots/blob/master/protocol/input-method-unstable-v2.xml#L60-L70>, <https://github.com/swaywm/wlroots/blob/master/protocol/input-method-unstable-v2.xml#L211-L228>, <https://github.com/swaywm/wlroots/blob/master/protocol/input-method-unstable-v2.xml#L283-L307>.

Acquire/release behavior:

- Creating the input-method object after Chromium/Zenity were already focused still produced `activate`, so acquire-around-delivery appears viable on niri. Production should avoid holding the IM seat for the daemon lifetime unless coexistence with fcitx5/IBus is deliberately solved.

### `zwp_virtual_keyboard_v1`

Prototype: `examples/insert_virtual_keyboard.rs`.

The prototype only worked after matching wtype's keymap shape and opening the temporary keymap file read-write. A write-only fd caused niri to report `key sent before keymap` because the compositor could not read the keymap.

Observed successes:

- Chromium ASCII: `cargo run --example insert_virtual_keyboard -- 'browser vk ok '` changed the page title to `dictate-captured:browser vk ok - Chromium`.
- Chromium smart quotes/em dash: `unicode “smart” — emoji 😀` preserved `“smart” —`.

Observed failure:

- The same Unicode string rendered the emoji as `` instead of `😀` in Chromium. That makes virtual-keyboard unsuitable as the default semantic text insertion route for Dictate.
- Earlier US-keymap prototypes did not type into Chromium/WezTerm; the custom generated XKB keymap was required.

Implementation notes for any future virtual-keyboard plan:

- Use `wayland-client = "0.31"` and `wayland-protocols-misc = { version = "0.3", features = ["client"] }`.
- Generate a small XKB keymap for the characters to type, wtype-style:
  - protocol key `1` maps to XKB keycode `9`, protocol key `2` to XKB keycode `10`, etc.
  - send protocol key numbers without adding XKB's `+8` offset.
  - keep the keymap fd readable by the compositor.
- wtype's implementation documents the same details: it uses `virtual-keyboard-unstable-v1`, generates `<K1> = 9` style keycodes, sends `time = 0`, and sends key codes directly. Sources: <https://github.com/atx/wtype/blob/master/main.c#L462-L500>, <https://github.com/atx/wtype/blob/master/main.c#L337-L354>.

### uinput alternatives

`ydotool` and `dotool` were not installed locally, and I did not install system packages during this spike.

Upstream docs confirm the setup friction that made this a fallback-only candidate:

- `ydotoold` is mandatory and requires access to `/dev/uinput`, which usually requires root permissions: <https://github.com/ReimuNotMoe/ydotool/blob/master/README.md#L75-L91>.
- `dotool` requires write permission to `/dev/uinput`, commonly via an `input` group and udev rule: <https://github.com/simono41/dotool/blob/master/doc/dotool.1.scd#L16-L30>, <https://github.com/simono41/dotool/blob/master/80-dotool.rules#L1-L2>.

That is too much setup friction for Dictate's default path, but it may still be a future opt-in fallback for compositors without useful Wayland insertion protocols.

### Clipboard baseline

Plan 001's clipboard delivery works locally and is reliable. The UX gap is one manual paste action, but it has two important advantages over a fragile insertion backend: the user can see where text will go before pasting, and failure is obvious. This should be the universal fallback.

## Cross-compositor notes

| Compositor family | Likely insertion path | Evidence level |
|---|---|---|
| niri | `zwp_input_method_v2` first; clipboard fallback; optional future virtual-keyboard fallback | Local live test |
| wlroots/Sway-style | Likely `zwp_input_method_v2` and/or `zwp_virtual_keyboard_v1` | Source docs: wlroots exposes `zwp_input_method_manager_v2`; wtype works where virtual-keyboard is supported |
| GNOME/Mutter | Clipboard fallback unless a GNOME-specific route is designed | Fcitx docs say GNOME uses IBus D-Bus for compositor/input-method, not `zwp_input_method_v2`; wtype maintainer reports Mutter lacking virtual-keyboard support |
| KDE/KWin | Clipboard fallback unless a KDE-specific route is designed | Fcitx docs say KWin uses `zwp_input_method_v1`; Wayland Explorer/wtype reports virtual-keyboard absent in KWin versions surveyed |
| RemoteDesktop/libei portal | Not recommended as default | Enigo/libei is experimental and has prompt/session reliability issues |

Sources:

- wlroots creates the `zwp_input_method_manager_v2` global: <https://github.com/swaywm/wlroots/blob/master/types/wlr_input_method_v2.c#L595-L608>.
- Fcitx Wayland compositor table: Sway uses `zwp_input_method_v2`, GNOME uses IBus D-Bus, KDE uses `zwp_input_method_v1`: <https://fcitx-im.org/wiki/Using_Fcitx_5_on_Wayland#Support_in_Wayland_Compositor>.
- wtype requires `virtual-keyboard-unstable-v1`: <https://github.com/atx/wtype/blob/master/man/wtype.1#L15-L19>.
- wtype maintainer notes Mutter/GNOME lacked virtual-keyboard support: <https://github.com/atx/wtype/issues/22#issuecomment-808920974>.
- Wayland Explorer virtual-keyboard support table marks Mutter/KWin as unsupported in surveyed versions: <https://wayland.app/protocols/virtual-keyboard-unstable-v1#compositor-support>.

## Crate choices

Recommended for the follow-up implementation:

- `wayland-client` for registry/dispatch/event queue.
- `wayland-protocols-misc` for both `zwp_input_method_v2` and `zwp_virtual_keyboard_v1` bindings.
- `xkbcommon` only if a virtual-keyboard fallback is implemented; input-method text insertion does not need it.

Do not use `enigo` as the primary Dictate insertion backend right now. Enigo's README marks Linux Wayland/libei text support as experimental and feature-gated because of bugs, and real-world reports still show portal/libei session problems after lock/suspend/compositor restart. Sources: <https://github.com/enigo-rs/enigo/blob/main/README.md#L12-L18>, <https://github.com/enigo-rs/enigo/blob/main/README.md#L36-L40>, <https://github.com/cjpais/Handy/pull/1395>.

## Open questions for the maintainer

1. Is it acceptable for `DeliveryTarget::Insert` to mean "insert into apps that support Wayland text input; otherwise copy to clipboard"? This is the safest near-term product behavior.
2. Do you use fcitx5/IBus or expect Dictate users to? If yes, we need a specific coexistence test before shipping an input-method backend.
3. Should terminal insertion be a separate opt-in mode? It likely needs virtual-keyboard or uinput and has different failure modes than semantic text insertion.
4. Is a future uinput fallback acceptable if it requires an `input` group/udev/daemon setup, or should Dictate stay zero-setup and clipboard-only when Wayland protocols are unavailable?
