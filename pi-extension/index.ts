import { spawnSync } from "node:child_process";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { BracketedPasteAutoSubmit } from "./paste.ts";

const DICTATE_TRANSACTION_MIME = "application/x-dictate-clipboard-transaction";
const CLIPBOARD_PROBE_TIMEOUT_MS = 100;

function clipboardHasDictateTransaction(): boolean {
  const result = spawnSync("wl-paste", ["--list-types"], {
    encoding: "utf8",
    timeout: CLIPBOARD_PROBE_TIMEOUT_MS,
  });

  return (
    result.status === 0 &&
    result.stdout
      .split(/\r?\n/u)
      .some((mimeType) => mimeType === DICTATE_TRANSACTION_MIME)
  );
}

export default function (pi: ExtensionAPI): void {
  let unsubscribe: (() => void) | undefined;

  pi.on("session_start", (_event, ctx) => {
    unsubscribe?.();
    unsubscribe = undefined;

    if (ctx.mode !== "tui") return;

    const autoSubmit = new BracketedPasteAutoSubmit();
    unsubscribe = ctx.ui.onTerminalInput((data) => {
      const rewritten = autoSubmit.rewrite(data, clipboardHasDictateTransaction);
      return rewritten === data ? undefined : { data: rewritten };
    });
  });

  pi.on("session_shutdown", () => {
    unsubscribe?.();
    unsubscribe = undefined;
  });
}
