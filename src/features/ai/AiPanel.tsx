import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  aiApi,
  errorMessage,
  type AiEvent,
  type ChatMessage,
  type PermissionMode,
  type ProviderView,
} from "@/lib/api";
import { IconCheck, IconSend, IconSparkle, IconTrash, IconX, IconChevronRight } from "@/components/icons";
import { Fragment, type ReactNode } from "react";

/**
 * Renders the model's reply with just enough markdown to be readable: fenced
 * code blocks, inline code and bold. Anything else stays plain text — no HTML
 * from the model is ever interpreted.
 */
function RichText({ text }: { text: string }) {
  const parts = text.split(/```(\w*)\n?([\s\S]*?)```/g);
  const out: ReactNode[] = [];
  for (let i = 0; i < parts.length; i += 3) {
    out.push(<Inline key={`t${i}`} text={parts[i]} />);
    if (i + 2 < parts.length) {
      out.push(
        <pre key={`c${i}`} className="ai__code">
          <code>{parts[i + 2].replace(/\n$/, "")}</code>
        </pre>,
      );
    }
  }
  return <>{out}</>;
}

function Inline({ text }: { text: string }) {
  const bits = text.split(/(`[^`\n]+`|\*\*[^*\n]+\*\*)/g);
  return (
    <>
      {bits.map((b, i) =>
        b.startsWith("`") && b.endsWith("`") && b.length > 1 ? (
          <code key={i} className="ai__inline">
            {b.slice(1, -1)}
          </code>
        ) : b.startsWith("**") && b.endsWith("**") && b.length > 4 ? (
          <strong key={i}>{b.slice(2, -2)}</strong>
        ) : (
          <Fragment key={i}>{b}</Fragment>
        ),
      )}
    </>
  );
}

interface ToolCallView {
  id: string;
  name: string;
  args: unknown;
  auto: boolean;
  /** undefined while waiting for the user. */
  result?: { ok: boolean; content: string };
  /** Set when the model produced this call, so the history round-trips. */
}

interface Turn {
  role: "user" | "assistant";
  text: string;
  toolCalls: ToolCallView[];
  error?: string;
}

/**
 * The assistant panel beside the terminal.
 *
 * The conversation lives here; the turn runs in Rust and streams events. A
 * gated tool call blocks in the engine until this panel sends its approval, so
 * closing the panel mid-turn reads as a refusal rather than a silent execution.
 */
export function AiPanel({
  sessionId,
  onClose,
}: {
  sessionId: string | null;
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
  const bottomRef = useRef<HTMLDivElement>(null);

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

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [turns]);

  const activeModelOptions = providers.find((p) => p.id === providerId)?.models ?? [];

  // Stream the engine's events into the last assistant turn.
  useEffect(() => {
    let disposed = false;
    let un: (() => void) | undefined;

    void listen<AiEvent>("ai-event", (evt) => {
      const ev = evt.payload;
      setTurns((prev) => {
        const next = [...prev];
        const last = next[next.length - 1];
        if (!last || last.role !== "assistant") return prev;

        if (ev.type === "text") {
          next[next.length - 1] = { ...last, text: last.text + ev.text };
        } else if (ev.type === "toolCall") {
          next[next.length - 1] = {
            ...last,
            toolCalls: [
              ...last.toolCalls,
              { id: ev.id, name: ev.name, args: ev.arguments, auto: ev.auto },
            ],
          };
        } else if (ev.type === "toolResult") {
          next[next.length - 1] = {
            ...last,
            toolCalls: last.toolCalls.map((c) =>
              c.id === ev.id ? { ...c, result: { ok: ev.ok, content: ev.content } } : c,
            ),
          };
        } else if (ev.type === "error") {
          next[next.length - 1] = { ...last, error: ev.message };
        }
        return next;
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

  const history = useCallback((): ChatMessage[] => {
    const out: ChatMessage[] = [];
    for (const t of turns) {
      if (t.role === "user") {
        out.push({ role: "user", content: t.text });
      } else {
        const calls = t.toolCalls
          .filter((c) => c.result)
          .map((c) => ({
            id: c.id,
            name: c.name,
            // The engine echoes the exact arguments back to the model.
            arguments: c.args,
          }));
        out.push({ role: "assistant", content: t.text, toolCalls: calls });
        for (const c of t.toolCalls) {
          if (c.result) {
            out.push({ role: "tool", content: c.result.content, toolCallId: c.id });
          }
        }
      }
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

    const userTurn: Turn = { role: "user", text, toolCalls: [] };
    const assistantTurn: Turn = { role: "assistant", text: "", toolCalls: [] };
    // History must go to Rust *without* the placeholder assistant turn.
    const prior = history();
    setTurns((prev) => [...prev, userTurn, assistantTurn]);
    setDraft("");
    setRunning(true);
    setError(null);

    try {
      await aiApi.send(providerId, model, sessionId, mode, [
        ...prior,
        { role: "user", content: text },
      ]);
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
        <button
          className="iconbtn"
          title="清空对话"
          disabled={running || turns.length === 0}
          onClick={() => setTurns([])}
        >
          <IconTrash size={14} />
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

      <div className="ai__log">
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

        {turns.map((t, i) => (
          <div key={i} className={`ai__turn ai__turn--${t.role}`}>
            {t.role === "assistant" && <div className="ai__who">助手</div>}
            {t.text && (
              <div className="ai__text">{t.role === "assistant" ? <RichText text={t.text} /> : t.text}</div>
            )}

            {t.toolCalls.map((c) => (
              <ToolCard
                key={c.id}
                call={c}
                onApprove={async (approved) => {
                  await aiApi.approve(c.id, approved);
                }}
              />
            ))}

            {t.error && <div className="error">{t.error}</div>}
          </div>
        ))}

        {running && (
          <div className="ai__thinking">
            <span className="dots">
              <i />
              <i />
              <i />
            </span>
            正在思考
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      {error && <div className="error" style={{ margin: "0 10px 8px" }}>{error}</div>}

      <div className="ai__composer">
        <textarea
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
    </div>
  );
}

function ToolCard({
  call,
  onApprove,
}: {
  call: ToolCallView;
  onApprove: (approved: boolean) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const waiting = !call.result;
  const needsHuman = waiting && !call.auto;

  return (
    <div className={`toolcard ${call.result && !call.result.ok ? "toolcard--err" : ""}`}>
      <div className="toolcard__head" onClick={() => setOpen(!open)}>
        <IconChevronRight size={12} className={`toolcard__chev ${open ? "toolcard__chev--open" : ""}`} />
        <span className="toolcard__name">{call.name}</span>
        <span className="toolcard__summary">{summarize(call.args)}</span>
        {call.result ? (
          <span className={`toolcard__state ${call.result.ok ? "" : "toolcard__state--err"}`}>
            {call.result.ok ? <IconCheck size={12} /> : "失败"}
          </span>
        ) : waiting ? (
          <span className="toolcard__state">{needsHuman ? "等待确认" : "运行中"}</span>
        ) : null}
      </div>

      {needsHuman && (
        <>
          <pre className="toolcard__args">{JSON.stringify(call.args, null, 2)}</pre>
          <div className="toolcard__actions">
            <button className="btn btn--ghost" onClick={() => void onApprove(false)}>
              拒绝
            </button>
            <button className="btn btn--primary" onClick={() => void onApprove(true)}>
              允许
            </button>
          </div>
        </>
      )}

      {open && call.result && <pre className="toolcard__out">{call.result.content}</pre>}
    </div>
  );
}

/** The one argument that says what a call does, e.g. the command or path. */
function summarize(args: unknown): string {
  if (!args || typeof args !== "object") return "";
  const a = args as Record<string, unknown>;
  const v = a.command ?? a.path ?? a.pattern ?? a.lines;
  return v == null ? "" : String(v);
}
