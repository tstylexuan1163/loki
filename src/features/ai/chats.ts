/**
 * One assistant conversation per terminal tab.
 *
 * The panel used to keep a single conversation for the whole window, while the
 * commands it ran went to whichever tab was in front. Switching tabs half-way
 * left a transcript about one server driving another. Now each tab owns its
 * conversation: switching tabs switches the panel to that tab's, and a turn's
 * events always land in the tab that asked, even if the user has moved on or
 * closed the panel meanwhile.
 *
 * Only one turn runs at a time across all tabs (`runningTab`): the engine has
 * one approval channel, and two servers changing at once is not something to
 * do by accident.
 */
import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { aiApi, conversationApi, type AiEventPayload } from "@/lib/api";
import { useTerminals } from "@/features/terminal/terminals";
import { useApp } from "@/app/store";

export interface ToolCallView {
  id: string;
  name: string;
  args: unknown;
  auto: boolean;
  /** undefined while waiting for the user. */
  result?: { ok: boolean; content: string };
}

/**
 * One block of a turn, kept in the order it happened: the model writes, calls
 * a tool, writes again. Rendering text first and every tool card after it (as
 * the first version did) put the answer above the work that produced it.
 */
export type Part = { kind: "text"; text: string } | { kind: "tool"; call: ToolCallView };

/** Metadata of a file sent with a user message; no content, no local path. */
export interface TurnAttachment {
  name: string;
  size: number;
  sha256: string;
  remotePath: string;
  secret: boolean;
}

/** How the history was fitted to the model for one reply (see `ai/context.rs`). */
export interface TurnContext {
  droppedRounds: number;
  shortenedOutputs: number;
  tokens: number;
  /** The model's context size the history was fitted to. */
  limit: number;
}

export interface Turn {
  role: "user" | "assistant";
  parts: Part[];
  error?: string;
  attachments?: TurnAttachment[];
  context?: TurnContext;
  /** The provider rejected the request as too long and the turn retried. */
  learned?: { tokens: number; exact: boolean };
  /** The user stopped this reply (or closed its tab, or locked). */
  stopped?: boolean;
}

export interface Chat {
  /** The stored conversation's id. */
  id: string;
  title: string;
  createdAt: number;
  /** The host the conversation is about; set when it starts. */
  hostId: string | null;
  hostLabel: string;
  turns: Turn[];
  /** Unsent text in this tab's input. */
  draft: string;
  error: string | null;
  /**
   * Set when a conversation about another host was opened from history in
   * this tab: it is shown, but not continued here, because its commands would
   * run on this tab's server.
   */
  foreign: { hostId: string | null; hostLabel: string } | null;
  /** Provider and model of the last question, saved with the conversation. */
  providerId: string | null;
  model: string | null;
}

