import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  aiApi,
  attachmentApi,
  conversationApi,
  errorMessage,
  type AiEvent,
  type ChatMessage,
  type AttachmentView,
  type PermissionMode,
  type ProviderView,
} from "@/lib/api";
import { confirmDialog, toast } from "@/components/ui";
import { useApp } from "@/app/store";
import { ConversationHistory } from "@/features/ai/ConversationHistory";
import { useAiComposer } from "@/features/ai/aiComposer";
import {
  IconAlert,
  IconCheck,
  IconChevronRight,
  IconClock,
  IconSend,
  IconFile,
  IconLock,
  IconPaperclip,
  IconPlus,
  IconSparkle,
  IconX,
  IconCopy,
} from "@/components/icons";
import { Markdown } from "@/features/ai/Markdown";

interface ToolCallView {
  id: string;
  name: string;
  args: unknown;
  auto: boolean;
  /** undefined while waiting for the user. */
  result?: { ok: boolean; content: string };
  /** Set when the model produced this call, so the history round-trips. */
}

/**
 * One block of a turn, kept in the order it happened: the model writes, calls
 * a tool, writes again. Rendering text first and every tool card after it (as
 * the first version did) put the answer above the work that produced it.
 */
type Part = { kind: "text"; text: string } | { kind: "tool"; call: ToolCallView };

/** Metadata of a file sent with a user message; no content, no local path. */
interface TurnAttachment {
  name: string;
  size: number;
  sha256: string;
  remotePath: string;
  secret: boolean;
}

interface Turn {
  role: "user" | "assistant";
  parts: Part[];
  error?: string;
  attachments?: TurnAttachment[];
}

/**
 * Reminds the model, in later turns, which files an earlier message carried.
 * Paths only: the content (if it was shared) went once, built by Rust, and a
 * key never goes at all.
 */
function attachmentNote(list: TurnAttachment[]): string {
  const lines = list.map((a, i) =>
    a.secret
      ? `#${i + 1} ${a.name}，含私钥：内容不提供，只能用路径操作（${a.remotePath}）`
      : `#${i + 1} ${a.name}，${formatSize(a.size)}，路径 ${a.remotePath}`,
  );
  return `\n\n附件（已上传到服务器）：\n${lines.join("\n")}`;
}

