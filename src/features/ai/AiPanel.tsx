import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  aiApi,
  attachmentApi,
  errorMessage,
  formatTokens,
  type ChatMessage,
  type AttachmentView,
  type PermissionMode,
  type ProviderView,
} from "@/lib/api";
import { confirmDialog, toast } from "@/components/ui";
import { useApp } from "@/app/store";
import { useTerminals } from "@/features/terminal/terminals";
import { ConversationHistory } from "@/features/ai/ConversationHistory";
import {
  blankChat,
  listenForTurns,
  newConversationId,
  saveChat,
  useChats,
  type Chat,
  type Part,
  type ToolCallView,
  type Turn,
  type TurnAttachment,
} from "@/features/ai/chats";
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
  IconStop,
  IconX,
  IconCopy,
} from "@/components/icons";
import { Markdown } from "@/features/ai/Markdown";

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
 * It shows the conversation of the tab in front (see `chats.ts`); the turn
 * runs in Rust and streams events into the tab that asked. A gated tool call
 * blocks in the engine until the user approves it in that tab's conversation.
 */
export function AiPanel({
  tabId,
  sessionId,
  hostId,
  hostLabel,
  onClose,
}: {
  /** The terminal tab whose conversation this is. */
  tabId: string;
  sessionId: string | null;
  /** Host this tab is bound to, so a conversation can be listed per host. */
  hostId?: string | null;
  hostLabel?: string;
  onClose: () => void;
}) {
  const ensure = useChats((s) => s.ensure);
  const update = useChats((s) => s.update);
  const replace = useChats((s) => s.replace);
  const setRunningTab = useChats((s) => s.setRunning);
  const stopTurn = useChats((s) => s.stop);
  const stored = useChats((s) => s.chats[tabId]);
  const runningTab = useChats((s) => s.runningTab);
  // Until `ensure` lands (one render), show an empty conversation.
  const fallback = useMemo(() => blankChat(hostId ?? null, hostLabel ?? ""), [hostId, hostLabel]);
  const chat: Chat = stored ?? fallback;
  const turns = chat.turns;
  const draft = chat.draft;
  const error = chat.error;
  const setDraft = (next: string | ((cur: string) => string)) =>
    update(tabId, { draft: typeof next === "function" ? next(useChats.getState().chats[tabId]?.draft ?? "") : next });
  const setError = (message: string | null) => update(tabId, { error: message });
  // One turn at a time across all tabs; `runningHere` is this tab's.
  const running = runningTab !== null;
  const runningHere = runningTab === tabId;

  useEffect(() => {
    listenForTurns();
  }, []);
  useEffect(() => {
    ensure(tabId, hostId ?? null, hostLabel ?? "");
  }, [tabId, hostId, hostLabel, ensure]);

  const [mode, setMode] = useState<PermissionMode>("manual");
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [providerId, setProviderId] = useState<string | null>(null);
  const [model, setModel] = useState<string | null>(null);
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
  const [historyOpen, setHistoryOpen] = useState(false);
  const logRef = useRef<HTMLDivElement>(null);

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

  // Locking the vault clears the panel, like the terminals around it (the
  // conversations themselves are dropped by chats.ts).
  const phase = useApp((s) => s.phase);
  useEffect(() => {
    if (phase === "unlocked") return;
    setHistoryOpen(false);
    // Rust forgot the attachments when the vault locked; cards left here would
    // be dropped silently on the next send.
    setPending([]);
    setShareContent({});
    setFilesBySession({});
  }, [phase]);

  /**
   * Shows a stored conversation in this tab. One about another host is shown
   * read-only: continuing it here would run its commands on this tab's server.
   */
  function openConversation(conv: Record<string, unknown>) {
    const convHost = typeof conv.hostId === "string" ? conv.hostId : null;
    const convLabel = typeof conv.hostLabel === "string" ? conv.hostLabel : "";
    const sameHost = convHost ? convHost === (hostId ?? null) : !hostId && convLabel === (hostLabel ?? "");
    replace(tabId, {
      ...blankChat(convHost, convLabel),
      id: String(conv.id),
      title: typeof conv.title === "string" ? conv.title : "",
      createdAt: typeof conv.createdAt === "number" ? conv.createdAt : Math.floor(Date.now() / 1000),
      turns: Array.isArray(conv.turns) ? (conv.turns as Turn[]) : [],
      providerId: typeof conv.providerId === "string" ? conv.providerId : null,
      model: typeof conv.model === "string" ? conv.model : null,
      foreign: sameHost ? null : { hostId: convHost, hostLabel: convLabel },
    });
    if (typeof conv.providerId === "string") setProviderId(conv.providerId);
    if (typeof conv.model === "string") setModel(conv.model);
    setHistoryOpen(false);
    pinned.current = true;
  }

  function newConversation() {
    if (runningHere) return;
    replace(tabId, blankChat(hostId ?? null, hostLabel ?? ""));
  }

  /**
   * Moves a conversation that belongs to another host over to a tab on that
   * host: an open tab whose own conversation is still empty, else a new one.
   */
  function continueOnItsHost() {
    const foreign = chat.foreign;
    const host = useApp.getState().hosts.find((h) => h.id === foreign?.hostId);
    if (!foreign || !host) {
      toast("找不到这台主机，可能已被删除", "error");
      return;
    }
    const terms = useTerminals.getState();
    const chats = useChats.getState().chats;
    const free = terms.tabs.find((t) => t.hostId === host.id && !(chats[t.id]?.turns.length));
    const target = free ? free.id : terms.openHost(host);
    if (free) terms.activate(free.id);
    replace(target, { ...chat, foreign: null, draft: "", error: null });
    replace(tabId, blankChat(hostId ?? null, hostLabel ?? ""));
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

  // A report only comes when the conversation had to be shortened to fit the
  // model: from then on, a fresh conversation would be faster and lose less.
  const long = [...turns].reverse().find((t) => t.context)?.context ?? null;

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
    if (!text || running || chat.foreign) return;
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
    const next: Chat = {
      ...chat,
      turns: [...turns, userTurn, assistantTurn],
      draft: "",
      error: null,
      providerId,
      model,
    };
    replace(tabId, next);
    setPending([]);
    setShareContent({});
    pinned.current = true;
    const turnId = newConversationId();
    setRunningTab(tabId, turnId);
    // Save as soon as the question is asked, so closing the panel right after
    // sending still leaves the conversation in the history.
    void saveChat(next);

    try {
      await aiApi.send(
        providerId,
        model,
        sessionId,
        mode,
        [...prior, { role: "user", content: text }],
        sendList,
        turnId,
      );
    } catch (e) {
      setError(errorMessage(e));
      setRunningTab(null);
    }
  }

  return (
    <div className="ai">
      <div className="panel-head">
        <span className="panel-head__title">
          <IconSparkle size={14} /> 助手
        </span>
        <div className="spacer" />
        <button className="iconbtn" title="新对话" disabled={runningHere} onClick={newConversation}>
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
          const live = runningHere && i === turns.length - 1;
          return (
            <div key={i} className={`ai__turn ai__turn--${t.role}`}>
              {t.role === "assistant" && <div className="ai__who">助手</div>}
              {t.learned && (
                <div className="ai__trimmed">
                  {t.learned.exact
                    ? `这个模型的上下文上限是 ${formatTokens(t.learned.tokens)} token（服务商在报错中给出，已记住），已精简后自动重试。`
                    : "请求超过了模型能接收的长度，已精简后自动重试。"}
                </div>
              )}
              {t.context && (t.context.droppedRounds > 0 || t.context.shortenedOutputs > 0) && (
                <div
                  className="ai__trimmed"
                  title={`本次发送约 ${formatTokens(t.context.tokens)} token，模型上限 ${formatTokens(t.context.limit)}`}
                >
                  为控制长度：
                  {[
                    t.context.shortenedOutputs > 0 && `${t.context.shortenedOutputs} 段命令输出已精简`,
                    t.context.droppedRounds > 0 && `更早的 ${t.context.droppedRounds} 轮对话未发送`,
                  ]
                    .filter(Boolean)
                    .join("，")}
                </div>
              )}
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
                    live={live}
                    calls={seg.calls}
                    onApprove={(id, approved) => void aiApi.approve(id, approved)}
                  />
                ),
              )}
              {t.error && <div className="error">{t.error}</div>}
              {t.stopped && <div className="ai__stopped">已停止</div>}
            </div>
          );
        })}

        {/* Only while nothing is being written: the caret already says "busy". */}
        {runningHere && turns[turns.length - 1]?.parts.at(-1)?.kind !== "text" && (
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

      {chat.foreign && (
        <div className="ai__foreign">
          <div>
            这段对话来自 <strong>{chat.foreign.hostLabel || "另一台主机"}</strong>，当前标签是{" "}
            <strong>{hostLabel || "本地"}</strong>。在这里继续，命令会在当前标签的服务器上执行，所以只能查看。
          </div>
          <div className="ai__foreign-actions">
            {chat.foreign.hostId && useApp.getState().hosts.some((h) => h.id === chat.foreign?.hostId) && (
              <button className="btn btn--sm btn--primary" onClick={continueOnItsHost}>
                到 {chat.foreign.hostLabel || "那台主机"} 继续
              </button>
            )}
            <button className="btn btn--sm" onClick={newConversation}>
              在这里开新对话
            </button>
          </div>
        </div>
      )}

      {!chat.foreign && !runningHere && long && (
        <div className="ai__long">
          <span>
            对话已接近模型上限（约 {formatTokens(long.tokens)} / {formatTokens(long.limit)} token），较早的内容会被精简或不再发送。开新对话会更快、更完整。
          </span>
          <button className="btn btn--sm" onClick={newConversation}>
            开新对话
          </button>
        </div>
      )}

      {running && !runningHere && (
        <div className="ai__note">另一个标签的助手正在执行，完成后才能在这里提问。</div>
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
          disabled={!!chat.foreign}
          placeholder={chat.foreign ? "这段对话只能查看" : "输入问题，Enter 发送，Shift+Enter 换行"}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        {runningHere ? (
          <button className="ai__send ai__send--stop" onClick={stopTurn} title="停止">
            <IconStop size={13} />
          </button>
        ) : (
          <button
            className="ai__send"
            disabled={running || !!chat.foreign || !draft.trim()}
            onClick={() => void send()}
            title="发送 (Enter)"
          >
            <IconSend size={15} />
          </button>
        )}
      </div>

      {historyOpen && (
        <ConversationHistory
          hostId={hostId ?? null}
          hostLabel={hostLabel ?? ""}
          currentId={chat.id}
          disabled={runningHere}
          onClose={() => setHistoryOpen(false)}
          onOpen={openConversation}
          onRenamed={(id, next) => useChats.getState().renamed(id, next)}
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
  live,
  calls,
  onApprove,
}: {
  /** Whether this is the turn that is running; otherwise an unfinished call
   *  was cut off (stopped, or the app closed) and will never finish. */
  live: boolean;
  calls: ToolCallView[];
  onApprove: (id: string, approved: boolean) => void;
}) {
  const failed = calls.filter((c) => c.result && !c.result.ok).length;
  // Only the user opens or closes the list. Tying it to "a call is running"
  // made it flap open and shut on every call while the model worked.
  const [open, setOpen] = useState(false);
  const running = live ? calls.find((c) => !c.result && c.auto) : undefined;
  // A call waiting for approval is shown even when folded: it needs a click.
  const asking = live ? calls.filter((c) => !c.result && !c.auto) : [];
  const pending = !!running || asking.length > 0;

  if (calls.length === 1) {
    const c = calls[0];
    return (
      <div className={`steps ${failed ? "steps--warn" : ""}`}>
        <div className="steps__list">
          <ToolRow call={c} live={live} onApprove={(ok) => onApprove(c.id, ok)} />
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
            <ToolRow key={c.id} call={c} live={live} onApprove={(ok) => onApprove(c.id, ok)} />
          ))}
        </div>
      )}
    </div>
  );
}

function ToolRow({
  call,
  live,
  onApprove,
}: {
  call: ToolCallView;
  live: boolean;
  onApprove: (approved: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const waiting = !call.result;
  const cancelled = waiting && !live;
  const needsHuman = waiting && !call.auto && !cancelled;
  const state = call.result
    ? call.result.ok
      ? "ok"
      : "err"
    : cancelled
      ? "skip"
      : needsHuman
        ? "ask"
        : "run";

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
          ) : state === "skip" ? (
            <IconX size={12} />
          ) : (
            <span className="spinner spinner--xs" />
          )}
        </span>
        <span className="step__label">{TOOL_LABEL[call.name] ?? call.name}</span>
        <code className="step__summary">{summarize(call.args)}</code>
        {cancelled && <span className="step__skipped">未执行</span>}
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
