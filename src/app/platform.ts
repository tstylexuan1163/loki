/**
 * Platform differences in keyboard handling.
 *
 * On macOS app shortcuts use ⌘, which a terminal never needs. On Windows and
 * Linux, Ctrl alone belongs to the shell (Ctrl+C, Ctrl+W, Ctrl+K…), so app
 * shortcuts use Ctrl+Shift, as Windows Terminal does.
 */
export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

/** True when the event carries this platform's app-shortcut modifier. */
export function isAppShortcut(e: KeyboardEvent | React.KeyboardEvent): boolean {
  return isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && e.shiftKey && !e.metaKey;
}

/**
 * The shortcut's key, independent of Shift: on Windows the modifier includes
 * Shift, which turns `t` into `T` and `1` into `!`, so digits and brackets are
 * read from the physical key instead.
 */
export function shortcutKey(e: KeyboardEvent | React.KeyboardEvent): string {
  if (/^Digit[0-9]$/.test(e.code)) return e.code.slice(5);
  if (e.code === "BracketLeft") return "[";
  if (e.code === "BracketRight") return "]";
  if (e.code === "Comma") return ",";
  return e.key.toLowerCase();
}

/** A shortcut label for this platform, e.g. `⌘T` or `Ctrl+Shift+T`. */
export function keyLabel(key: string): string {
  return isMac ? `⌘${key}` : `Ctrl+Shift+${key.toUpperCase()}`;
}

/** Marks the document so CSS can leave room for the macOS traffic lights. */
export function applyPlatform() {
  document.documentElement.dataset.platform = isMac ? "mac" : "other";
}