/** A conversation id; the panel owns it, the vault just keeps it. */
export function newConversationId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c && "randomUUID" in c) return c.randomUUID();
  return `c-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

export function blankChat(hostId: string | null, hostLabel: string): Chat {
  return {
    id: newConversationId(),
    title: "",
    createdAt: Math.floor(Date.now() / 1000),
    hostId,
    hostLabel,
    turns: [],
    draft: "",
    error: null,
    foreign: null,
    providerId: null,
    model: null,
  };
}

const textOf = (t: Turn) => t.parts.map((p) => (p.kind === "text" ? p.text : "")).join("");

/** Saves a conversation as it stands. Failures stay quiet on purpose: a full
 *  disk must not interrupt a chat, and the next save retries. */
export async function saveChat(chat: Chat) {
  if (chat.turns.length === 0 || chat.foreign) return;
  const firstUser = chat.turns.find((t) => t.role === "user");
  const fallback = firstUser ? textOf(firstUser).trim().slice(0, 30) : "";
  try {
    await conversationApi.save({
      id: chat.id,
      title: chat.title || fallback || "未命名对话",
      hostId: chat.hostId,
      hostLabel: chat.hostLabel,
      providerId: chat.providerId,
      model: chat.model,
      createdAt: chat.createdAt,
      updatedAt: Math.floor(Date.now() / 1000),
      turns: chat.turns,
    });
  } catch {
    /* ignored: see above */
  }
}

/** Applies one engine event to the parts of the turn that is running. */
export function applyEvent(parts: Part[], ev: AiEventPayload): Part[] {
  if (ev.type === "text") {
    const last = parts[parts.length - 1];
    // Consecutive pieces belong to the same paragraph; after a tool call the
    // model is writing a new block.
    if (last?.kind === "text") {
      return [...parts.slice(0, -1), { kind: "text", text: last.text + ev.text }];
    }
    return [...parts, { kind: "text", text: ev.text }];
  }
  if (ev.type === "toolCall") {
    return [
      ...parts,
      { kind: "tool", call: { id: ev.id, name: ev.name, args: ev.arguments, auto: ev.auto } },
    ];
  }
  if (ev.type === "toolResult") {
    return parts.map((p) =>
      p.kind === "tool" && p.call.id === ev.id
        ? { kind: "tool", call: { ...p.call, result: { ok: ev.ok, content: ev.content } } }
        : p,
    );
  }
  return parts;
}

interface ChatsState {
  /** By terminal tab id. */
  chats: Record<string, Chat>;
  /** The tab whose turn is streaming, if any, and that turn's id. */
  runningTab: string | null;
  runningTurn: string | null;
  /** Gives a tab a blank conversation unless it already has one. */
  ensure(tabId: string, hostId: string | null, hostLabel: string): void;
  update(tabId: string, patch: Partial<Chat>): void;
  replace(tabId: string, chat: Chat): void;
  setRunning(tabId: string | null, turnId?: string | null): void;
  /** Stops the running turn, wherever it is. */
  stop(): void;
  /** Renames a conversation wherever it is open. */
  renamed(id: string, title: string): void;
  clear(): void;
}

export const useChats = create<ChatsState>((set, get) => ({
  chats: {},
  runningTab: null,
  runningTurn: null,

  ensure(tabId, hostId, hostLabel) {
    if (get().chats[tabId]) return;
    set({ chats: { ...get().chats, [tabId]: blankChat(hostId, hostLabel) } });
  },

  update(tabId, patch) {
    const cur = get().chats[tabId];
    if (!cur) return;
    set({ chats: { ...get().chats, [tabId]: { ...cur, ...patch } } });
  },

  replace(tabId, chat) {
    set({ chats: { ...get().chats, [tabId]: chat } });
  },

  setRunning(tabId, turnId = null) {
    set({ runningTab: tabId, runningTurn: tabId ? turnId : null });
  },

  stop() {
    const { runningTurn } = get();
    if (!get().runningTab) return;
    // Rust answers with a `stopped` event, which ends the turn here; if it
    // never comes (the turn already ended), the next send replaces it.
    void aiApi.cancel(runningTurn).catch(() => {});
  },

  renamed(id, title) {
    const next = { ...get().chats };
    for (const [tab, c] of Object.entries(next)) if (c.id === id) next[tab] = { ...c, title };
    set({ chats: next });
  },

  clear() {
    set({ chats: {}, runningTab: null, runningTurn: null });
  },
}));

let listening = false;

/**
 * Streams engine events into the tab that asked. Started by the panel the
 * first time it mounts and kept for the life of the app, so closing the panel
 * mid-turn does not drop what the model is still writing.
 */
export function listenForTurns() {
  if (listening) return;
  listening = true;
  void listen<AiEventPayload>("ai-event", (evt) => {
    const ev = evt.payload;
    const { runningTab, runningTurn, chats } = useChats.getState();
    if (!runningTab) return;
    // A late event from a turn that was stopped or replaced is not this one's.
    if (ev.turnId && runningTurn && ev.turnId !== runningTurn) return;
    const finished = ev.type === "done" || ev.type === "error" || ev.type === "stopped";
    const chat = chats[runningTab];
    if (chat) {
      const last = chat.turns[chat.turns.length - 1];
      if (last?.role === "assistant") {
        const updated: Turn =
          ev.type === "error"
            ? { ...last, error: ev.message }
            : ev.type === "context"
              ? {
                  ...last,
                  context: {
                    droppedRounds: ev.droppedRounds,
                    shortenedOutputs: ev.shortenedOutputs,
                    tokens: ev.tokens,
                    limit: ev.limit,
                  },
                }
              : ev.type === "limitLearned"
                ? { ...last, learned: { tokens: ev.tokens, exact: ev.exact } }
                : ev.type === "stopped"
                  ? { ...last, stopped: true }
                  : { ...last, parts: applyEvent(last.parts, ev) };
        const next = { ...chat, turns: [...chat.turns.slice(0, -1), updated] };
        useChats.getState().replace(runningTab, next);
        // A finished turn is saved once more, with every part in place.
        if (finished) void saveChat(next);
      }
    }
    if (finished) useChats.getState().setRunning(null);
  });
}

// A closed tab takes its conversation with it (it is already in the history),
// and stops its turn: left running, a call waiting for an approval nobody can
// give any more would hold the one-turn-at-a-time slot for every other tab.
useTerminals.subscribe((t, prev) => {
  if (t.tabs === prev.tabs) return;
  const open = new Set(t.tabs.map((x) => x.id));
  const { chats, runningTab, runningTurn } = useChats.getState();
  if (runningTab && !open.has(runningTab)) {
    void aiApi.cancel(runningTurn).catch(() => {});
    useChats.getState().setRunning(null);
  }
  const kept = Object.fromEntries(Object.entries(chats).filter(([id]) => open.has(id)));
  if (Object.keys(kept).length !== Object.keys(chats).length) useChats.setState({ chats: kept });
});

// Locking forgets every conversation shown, like the terminals around them.
useApp.subscribe((s, prev) => {
  if (s.phase !== prev.phase && s.phase !== "unlocked") useChats.getState().clear();
});
