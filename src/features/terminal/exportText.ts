import type { Terminal } from "@xterm/xterm";

/**
 * The whole scrollback as plain text.
 *
 * `translateToString(true)` trims each row's trailing blanks, which is right for
 * a file. A row flagged `isWrapped` is a continuation of the previous logical
 * line (the terminal broke it for width, not the program), so it is appended
 * rather than started on a new line. Trailing blank rows are dropped.
 */
export function bufferToText(term: Terminal): string {
  const buffer = term.buffer.active;
  const logical: string[] = [];

  for (let i = 0; i < buffer.length; i++) {
    const line = buffer.getLine(i);
    if (!line) continue;
    const text = line.translateToString(true);
    if (line.isWrapped && logical.length > 0) {
      logical[logical.length - 1] += text;
    } else {
      logical.push(text);
    }
  }

  while (logical.length > 0 && logical[logical.length - 1].trim() === "") logical.pop();
  return logical.join("\n");
}

/** `20261029-2250`, local time, for suggested file names. */
export function fileStamp(d: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(
    d.getMinutes(),
  )}`;
}

/** File-name-safe version of a tab title. */
export function safeStem(raw: string): string {
  const cleaned = raw
    .replace(/[\\/:*?"<>|]/g, "-")
    .replace(/\s+/g, "-")
    .replace(/^-+|-+$/g, "");
  return cleaned.slice(0, 60) || "loki";
}
