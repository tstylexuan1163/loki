import { useEffect, useMemo, useState } from "react";
import {
  conversationApi,
  errorMessage,
  type ConversationSummary,
} from "@/lib/api";
import { Segmented, confirmDialog, toast } from "@/components/ui";
import { IconClock, IconEdit, IconTrash, IconX } from "@/components/icons";

function dayBucket(secs: number): "today" | "yesterday" | "earlier" {
  const now = new Date();
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000;
  if (secs >= start) return "today";
  if (secs >= start - 86400) return "yesterday";
  return "earlier";
}

const BUCKET_LABEL = { today: "今天", yesterday: "昨天", earlier: "更早" } as const;

function clockOf(secs: number): string {
  const d = new Date(secs * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  const today = dayBucket(secs) === "today";
  return today
    ? `${p(d.getHours())}:${p(d.getMinutes())}`
    : `${d.getMonth() + 1}月${d.getDate()}日`;
}

/**
 * The history drawer inside the assistant panel.
 *
 * It owns its own loading state so opening it never blocks the chat, and it
 * reloads whenever the scope changes — a stale list after sending a message
 * would be confusing.
 */
export function ConversationHistory({
  hostId,
  hostLabel,
  currentId,
  disabled,
  onClose,
  onOpen,
  onRenamed,
}: {
  hostId: string | null;
  hostLabel: string;
  currentId: string;
  /** True while a turn is running: switching would abandon it. */
  disabled: boolean;
  onClose: () => void;
  onOpen: (conversation: Record<string, unknown>) => void;
  /** Lets the panel keep its own title in step after a rename. */
  onRenamed?: (id: string, title: string) => void;
}) {
  const [scope, setScope] = useState<"host" | "all">("host");
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<ConversationSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const reload = () => {
    setLoading(true);
    setError(null);
    void conversationApi
      .list(scope === "host" ? hostId : null)
      .then((list) => setItems(list))
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setLoading(false));
  };

  useEffect(reload, [scope, hostId]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return items.filter((c) => !q || c.title.toLowerCase().includes(q));
  }, [items, query]);

  const groups = useMemo(() => {
    const out: Record<string, ConversationSummary[]> = {};
    for (const c of visible) {
      const key = dayBucket(c.updatedAt);
      (out[key] ??= []).push(c);
    }
    return out;
  }, [visible]);

  const open = async (id: string) => {
    try {
      const conv = await conversationApi.get(id);
      if (!conv) {
        toast("对话不存在", "error");
        return;
      }
      onOpen(conv as Record<string, unknown>);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const commitRename = async (id: string) => {
    const title = draft.trim();
    setRenaming(null);
    if (!title) return;
    try {
      await conversationApi.rename(id, title);
      setItems((prev) => prev.map((c) => (c.id === id ? { ...c, title } : c)));
      onRenamed?.(id, title);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const remove = async (c: ConversationSummary) => {
    const ok = await confirmDialog({
      title: "删除对话",
      body: `确定删除「${c.title}」吗？删除后无法恢复。`,
      confirmLabel: "删除",
      danger: true,
    });
    if (!ok) return;
    try {
      await conversationApi.remove(c.id);
      setItems((prev) => prev.filter((x) => x.id !== c.id));
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  return (
    <div className="conv">
      <div className="conv__head">
        <span className="conv__title">
          <IconClock size={14} /> 历史对话
        </span>
        <div className="spacer" />
        <button className="iconbtn" onClick={onClose} title="关闭">
          <IconX size={14} />
        </button>
      </div>

      <div className="conv__filters">
        <Segmented
          value={scope}
          options={[
            { value: "host", label: hostLabel ? `当前主机` : "当前主机" },
            { value: "all", label: "全部" },
          ]}
          onChange={(v) => setScope(v)}
        />
        <input
          className="input conv__search"
          placeholder="按标题搜索"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      <div className="conv__list">
        {loading && <div className="conv__state">正在加载…</div>}
        {!loading && error && <div className="conv__state conv__state--err">{error}</div>}
        {!loading && !error && visible.length === 0 && (
          <div className="conv__state">
            {query ? "没有匹配的对话" : "还没有保存的对话。提出问题后会自动保存。"}
          </div>
        )}

        {(["today", "yesterday", "earlier"] as const).map((bucket) => {
          const list = groups[bucket];
          if (!list?.length) return null;
          return (
            <div key={bucket} className="conv__group">
              <div className="conv__groupTitle">{BUCKET_LABEL[bucket]}</div>
              {list.map((c) => (
                <div
                  key={c.id}
                  className={`conv__row ${c.id === currentId ? "conv__row--on" : ""}`}
                  onClick={() => {
                    if (disabled || renaming === c.id) return;
                    void open(c.id);
                  }}
                >
                  {renaming === c.id ? (
                    <input
                      className="input conv__rename"
                      autoFocus
                      value={draft}
                      onChange={(e) => setDraft(e.target.value)}
                      onClick={(e) => e.stopPropagation()}
                      onBlur={() => void commitRename(c.id)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void commitRename(c.id);
                        if (e.key === "Escape") setRenaming(null);
                      }}
                    />
                  ) : (
                    <>
                      <div className="conv__main">
                        <div className="conv__rowTitle">{c.title}</div>
                        <div className="conv__meta">
                          {c.hostLabel || "未绑定主机"} · {c.turnCount} 轮 ·{" "}
                          {clockOf(c.updatedAt)}
                        </div>
                      </div>
                      <div className="conv__actions" onClick={(e) => e.stopPropagation()}>
                        <button
                          className="iconbtn"
                          title="重命名"
                          disabled={disabled}
                          onClick={() => {
                            setRenaming(c.id);
                            setDraft(c.title);
                          }}
                        >
                          <IconEdit size={13} />
                        </button>
                        <button
                          className="iconbtn iconbtn--danger"
                          title="删除"
                          disabled={disabled}
                          onClick={() => void remove(c)}
                        >
                          <IconTrash size={13} />
                        </button>
                      </div>
                    </>
                  )}
                </div>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
