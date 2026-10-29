import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { termBacklog, type TermEvent } from "@/lib/api";
import { usePrefs } from "@/app/prefs";
import { schemeById } from "@/features/terminal/themes";
import { isAppShortcut, isMac, shortcutKey } from "@/app/platform";
import "@xterm/xterm/css/xterm.css";

/** Live xterm instances by tab id, for the find bar and toolbar actions. */
export const terminalRegistry = new Map<
  string,
  { term: Terminal; search: SearchAddon; fit: () => void }
>();

export interface TerminalPaneProps {
  tabId: string;
  /** The backend session; null while connecting. Events for others are ignored. */
  sessionId: string | null;
  /** Only the active pane is visible; the others keep their screen. */
  active: boolean;
  /** Fired when the user types; already UTF-8 encoded. */
  onInput: (data: Uint8Array) => void;
  /** Reports the new size so the remote PTY can follow. */
  onResize: (cols: number, rows: number) => void;
}

function decode(b64: string): Uint8Array {
  const raw = atob(b64);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return bytes;
}

/**
 * The terminal itself.
 *
 * One pane per tab, created once and kept mounted while the tab exists: the
 * screen and scrollback live in xterm, so unmounting on a tab switch (as the
 * first version did) threw the user's history away. Inactive panes are hidden
 * with `visibility`, which keeps their size so `fit` stays correct.
 *
 * It owns its subscription to `term-event` rather than asking a parent to hand
 * it a writer function; output the backend emitted before the subscription
 * existed is replayed from the Rust-side buffer.
 */
