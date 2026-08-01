# Dictate for Pi

A [Pi coding agent](https://github.com/earendil-works/pi-mono) extension that submits text inserted by [Dictate](https://github.com/joshuadavidthomas/dictate) without a second Enter keypress.

## Install

From a Dictate checkout:

```bash
pi install .
```

Run `/reload` in an open Pi session, or start a new session.

To remove it:

```bash
pi remove .
```

## How it works

Dictate marks its temporary Wayland clipboard offer with the private MIME type `application/x-dictate-clipboard-transaction`. The extension watches Pi's raw terminal input for a completed bracketed paste. It asks `wl-paste --list-types` whether that marker is still present and, when confirmed, places one Enter key directly after the paste terminator.

This check ties auto-submit to both sides of the integration:

- The receiving process must be Pi because only Pi loads the extension.
- The paste must come from Dictate's active clipboard transaction.

Ordinary clipboard pastes remain in the editor. A failed or slow clipboard probe also leaves the text in the editor. Dictate's direct-typing fallback does not auto-submit because it carries no clipboard marker.

## Requirements

- Pi coding agent
- Dictate configured with `delivery = "insert"`
- A terminal that supports bracketed paste
- `wl-paste` from `wl-clipboard`
- A Wayland clipboard protocol supported by Dictate

## Test

```bash
npm test
```