/** Applies one engine event to the parts of the turn that is running. */
function applyEvent(parts: Part[], ev: AiEvent): Part[] {
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

/**
 * Rebuilds the message list the engine produced, step by step: each step is
 * the text the model wrote plus the tools it called, followed by their results.
 * A text block after a tool call starts the next step.
 */
function turnToMessages(parts: Part[]): ChatMessage[] {
  const out: ChatMessage[] = [];
  let step: { text: string; calls: ToolCallView[] } | null = null;
  const flush = () => {
    if (!step) return;
    const done = step.calls.filter((c) => c.result);
    if (step.text || done.length) {
      out.push({
        role: "assistant",
        content: step.text,
        // The engine echoes the exact arguments back to the model.
        toolCalls: done.map((c) => ({ id: c.id, name: c.name, arguments: c.args })),
      });
      for (const c of done) out.push({ role: "tool", content: c.result!.content, toolCallId: c.id });
    }
    step = null;
  };
  for (const p of parts) {
    if (p.kind === "text") {
      if (step && step.calls.length) flush();
      step ??= { text: "", calls: [] };
      step.text += p.text;
    } else {
      step ??= { text: "", calls: [] };
      step.calls.push(p.call);
    }
  }
  flush();
  return out;
}

/** A conversation id; the panel owns it, the store just keeps it. */
function newConversationId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c && "randomUUID" in c) return c.randomUUID();
  return `c-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

/** `2.1 KB` in the bubble, matching the card. */
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

const textOf = (t: Turn) =>
  t.parts.map((p) => (p.kind === "text" ? p.text : "")).join("");

/**
 * The assistant panel beside the terminal.
 *
 * The conversation lives here; the turn runs in Rust and streams events. A
 * gated tool call blocks in the engine until this panel sends its approval, so
 * closing the panel mid-turn reads as a refusal rather than a silent execution.
 */
export function AiPanel({
  sessionId,
  hostId,
  hostLabel,
  onClose,
}: {
  sessionId: string | null;
  /** Host this tab is bound to, so a conversation can be listed per host. */
  hostId?: string | null;
  hostLabel?: string;
  onClose: () => void;
}) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [draft, setDraft] = useState("");
  const [mode, setMode] = useState<PermissionMode>("manual");
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [providerId, setProviderId] = useState<string | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [conversationId, setConversationId] = useState(newConversationId);
  // Files attached to the message being composed, and everything uploaded per
  // session (for the clean-up row). The panel outlives tab switches, so both
  // are tied to a session: a path staged on one server is wrong on another.
  const [pending, setPending] = useState<AttachmentView[]>([]);
  const [shareContent, setShareContent] = useState<Record<string, boolean>>({});
  const [uploads, setUploads] = useState<{ id: string; name: string; percent: number }[]>([]);
  const [filesBySession, setFilesBySession] = useState<
    Record<string, { id: string; name: string }[]>
  >({});
  const sessionFiles = (sessionId && filesBySession[sessionId]) || [];
  const pendingRef = useRef(pending);
  useEffect(() => {
    pendingRef.current = pending;
  }, [pending]);

  // Switching to another tab drops what was about to be sent: those files sit
  // on the previous server. They were never described to the model, so Rust
  // deletes the uploaded copies too.
  useEffect(() => {
    const stale = pendingRef.current;
    if (stale.length === 0) return;
    setPending([]);
    setShareContent({});
    for (const a of stale) void attachmentApi.detach(a.id).catch(() => {});
    setFilesBySession((prev) => {
      const next: typeof prev = {};
      const gone = new Set(stale.map((a) => a.id));
      for (const [sid, list] of Object.entries(prev)) next[sid] = list.filter((f) => !gone.has(f.id));
      return next;
    });
    toast("已切换会话，未发送的附件已移除", "info");
  }, [sessionId]);
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const pendingInsert = useAiComposer((s) => s.pendingInsert);
  const setPendingInsert = useAiComposer((s) => s.setPendingInsert);

  // Text handed over by the files panel ("send to assistant").
  useEffect(() => {
    if (!pendingInsert) return;
    setDraft((cur) => (cur ? `${cur} ${pendingInsert}` : pendingInsert));
    setPendingInsert(null);
    inputRef.current?.focus();
  }, [pendingInsert, setPendingInsert]);
  const [title, setTitle] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const createdAt = useRef(Math.floor(Date.now() / 1000));
  const logRef = useRef<HTMLDivElement>(null);
  // The host a conversation belongs to, captured when it is created: switching
  // tabs mid-conversation must not silently rebind the transcript.
  const boundHost = useRef({ id: hostId ?? null, label: hostLabel ?? "" });
  const turnsRef = useRef<Turn[]>([]);
  useEffect(() => {
    turnsRef.current = turns;
  }, [turns]);

  useEffect(() => {
    void (async () => {
      try {
        const list = await aiApi.providers();
        setProviders(list);
        const first = list[0];
        setProviderId((cur) => cur ?? first?.id ?? null);
        setModel((cur) => cur ?? first?.models[0] ?? null);
      } catch (e) {
        setError(errorMessage(e));
      }
    })();
  }, []);

  // Follow the stream while the reader is at the bottom; leave them alone if
  // they scrolled up to read something earlier.
  const pinned = useRef(true);
  useEffect(() => {
    const el = logRef.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [turns]);

  const activeModelOptions = providers.find((p) => p.id === providerId)?.models ?? [];

  // Stream the engine's events into the last assistant turn.
  useEffect(() => {
    let disposed = false;
    let un: (() => void) | undefined;

    void listen<AiEvent>("ai-event", (evt) => {
      const ev = evt.payload;
      setTurns((prev) => {
        const last = prev[prev.length - 1];
        if (!last || last.role !== "assistant") return prev;
        const updated: Turn =
          ev.type === "error" ? { ...last, error: ev.message } : { ...last, parts: applyEvent(last.parts, ev) };
        return [...prev.slice(0, -1), updated];
      });

      if (ev.type === "done" || ev.type === "error") setRunning(false);
    }).then((f) => {
      if (disposed) f();
      else un = f;
    });

    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  /** Saves the conversation as it stands. Failures stay quiet on purpose: a
   *  full disk must not interrupt a chat, and the next save retries. */
  const persist = useCallback(
    async (snapshot?: Turn[]) => {
      const list = snapshot ?? turnsRef.current;
      if (list.length === 0) return;
      const firstUser = list.find((t) => t.role === "user");
      const fallback = firstUser ? textOf(firstUser).trim().slice(0, 30) : "";
      try {
        await conversationApi.save({
          id: conversationId,
          title: title || fallback || "未命名对话",
          hostId: boundHost.current.id,
          hostLabel: boundHost.current.label,
          providerId,
          model,
          createdAt: createdAt.current,
          updatedAt: Math.floor(Date.now() / 1000),
          turns: list,
        });
      } catch {
        /* ignored: see above */
      }
    },
    [conversationId, title, providerId, model],
  );
  const persistRef = useRef(persist);
  useEffect(() => {
    persistRef.current = persist;
  }, [persist]);

  // A finished turn is saved once more, when the parts have all arrived.
  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !running) void persistRef.current();
    wasRunning.current = running;
  }, [running]);

  // Locking the vault clears the panel, like the terminals around it.
  const phase = useApp((s) => s.phase);
  useEffect(() => {
    if (phase === "unlocked") return;
    setTurns([]);
    setConversationId(newConversationId());
    setTitle("");
    createdAt.current = Math.floor(Date.now() / 1000);
    setHistoryOpen(false);
    // Rust forgot the attachments when the vault locked; cards left here would
    // be dropped silently on the next send.
    setPending([]);
    setShareContent({});
    setFilesBySession({});
  }, [phase]);

  /** Replaces the panel contents with a stored conversation. */
  function openConversation(conv: Record<string, unknown>) {
    const next = Array.isArray(conv.turns) ? (conv.turns as Turn[]) : [];
    setTurns(next);
    setConversationId(String(conv.id));
    setTitle(typeof conv.title === "string" ? conv.title : "");
    boundHost.current = {
      id: typeof conv.hostId === "string" ? conv.hostId : null,
      label: typeof conv.hostLabel === "string" ? conv.hostLabel : "",
    };
    createdAt.current =
      typeof conv.createdAt === "number" ? conv.createdAt : Math.floor(Date.now() / 1000);
    if (typeof conv.providerId === "string") setProviderId(conv.providerId);
    if (typeof conv.model === "string") setModel(conv.model);
    setHistoryOpen(false);
    pinned.current = true;
  }

  function newConversation() {
    if (running) return;
    setTurns([]);
    setConversationId(newConversationId());
    setTitle("");
    createdAt.current = Math.floor(Date.now() / 1000);
    boundHost.current = { id: hostId ?? null, label: hostLabel ?? "" };
    setError(null);
  }


  // Progress for an upload in flight. At 100% the server is still checking
  // the hash; the card goes away once Rust says the file is done.
  useEffect(() => {
    let disposed = false;
    let un: (() => void) | undefined;
    void listen<{ id: string; name: string; percent: number; done: boolean }>("attach-progress", (e) => {
      const p = e.payload;
      setUploads((prev) => {
        const rest = prev.filter((u) => u.id !== p.id);
        return p.done ? rest : [...rest, { id: p.id, name: p.name, percent: p.percent }];
      });
    }).then((f) => {
      if (disposed) f();
      else un = f;
    });
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  /** Uploads the chosen files to this session's staging folder. */
  async function attach(paths: string[]) {
    if (!sessionId) {
      toast("仅 SSH 连接支持附件", "info");
      return;
    }
    if (paths.length === 0) return;
    try {
      const views = await attachmentApi.attach(sessionId, paths);
      setPending((prev) => [...prev, ...views]);
      setShareContent((prev) => {
        const next = { ...prev };
        // Text files default to "let the model see it"; a key never does.
        for (const v of views) if (v.shareable) next[v.id] = true;
        return next;
      });
      setFilesBySession((prev) => ({
        ...prev,
        [sessionId]: [...(prev[sessionId] ?? []), ...views.map((v) => ({ id: v.id, name: v.name }))],
      }));
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setUploads([]);
    }
  }

  async function pickFiles() {
    if (!sessionId) {
      toast("仅 SSH 连接支持附件", "info");
      return;
    }
    try {
      const picked = await openFileDialog({
        multiple: true,
        title: "选择要上传到服务器的文件",
      });
      const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
      if (list.length) void attach(list as string[]);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  async function removePending(a: AttachmentView) {
    setPending((prev) => prev.filter((x) => x.id !== a.id));
    if (sessionId) {
      setFilesBySession((prev) => ({
        ...prev,
        [sessionId]: (prev[sessionId] ?? []).filter((x) => x.id !== a.id),
      }));
    }
    try {
      await attachmentApi.detach(a.id);
    } catch {
      /* the file is staging-only; a failed delete is not worth an error box */
    }
  }

  async function cleanupStaging() {
    if (!sessionId || sessionFiles.length === 0) return;
    const ok = await confirmDialog({
      title: "清理暂存文件",
      body: `将从服务器删除本会话上传的 ${sessionFiles.length} 个暂存文件。已经用过的文件也会删除，模型之后无法再引用它们的路径。`,
      confirmLabel: "删除",
      danger: true,
    });
    if (!ok) return;
    try {
      const n = await attachmentApi.cleanup(sessionId);
      setFilesBySession((prev) => ({ ...prev, [sessionId]: [] }));
      setPending([]);
      toast(`已删除 ${n} 个文件`, "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  // Files dragged from Finder onto the panel are attached. The files panel has
  // its own handler; only one of the two is mounted at a time.
  useEffect(() => {
    let un: (() => void) | undefined;
    let disposed = false;
    const inside = (pos: { x: number; y: number }) => {
      const r = rootRef.current?.getBoundingClientRect();
      if (!r) return false;
      const x = pos.x / window.devicePixelRatio;
      const y = pos.y / window.devicePixelRatio;
      return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
    };
    void getCurrentWebview()
      .onDragDropEvent((evt) => {
        const p = evt.payload;
        if (p.type === "drop" && inside(p.position) && p.paths.length) void attach(p.paths);
      })
      .then((f) => {
        if (disposed) f();
        else un = f;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      un?.();
    };
  }, [sessionId]);

  const history = useCallback((): ChatMessage[] => {
    const out: ChatMessage[] = [];
    for (const t of turns) {
      if (t.role === "user") {
        const note = t.attachments?.length ? attachmentNote(t.attachments) : "";
        out.push({ role: "user", content: textOf(t) + note });
      }
      else out.push(...turnToMessages(t.parts));
    }
    return out;
  }, [turns]);

  async function send() {
    const text = draft.trim();
    if (!text || running) return;
    if (!providerId) {
      setError("先在设置里添加一个模型服务商");
      return;
    }

    // Attachments travel as ids; Rust owns the bytes and builds the block it
    // shows the model. The bubble keeps only metadata.
    const sendList = pending.map((a) => ({
      id: a.id,
      shareContent: a.shareable ? (shareContent[a.id] ?? true) : false,
    }));
    const meta: TurnAttachment[] = pending.map((a) => ({
      name: a.name,
      size: a.size,
      sha256: a.sha256,
      remotePath: a.remotePath,
      secret: a.secret,
    }));
    const userTurn: Turn = {
      role: "user",
      parts: [{ kind: "text", text }],
      attachments: meta.length ? meta : undefined,
    };
    const assistantTurn: Turn = { role: "assistant", parts: [] };
    // History must go to Rust *without* the placeholder assistant turn.
    const prior = history();
    const nextTurns = [...turns, userTurn, assistantTurn];
    setTurns(nextTurns);
    setDraft("");
    setPending([]);
    setShareContent({});
    pinned.current = true;
    setRunning(true);
    setError(null);
    // Save as soon as the question is asked, so closing the panel right after
    // sending still leaves the conversation in the history.
    void persist(nextTurns);

    try {
      await aiApi.send(
        providerId,
        model,
        sessionId,
        mode,
        [...prior, { role: "user", content: text }],
        sendList,
      );
    } catch (e) {
      setError(errorMessage(e));
      setRunning(false);
    }
  }

  return (
    <div className="ai">
      <div className="panel-head">
        <span className="panel-head__title">
          <IconSparkle size={14} /> 助手
        </span>
        <div className="spacer" />
        <button className="iconbtn" title="新对话" disabled={running} onClick={newConversation}>
          <IconPlus size={14} />
        </button>
        <button
          className={`iconbtn ${historyOpen ? "iconbtn--on" : ""}`}
          title="历史对话"
          onClick={() => setHistoryOpen((v) => !v)}
        >
          <IconClock size={14} />
        </button>
        <button className="iconbtn" onClick={onClose} title="关闭">
          <IconX size={15} />
        </button>
      </div>
      <div className="ai__bar">
        <select
          className="ai__select"
          value={providerId ?? ""}
          onChange={(e) => {
            const id = e.target.value || null;
            setProviderId(id);
            // A model id belongs to one provider; carrying it across would send
            // a name the new endpoint has never heard of.
            setModel(providers.find((p) => p.id === id)?.models[0] ?? null);
          }}
        >
          {providers.length === 0 && <option value="">未配置服务商</option>}
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        {activeModelOptions.length > 1 && (
          <select
            className="ai__select"
            value={model ?? ""}
            onChange={(e) => setModel(e.target.value || null)}
            title="模型"
          >
            {activeModelOptions.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        )}
        <select
          className="ai__select"
          value={mode}
          onChange={(e) => setMode(e.target.value as PermissionMode)}
          title="权限模式"
        >
          <option value="manual">每次确认</option>
          <option value="auto">自动（只读）</option>
          <option value="bypass">完全放开</option>
        </select>
      </div>
      {!sessionId && (
        <div className="ai__note">当前标签不是 SSH 会话，助手只能聊天，不能操作服务器。</div>
      )}

      <div
        className="ai__log"
        ref={logRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
        }}
      >
        {turns.length === 0 && (
          <div className="ai__welcome">
            <div className="ai__welcome-icon">
              <IconSparkle size={20} />
            </div>
            <div className="ai__welcome-title">问点什么</div>
            <div className="ai__welcome-hint">模型可以读终端、列目录、读文件；改动服务器的操作需要你确认。</div>
            {providers.length === 0 ? (
              <div className="ai__welcome-hint">先到「设置 → AI」添加一个模型服务商。</div>
            ) : (
              <div className="ai__suggest">
                {["看看这台机器的磁盘占用", "最近有哪些服务报错？", "解释一下终端里最后的输出"].map((q) => (
                  <button key={q} className="chip" onClick={() => setDraft(q)}>
                    {q}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        {turns.map((t, i) => {
          const live = running && i === turns.length - 1;
          return (
            <div key={i} className={`ai__turn ai__turn--${t.role}`}>
              {t.role === "assistant" && <div className="ai__who">助手</div>}
              {t.attachments && t.attachments.length > 0 && (
                <div className="ai__sentFiles">
                  {t.attachments.map((a) => (
                    <span key={a.remotePath} className="ai__sentFile" title={a.remotePath}>
                      {a.secret ? <IconLock size={11} /> : <IconFile size={11} />}
                      {a.name}
                      <span className="ai__sentSize">{formatSize(a.size)}</span>
                    </span>
                  ))}
                </div>
              )}
              {segments(t.parts).map((seg, j, all) =>
                seg.kind === "text" ? (
                  <div
                    key={j}
                    className={`ai__text ${live && j === all.length - 1 ? "ai__text--live" : ""}`}
                  >
                    {t.role === "assistant" ? <Markdown text={seg.text} /> : seg.text}
                  </div>
                ) : (
                  <ToolGroup
                    key={j}
                    calls={seg.calls}
                    onApprove={(id, approved) => void aiApi.approve(id, approved)}
                  />
                ),
              )}
              {t.error && <div className="error">{t.error}</div>}
            </div>
          );
        })}

        {/* Only while nothing is being written: the caret already says "busy". */}
        {running && turns[turns.length - 1]?.parts.at(-1)?.kind !== "text" && (
          <div className="ai__thinking">
            <span className="dots">
              <i />
              <i />
              <i />
            </span>
            正在思考
          </div>
        )}
      </div>

      {error && <div className="error" style={{ margin: "0 10px 8px" }}>{error}</div>}

      {(uploads.length > 0 || pending.length > 0) && (
        <div className="ai__attach">
          {uploads.map((u) => (
            <div key={u.id} className="ai__attachCard ai__attachCard--busy">
              <IconFile size={13} />
              <div className="ai__attachMain">
                <div className="ai__attachName">{u.name}</div>
                <div className="ai__attachBar">
                  <span style={{ width: `${u.percent}%` }} />
                </div>
              </div>
              <span className="ai__attachPct">{u.percent >= 100 ? "校验中…" : `${u.percent}%`}</span>
            </div>
          ))}
          {pending.map((a) => (
            <div key={a.id} className={`ai__attachCard ${a.secret ? "ai__attachCard--secret" : ""}`}>
              {a.secret ? <IconLock size={13} /> : <IconFile size={13} />}
              <div className="ai__attachMain">
                <div className="ai__attachName">
                  {a.name} <span className="ai__attachSize">{formatSize(a.size)}</span>
                </div>
                <button
                  className="ai__attachPath"
                  title="复制远端路径"
                  onClick={() => {
                    void navigator.clipboard?.writeText(a.remotePath);
                    toast("已复制路径", "success");
                  }}
                >
                  <IconCopy size={10} /> {a.remotePath}
                </button>
                {a.secret && <div className="ai__attachNote">私钥内容不会发给 AI</div>}
                {!a.verified && (
                  <div className="ai__attachWarn">
                    <IconAlert size={10} /> 服务器校验未通过
                  </div>
                )}
                {a.shareable && (
                  <label className="ai__attachShare">
                    <input
                      type="checkbox"
                      checked={shareContent[a.id] ?? true}
                      onChange={(e) =>
                        setShareContent((prev) => ({ ...prev, [a.id]: e.target.checked }))
                      }
                    />
                    让 AI 看到内容
                  </label>
                )}
              </div>
              <button className="iconbtn" title="移除" onClick={() => void removePending(a)}>
                <IconX size={12} />
              </button>
            </div>
          ))}
        </div>
      )}

      {sessionFiles.length > 0 && (
        <div className="ai__staging">
          服务器上有 {sessionFiles.length} 个暂存文件 ·{" "}
          <button className="linkbtn" onClick={() => void cleanupStaging()}>
            清理
          </button>
        </div>
      )}

      <div className="ai__composer">
        <button
          className="ai__attachBtn"
          title={sessionId ? "附加文件（上传到服务器）" : "仅 SSH 连接支持附件"}
          disabled={!sessionId || running}
          onClick={() => void pickFiles()}
        >
          <IconPaperclip size={15} />
        </button>
        <textarea
          ref={inputRef}
          className="ai__input"
          placeholder="输入问题，Enter 发送，Shift+Enter 换行"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button
          className="ai__send"
          disabled={running || !draft.trim()}
          onClick={() => void send()}
          title="发送 (Enter)"
        >
          <IconSend size={15} />
        </button>
      </div>

      {historyOpen && (
        <ConversationHistory
          hostId={hostId ?? null}
          hostLabel={hostLabel ?? ""}
          currentId={conversationId}
          disabled={running}
          onClose={() => setHistoryOpen(false)}
          onOpen={openConversation}
          onRenamed={(id, next) => {
            if (id === conversationId) setTitle(next);
          }}
        />
      )}
    </div>
  );
}

/** Consecutive tool calls are shown together, as one block of steps. */
type Segment = { kind: "text"; text: string } | { kind: "tools"; calls: ToolCallView[] };

function segments(parts: Part[]): Segment[] {
  const out: Segment[] = [];
  for (const p of parts) {
    const last = out[out.length - 1];
    if (p.kind === "text") out.push({ kind: "text", text: p.text });
    else if (last?.kind === "tools") last.calls.push(p.call);
    else out.push({ kind: "tools", calls: [p.call] });
  }
  return out;
}

const TOOL_LABEL: Record<string, string> = {
  read_terminal: "读取终端",
  run_command: "运行命令",
  list_directory: "列出目录",
  read_file: "读取文件",
  write_file: "写入文件",
  grep: "搜索内容",
};

/**
 * A run of tool calls. Several finished calls fold into one line ("执行了 7
 * 步") so the answer stays readable; a call waiting for approval always shows.
 */
function ToolGroup({
  calls,
  onApprove,
}: {
  calls: ToolCallView[];
  onApprove: (id: string, approved: boolean) => void;
}) {
  const failed = calls.filter((c) => c.result && !c.result.ok).length;
  // Only the user opens or closes the list. Tying it to "a call is running"
  // made it flap open and shut on every call while the model worked.
  const [open, setOpen] = useState(false);
  const running = calls.find((c) => !c.result && c.auto);
  // A call waiting for approval is shown even when folded: it needs a click.
  const asking = calls.filter((c) => !c.result && !c.auto);
  const pending = !!running || asking.length > 0;

  if (calls.length === 1) {
    const c = calls[0];
    return (
      <div className={`steps ${failed ? "steps--warn" : ""}`}>
        <div className="steps__list">
          <ToolRow call={c} onApprove={(ok) => onApprove(c.id, ok)} />
        </div>
      </div>
    );
  }

  const shown = open ? calls : asking;
  return (
    <div className={`steps ${failed ? "steps--warn" : ""}`}>
      <button className="steps__head" onClick={() => setOpen(!open)}>
        {running ? (
          <span className="spinner spinner--xs" />
        ) : (
          <IconChevronRight size={12} className={`steps__chev ${open ? "steps__chev--open" : ""}`} />
        )}
        <span>
          {pending ? "正在执行" : "执行了"} {calls.length} 步
          {failed > 0 && <span className="steps__failed"> · {failed} 步失败</span>}
        </span>
        <span className="steps__names">
          {running
            ? `${TOOL_LABEL[running.name] ?? running.name} ${summarize(running.args)}`
            : [...new Set(calls.map((c) => TOOL_LABEL[c.name] ?? c.name))].join("、")}
        </span>
      </button>
      {shown.length > 0 && (
        <div className="steps__list">
          {shown.map((c) => (
            <ToolRow key={c.id} call={c} onApprove={(ok) => onApprove(c.id, ok)} />
          ))}
        </div>
      )}
    </div>
  );
}

function ToolRow({ call, onApprove }: { call: ToolCallView; onApprove: (approved: boolean) => void }) {
  const [open, setOpen] = useState(false);
  const waiting = !call.result;
  const needsHuman = waiting && !call.auto;
  const state = call.result ? (call.result.ok ? "ok" : "err") : needsHuman ? "ask" : "run";

  return (
    <div className={`step step--${state}`}>
      <button className="step__head" onClick={() => setOpen(!open)} disabled={!call.result}>
        <span className="step__icon">
          {state === "ok" ? (
            <IconCheck size={12} />
          ) : state === "err" ? (
            <IconAlert size={12} />
          ) : state === "ask" ? (
            <IconClock size={12} />
          ) : (
            <span className="spinner spinner--xs" />
          )}
        </span>
        <span className="step__label">{TOOL_LABEL[call.name] ?? call.name}</span>
        <code className="step__summary">{summarize(call.args)}</code>
        {call.result && (
          <IconChevronRight size={12} className={`step__chev ${open ? "step__chev--open" : ""}`} />
        )}
      </button>

      {needsHuman && (
        <div className="step__ask">
          <pre className="step__args">{JSON.stringify(call.args, null, 2)}</pre>
          <div className="step__actions">
            <span className="step__ask-text">需要你确认</span>
            <button className="btn btn--sm btn--ghost" onClick={() => onApprove(false)}>
              拒绝
            </button>
            <button className="btn btn--sm btn--primary" onClick={() => onApprove(true)}>
              允许
            </button>
          </div>
        </div>
      )}

      {open && call.result && <pre className="step__out">{call.result.content || "（没有输出）"}</pre>}
    </div>
  );
}

/** The one argument that says what a call does, e.g. the command or path. */
function summarize(args: unknown): string {
  if (!args || typeof args !== "object") return "";
  const a = args as Record<string, unknown>;
  const v = a.command ?? a.path ?? a.pattern ?? (a.lines != null ? `最近 ${a.lines} 行` : null);
  return v == null ? "" : String(v);
}
