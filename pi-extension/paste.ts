const PASTE_START = "\u001b[200~";
const PASTE_END = "\u001b[201~";
const TOKEN_TAIL_LENGTH = Math.max(PASTE_START.length, PASTE_END.length) - 1;

export class BracketedPasteAutoSubmit {
  private pasteOpen = false;
  private tail = "";

  rewrite(data: string, shouldSubmit: () => boolean): string {
    const priorTailLength = this.tail.length;
    const combined = this.tail + data;
    const submitOffsets: number[] = [];
    let cursor = 0;

    while (cursor < combined.length) {
      const token = this.pasteOpen ? PASTE_END : PASTE_START;
      const tokenOffset = combined.indexOf(token, cursor);
      if (tokenOffset === -1) break;

      cursor = tokenOffset + token.length;
      if (this.pasteOpen) {
        this.pasteOpen = false;
        const dataOffset = cursor - priorTailLength;
        if (dataOffset >= 0 && shouldSubmit()) submitOffsets.push(dataOffset);
      } else {
        this.pasteOpen = true;
      }
    }

    this.tail = combined.slice(-TOKEN_TAIL_LENGTH);
    if (submitOffsets.length === 0) return data;

    let rewritten = "";
    let copiedThrough = 0;
    for (const offset of submitOffsets) {
      rewritten += data.slice(copiedThrough, offset) + "\r";
      copiedThrough = offset;
    }
    return rewritten + data.slice(copiedThrough);
  }
}
