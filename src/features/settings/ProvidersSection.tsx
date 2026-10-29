import { useEffect, useState } from "react";
import { aiApi, errorMessage, type ProviderView } from "@/lib/api";

/** Presets so the common cases need two fields at most. */
const PRESETS = [
  { label: "OpenAI", protocol: "open_ai", baseUrl: "https://api.openai.com", model: "gpt-4o-mini" },
  {
    label: "OpenRouter",
    protocol: "open_ai",
    baseUrl: "https://openrouter.ai/api",
    model: "anthropic/claude-3.5-sonnet",
  },
  {
    label: "Anthropic",
    protocol: "anthropic",
    baseUrl: "https://api.anthropic.com",
    model: "claude-sonnet-4-20250514",
  },
  { label: "Ollama", protocol: "open_ai", baseUrl: "http://localhost:11434", model: "llama3.2" },
  {
    label: "LM Studio",
    protocol: "open_ai",
    baseUrl: "http://localhost:1234",
    model: "local-model",
  },
];

export function ProvidersSection() {
  const [list, setList] = useState<ProviderView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<ProviderView | null>(null);
  const [showForm, setShowForm] = useState(false);

  async function refresh() {
    try {
      setList(await aiApi.providers());
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  return (
    <div className="editor__section">
      <div className="editor__legend">模型服务商（AI 助手）</div>

      {list.length === 0 && !showForm && (
        <div className="muted" style={{ lineHeight: 1.7 }}>
          还没有配置。添加一个后，终端页右侧的「助手」就能用了。
        </div>
      )}

      {list.map((p) => (
        <div key={p.id} className="hostrow" style={{ gridTemplateColumns: "1fr 1.5fr 130px" }}>
          <div className="hostrow__name">
            <span className="hostrow__dot" />
            {p.name}
          </div>
          <div className="hostrow__addr" style={{ fontSize: 11.5 }}>
            {p.model} · {p.baseUrl}
          </div>
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
            <button
              className="btn"
              onClick={() => {
                setEditing(p);
                setShowForm(true);
              }}
            >
              编辑
            </button>
            <button
              className="btn btn--ghost"
              onClick={async () => {
                await aiApi.deleteProvider(p.id);
                await refresh();
              }}
            >
              删除
            </button>
          </div>
        </div>
      ))}

      {error && <div className="error">{error}</div>}

      {showForm ? (
        <ProviderForm
          initial={editing}
          onCancel={() => {
            setShowForm(false);
            setEditing(null);
          }}
          onSaved={async () => {
            setShowForm(false);
            setEditing(null);
            await refresh();
          }}
        />
      ) : (
        <div className="footerbar" style={{ marginTop: 12 }}>
          <button
            className="btn btn--primary"
            onClick={() => {
              setEditing(null);
              setShowForm(true);
            }}
          >
            + 添加服务商
          </button>
        </div>
      )}
    </div>
  );
}

function ProviderForm({
  initial,
  onCancel,
  onSaved,
}: {
  initial: ProviderView | null;
  onCancel: () => void;
  onSaved: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [protocol, setProtocol] = useState<string>(initial?.protocol ?? "open_ai");
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl ?? "https://api.openai.com");
  const [model, setModel] = useState(initial?.model ?? "");
  const [apiKey, setApiKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A key is required on create; on edit, blank means "keep the stored one".
  const needKey = !initial && !apiKey.trim();
  const canSave = name.trim() && baseUrl.trim() && model.trim() && !needKey;

  async function save() {
    if (!canSave || busy) return;
    setBusy(true);
    setError(null);
    try {
      await aiApi.saveProvider({
        id: initial?.id,
        name: name.trim(),
        protocol,
        baseUrl: baseUrl.trim(),
        model: model.trim(),
        apiKey: apiKey.trim() || undefined,
      });
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <div className="editor__section" style={{ border: "1px solid var(--border)", borderRadius: 10, padding: 14, marginTop: 12 }}>
      <div className="field">
        <label className="field__label">快速选择</label>
        <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
          {PRESETS.map((p) => (
            <button
              key={p.label}
              className="btn"
              onClick={() => {
                setName(p.label);
                setProtocol(p.protocol);
                setBaseUrl(p.baseUrl);
                setModel(p.model);
              }}
            >
              {p.label}
            </button>
          ))}
        </div>
      </div>

      <div className="grid2">
        <div className="field">
          <label className="field__label">名称</label>
          <input className="field__input" value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="field">
          <label className="field__label">协议</label>
          <select
            className="field__input"
            value={protocol}
            onChange={(e) => setProtocol(e.target.value)}
          >
            <option value="open_ai">OpenAI 兼容</option>
            <option value="anthropic">Anthropic</option>
          </select>
        </div>
      </div>

      <div className="field">
        <label className="field__label">Base URL</label>
        <input className="field__input" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} />
      </div>

      <div className="field">
        <label className="field__label">模型</label>
        <input className="field__input" value={model} onChange={(e) => setModel(e.target.value)} />
      </div>

      <div className="field">
        <label className="field__label">API Key</label>
        <input
          className="field__input"
          type="password"
          value={apiKey}
          placeholder={initial?.hasKey ? "已保存，留空则不修改" : "sk-..."}
          onChange={(e) => setApiKey(e.target.value)}
        />
        <div className="muted" style={{ marginTop: 6 }}>
          密钥和其他敏感信息一样，加密后才写入本机数据库。
        </div>
      </div>

      {error && <div className="error">{error}</div>}

      <div className="footerbar">
        <button className="btn btn--ghost" onClick={onCancel}>
          取消
        </button>
        <button className="btn btn--primary" disabled={!canSave || busy} onClick={() => void save()}>
          {busy ? "保存中…" : "保存"}
        </button>
      </div>
    </div>
  );
}
