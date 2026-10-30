import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import {
  directApi,
  errorMessage,
  sessionApi,
  termApi,
  type ConnectResult,
  type HostView,
  type OpenInput,
  type QuickTarget,
  type TermEvent,
} from "@/lib/api";

export type TermStatus = "connecting" | "connected" | "failed" | "closed";
export type TabKind = "ssh" | "telnet" | "serial" | "local";

/**
 * Connection parameters for a tab that has no saved host: a quick-connect
 * telnet or serial target. Kept on the tab so reconnect works without asking
 * again, exactly like `QuickTarget` does for SSH.
 */
export type DirectTarget =
  | { kind: "telnet"; host: string; port: number }
  | {
      kind: "serial";
      path: string;
      baud: number;
      dataBits: number;
      parity: string;
      stopBits: number;
    };

/** Something the user must answer before the connection can continue. */
export type TabPrompt =
  | {
      type: "hostKey";
      fingerprint: string;
      keyType: string;
      host: string;
      port: number;
      /** Present when the key changed for a host we already knew. */
      previous: string | null;
    }
  | { type: "password"; host: string; username: string; retry: boolean; reason?: string }
  /** The host was saved without a login name. */
  | { type: "username"; host: string };

export interface TermTab {
  id: string;
  title: string;
  /** Which backend drives this tab; telnet, serial and local share one. */
  kind: TabKind;
  hostId: string | null;
  target: QuickTarget | null;
  /** Set for unsaved telnet/serial tabs; `target` is the SSH equivalent. */
  direct: DirectTarget | null;
  status: TermStatus;
  /** Backend session id once connected. */
  sessionId: string | null;
  error: string | null;
  prompt: TabPrompt | null;
  connectedAt: number | null;
  /** Non-null while this session is being recorded to a file. */
  recording: import("@/lib/api").RecordInfo | null;
  /** Login name answered at the prompt; reused for retries and reconnects. */
  login: { username: string; remember: boolean } | null;
}

interface TermState {
  tabs: TermTab[];
  active: string | null;

  openHost(host: HostView): string;
  openTarget(target: QuickTarget): string;
  openTelnet(target: { host: string; port: number }): string;
  openSerial(opts: Omit<Extract<DirectTarget, { kind: "serial" }>, "kind">): string;
  openLocal(): string;
  acceptHostKey(tabId: string): void;
  submitPassword(tabId: string, password: string, remember: boolean): void;
  submitUsername(tabId: string, username: string, remember: boolean): void;
  cancelPrompt(tabId: string): void;
  reconnect(tabId: string): void;
  sendInput(tabId: string, data: Uint8Array): void;
  sendText(tabId: string, text: string): void;
  resize(tabId: string, cols: number, rows: number): void;
  closeTab(tabId: string): Promise<void>;
  closeOthers(tabId: string): void;
  activate(tabId: string): void;
  cycle(delta: number): void;
  rename(tabId: string, title: string): void;
  setRecording(tabId: string, info: import("@/lib/api").RecordInfo | null): void;
  /** Drops every tab without talking to the backend (it already closed them). */
  reset(): void;
}

/** The latest size any pane measured; new connections start at it. */
let lastSize = { cols: 120, rows: 32 };
export const noteSize = (cols: number, rows: number) => {
  lastSize = { cols, rows };
};