export function TerminalPane({ tabId, sessionId, active, onInput, onResize }: TerminalPaneProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  // Set once a session has attached, so only a *re*connect draws a divider.
  const attachedOnce = useRef(false);
  // Callbacks change identity on every render; xterm handlers read the latest.
  const inputRef = useRef(onInput);
  const resizeRef = useRef(onResize);
  inputRef.current = onInput;
  resizeRef.current = onResize;

  // Create the terminal once.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const p = usePrefs.getState();

    const term = new Terminal({
      theme: schemeById(p.terminalScheme).theme,
      fontFamily: p.fontFamily,
      fontSize: p.fontSize,
      lineHeight: p.lineHeight,
      cursorBlink: p.cursorBlink,
      cursorStyle: p.cursorStyle,
      scrollback: p.scrollback,
      allowProposedApi: true,
      macOptionIsMeta: true,
      // Option+click moves the cursor in shells that support it.
      altClickMovesCursor: true,
    });
    const fit = new FitAddon();
    const search = new SearchAddon();
    term.loadAddon(fit);
    term.loadAddon(search);
    // Links open in the system browser, never inside the app's WebView.
    term.loadAddon(new WebLinksAddon((_e, uri) => void openUrl(uri).catch(() => {})));

    term.open(host);
    termRef.current = term;
    fitRef.current = fit;

    const safeFit = () => {
      try {
        fit.fit();
      } catch {
        /* zero-sized while hidden or before layout */
      }
    };
    safeFit();
    terminalRegistry.set(tabId, { term, search, fit: safeFit });

    const encoder = new TextEncoder();
    const dataSub = term.onData((s) => inputRef.current(encoder.encode(s)));
    const binSub = term.onBinary((s) => {
      const bytes = new Uint8Array(s.length);
      for (let i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i) & 0xff;
      inputRef.current(bytes);
    });
    const resizeSub = term.onResize(({ cols, rows }) => resizeRef.current(cols, rows));
    const selSub = term.onSelectionChange(() => {
      if (usePrefs.getState().copyOnSelect && term.hasSelection()) {
        void navigator.clipboard.writeText(term.getSelection()).catch(() => {});
      }
    });

    // The app modifier (⌘, or Ctrl+Shift elsewhere) + C copies a selection
    // instead of sending ^C; + V pastes; + K clears the screen.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown" || !isAppShortcut(e)) return true;
      const key = shortcutKey(e);
      if (key === "c") {
        if (term.hasSelection()) {
          void navigator.clipboard.writeText(term.getSelection()).catch(() => {});
        }
        return false;
      }
      // ⌘V reaches xterm as a native paste event on macOS; Ctrl+Shift+V
      // does not, so read the clipboard ourselves.
      if (key === "v" && !isMac) {
        e.preventDefault();
        void navigator.clipboard
          .readText()
          .then((text) => term.paste(text))
          .catch(() => {});
        return false;
      }
      if (key === "k") {
        // Clear, as in Terminal.app; stop it here so the window-level ⌘K
        // (quick connect) does not also fire.
        e.preventDefault();
        e.stopPropagation();
        term.clear();
        return false;
      }
      // Let app-level shortcuts (tabs, find, ...) through to the window.
      return !["t", "w", "f", "d", "l", "1", "2", "3", "4", "5", "6", "7", "8", "9", "[", "]", ","].includes(
        key,
      );
    });

    const refocus = () => term.focus();
    host.addEventListener("mousedown", refocus);
    const observer = new ResizeObserver(safeFit);
    observer.observe(host);

    return () => {
      host.removeEventListener("mousedown", refocus);
      observer.disconnect();
      dataSub.dispose();
      binSub.dispose();
      resizeSub.dispose();
      selSub.dispose();
      terminalRegistry.delete(tabId);
      term.dispose();
      termRef.current = null;
    };
  }, [tabId]);

  // Attach to the backend session; a reconnect swaps the id and keeps the
  // scrollback, with a divider so old and new output are distinguishable.
  useEffect(() => {
    const term = termRef.current;
    if (!term || !sessionId) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;

    if (attachedOnce.current) {
      term.write("\r\n\x1b[2m──── 重新连接 ────\x1b[0m\r\n");
    }
    attachedOnce.current = true;

    // Fetch the backlog only *after* the listener is attached. The reverse
    // order drops anything the server sends between the two calls, which is
    // exactly when a login banner arrives.
    void listen<TermEvent>("term-event", (evt) => {
      const ev = evt.payload;
      if (ev.sessionId !== sessionId) return;
      if (ev.type === "data") term.write(decode(ev.data));
      else if (ev.type === "exit") term.write(`\r\n\x1b[2m[进程已退出，状态码 ${ev.code}]\x1b[0m`);
      else if (ev.type === "closed") term.write("\r\n\x1b[2m[连接已关闭]\x1b[0m\r\n");
    }).then((un) => {
      if (disposed) {
        un();
        return;
      }
      unlisten = un;
      void termBacklog(sessionId)
        .then((b64) => {
          if (b64 && !disposed) term.write(decode(b64));
        })
        .catch(() => {
          /* the session may already be gone; the closed event explains it */
        });
    });

    // The PTY was opened at the last size any pane measured; tell the server
    // this pane's real size now that there is a session to tell.
    resizeRef.current(term.cols, term.rows);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [sessionId]);

  // Becoming visible: re-measure and take the keyboard.
  useEffect(() => {
    if (!active) return;
    const id = window.requestAnimationFrame(() => {
      try {
        fitRef.current?.fit();
      } catch {
        /* ignore */
      }
      // Do not steal the caret from a field the user is typing into, such
      // as the password prompt drawn over this pane.
      const el = document.activeElement;
      const typing =
        el instanceof HTMLElement &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA") &&
        !hostRef.current?.contains(el);
      if (!typing) termRef.current?.focus();
    });
    return () => window.cancelAnimationFrame(id);
  }, [active]);

  // Apply preference changes to the running terminal without recreating it.
  useEffect(
    () =>
      usePrefs.subscribe((p) => {
        const term = termRef.current;
        if (!term) return;
        term.options.theme = schemeById(p.terminalScheme).theme;
        term.options.fontSize = p.fontSize;
        term.options.fontFamily = p.fontFamily;
        term.options.lineHeight = p.lineHeight;
        term.options.cursorStyle = p.cursorStyle;
        term.options.cursorBlink = p.cursorBlink;
        term.options.scrollback = p.scrollback;
        try {
          fitRef.current?.fit();
        } catch {
          /* ignore */
        }
      }),
    [],
  );

  return (
    <div
      ref={hostRef}
      data-tab={tabId}
      className={`xterm-host ${active ? "" : "xterm-host--hidden"}`}
    />
  );
}