/** `/dev/tty.usbserial-1420` is too long for a tab; keep the last segment. */
export function serialLabel(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

let subscribed = false;
const newId = () => `tab-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

export const useTerminals = create<TermState>((set, get) => {
  const patch = (tabId: string, p: Partial<TermTab>) =>
    set({ tabs: get().tabs.map((t) => (t.id === tabId ? { ...t, ...p } : t)) });

  async function subscribe() {
    if (subscribed) return;
    subscribed = true;
    await listen<TermEvent>("term-event", (evt) => {
      const ev = evt.payload;
      // Data frames are consumed by TerminalPane directly; the store only
      // tracks lifecycle so the tab strip stays accurate.
      if (ev.type !== "closed") return;
      const tab = get().tabs.find((t) => t.sessionId === ev.sessionId);
      // The file is already closed by the Rust side; drop the indicator.
      if (tab) patch(tab.id, { status: "closed", recording: null });
    });
  }

  /** Runs one SSH attempt and turns the result into tab state. */
  async function attempt(tabId: string, extra: Partial<OpenInput> = {}) {
    const tab = get().tabs.find((t) => t.id === tabId);
    if (!tab) return;
    const prevPrompt = tab.prompt;
    patch(tabId, { status: "connecting", error: null, prompt: null });
    try {
      await subscribe();
      const res: ConnectResult = await sessionApi.open({
        hostId: tab.hostId,
        target: tab.target,
        cols: lastSize.cols,
        rows: lastSize.rows,
        username: tab.login?.username ?? null,
        rememberUsername: tab.login?.remember ?? false,
        ...extra,
      });
      // The tab may have been closed while connecting.
      if (!get().tabs.some((t) => t.id === tabId)) {
        if (res.session) void sessionApi.close(res.session.id).catch(() => {});
        return;
      }
      if (res.kind === "hostKey") {
        patch(tabId, {
          status: "connecting",
          prompt: {
            type: "hostKey",
            fingerprint: res.fingerprint ?? "",
            keyType: res.keyType ?? "",
            host: res.host ?? "",
            port: res.port ?? 22,
            previous: res.previous,
          },
        });
      } else if (res.kind === "needUsername") {
        patch(tabId, { status: "connecting", prompt: { type: "username", host: res.host ?? "" } });
      } else if (res.kind === "needPassword") {
        patch(tabId, {
          status: "connecting",
          prompt: {
            type: "password",
            host: res.host ?? "",
            username: res.username ?? "",
            retry: !!extra.password,
          },
        });
      } else {
        patch(tabId, {
          status: "connected",
          sessionId: res.session?.id ?? null,
          connectedAt: Date.now(),
          // Saved once is enough; reconnects reuse the name without rewriting.
          login: tab.login ? { ...tab.login, remember: false } : null,
        });
      }
    } catch (e) {
      const message = errorMessage(e);
      // A typed password that the server refused: ask again rather than
      // leaving the user at a dead end.
      if (extra.password && /密码被拒绝|认证失败/.test(message)) {
        patch(tabId, {
          status: "connecting",
          prompt: {
            type: "password",
            host: extra.passwordHost ?? "",
            username:
              prevPrompt?.type === "password" ? prevPrompt.username : (tab.target?.username ?? ""),
            retry: true,
            // Say what the server said when it was more than "wrong password",
            // e.g. too many attempts from the agent's keys first.
            reason: /密码被拒绝/.test(message) ? undefined : message,
          },
        });
        return;
      }
      patch(tabId, { status: "failed", error: message });
    }
  }

  async function openDirect(tabId: string) {
    const tab = get().tabs.find((t) => t.id === tabId);
    if (!tab) return;
    patch(tabId, { status: "connecting", error: null });
    try {
      await subscribe();
      if (tab.kind === "local") {
        const info = await sessionApi.local(lastSize.cols, lastSize.rows);
        patch(tabId, {
          status: "connected",
          sessionId: info.id,
          connectedAt: Date.now(),
          title: tab.title || info.target,
        });
      } else if (tab.direct) {
        // Unsaved quick-connect target: the parameters live on the tab.
        const info =
          tab.direct.kind === "telnet"
            ? await directApi.telnetOpen({
                host: tab.direct.host,
                port: tab.direct.port,
                cols: lastSize.cols,
                rows: lastSize.rows,
              })
            : await directApi.serialOpen({
                path: tab.direct.path,
                baud: tab.direct.baud,
                dataBits: tab.direct.dataBits,
                parity: tab.direct.parity,
                stopBits: tab.direct.stopBits,
              });
        patch(tabId, {
          status: "connected",
          sessionId: info.id,
          connectedAt: Date.now(),
        });
      } else {
        // Saved telnet/serial hosts go through session_open, which dispatches
        // on the stored protocol.
        const res = await sessionApi.open({
          hostId: tab.hostId,
          cols: lastSize.cols,
          rows: lastSize.rows,
        });
        patch(tabId, {
          status: "connected",
          sessionId: res.session?.id ?? null,
          connectedAt: Date.now(),
        });
      }
    } catch (e) {
      patch(tabId, { status: "failed", error: errorMessage(e) });
    }
  }

  function add(
    tab: Omit<
      TermTab,
      "id" | "status" | "sessionId" | "error" | "prompt" | "connectedAt" | "recording" | "login"
    >,
  ) {
    const full: TermTab = {
      ...tab,
      id: newId(),
      status: "connecting",
      sessionId: null,
      error: null,
      prompt: null,
      connectedAt: null,
      recording: null,
      login: null,
    };
    set({ tabs: [...get().tabs, full], active: full.id });
    return full.id;
  }

  function closeBackend(tab: TermTab) {
    if (!tab.sessionId) return;
    const call =
      tab.kind === "ssh" ? sessionApi.close(tab.sessionId) : directApi.close(tab.sessionId);
    void call.catch(() => {});
  }

  return {
    tabs: [],
    active: null,

    openHost(host) {
      const title =
        host.name ||
        (host.protocol === "serial" ? host.address : `${host.username}@${host.address}`);
      const id = add({ title, kind: host.protocol, hostId: host.id, target: null, direct: null });
      if (host.protocol === "ssh") void attempt(id);
      else void openDirect(id);
      return id;
    },

    openTarget(target) {
      const id = add({
        title: `${target.username}@${target.address}`,
        kind: "ssh",
        hostId: null,
        target,
        direct: null,
      });
      void attempt(id);
      return id;
    },

    openTelnet({ host, port }) {
      const id = add({
        title: `${host}:${port}`,
        kind: "telnet",
        hostId: null,
        target: null,
        direct: { kind: "telnet", host, port },
      });
      void openDirect(id);
      return id;
    },

    openSerial(opts) {
      const id = add({
        title: `${serialLabel(opts.path)} · ${opts.baud}`,
        kind: "serial",
        hostId: null,
        target: null,
        direct: { kind: "serial", ...opts },
      });
      void openDirect(id);
      return id;
    },

    openLocal() {
      const id = add({ title: "本地终端", kind: "local", hostId: null, target: null, direct: null });
      void openDirect(id);
      return id;
    },

    acceptHostKey(tabId) {
      const tab = get().tabs.find((t) => t.id === tabId);
      if (tab?.prompt?.type !== "hostKey") return;
      void attempt(tabId, { accept: tab.prompt.fingerprint });
    },

    submitPassword(tabId, password, remember) {
      const tab = get().tabs.find((t) => t.id === tabId);
      if (tab?.prompt?.type !== "password") return;
      void attempt(tabId, {
        password,
        passwordHost: tab.prompt.host || null,
        rememberPassword: remember && !!tab.hostId,
      });
    },

    submitUsername(tabId, username, remember) {
      const tab = get().tabs.find((t) => t.id === tabId);
      if (tab?.prompt?.type !== "username" || !username.trim()) return;
      // `attempt` reads the login from the tab, so set it first.
      patch(tabId, { login: { username: username.trim(), remember: remember && !!tab.hostId } });
      void attempt(tabId);
    },

    cancelPrompt(tabId) {
      patch(tabId, { prompt: null, status: "failed", error: "已取消连接" });
    },

    reconnect(tabId) {
      const tab = get().tabs.find((t) => t.id === tabId);
      if (!tab) return;
      closeBackend(tab);
      patch(tabId, { sessionId: null });
      if (tab.kind === "ssh") void attempt(tabId);
      else void openDirect(tabId);
    },

    sendInput(tabId, data) {
      const tab = get().tabs.find((t) => t.id === tabId);
      if (!tab?.sessionId || tab.status !== "connected") return;
      let binary = "";
      for (const b of data) binary += String.fromCharCode(b);
      const encoded = btoa(binary);
      const send =
        tab.kind === "ssh"
          ? sessionApi.write(tab.sessionId, encoded)
          : directApi.write(tab.sessionId, encoded);
      void send.catch(() => {
        // A failed write means the channel is gone; the closed event will follow.
      });
    },

    sendText(tabId, text) {
      get().sendInput(tabId, new TextEncoder().encode(text));
    },

    resize(tabId, cols, rows) {
      noteSize(cols, rows);
      const tab = get().tabs.find((t) => t.id === tabId);
      if (!tab?.sessionId || tab.status !== "connected") return;
      const call =
        tab.kind === "ssh"
          ? termApi.resize(tab.sessionId, cols, rows)
          : directApi.resize(tab.sessionId, cols, rows);
      void call.catch(() => {});
    },

    async closeTab(tabId) {
      const tabs = get().tabs;
      const idx = tabs.findIndex((t) => t.id === tabId);
      if (idx < 0) return;
      closeBackend(tabs[idx]);
      const rest = tabs.filter((t) => t.id !== tabId);
      // Activate the neighbour, as browsers do, rather than jumping to the end.
      const nextActive =
        get().active === tabId ? (rest[Math.min(idx, rest.length - 1)]?.id ?? null) : get().active;
      set({ tabs: rest, active: nextActive });
    },

    closeOthers(tabId) {
      for (const t of get().tabs) if (t.id !== tabId) closeBackend(t);
      set({ tabs: get().tabs.filter((t) => t.id === tabId), active: tabId });
    },

    activate(tabId) {
      set({ active: tabId });
    },

    cycle(delta) {
      const { tabs, active } = get();
      if (tabs.length < 2) return;
      const idx = tabs.findIndex((t) => t.id === active);
      const next = (idx + delta + tabs.length) % tabs.length;
      set({ active: tabs[next].id });
    },

    rename(tabId, title) {
      if (title.trim()) patch(tabId, { title: title.trim() });
    },

    setRecording(tabId, info) {
      patch(tabId, { recording: info });
    },

    reset() {
      set({ tabs: [], active: null });
    },
  };
});
