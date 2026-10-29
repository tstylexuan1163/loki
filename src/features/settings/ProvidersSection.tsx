import { useEffect, useState } from "react";
import { aiApi, errorMessage, type ProviderInput, type ProviderView } from "@/lib/api";
import { Modal } from "@/components/ui";
import { IconEye, IconEyeOff, IconPlus, IconRefresh, IconTrash, IconPlug } from "@/components/icons";

/** Fill helpers only: they set protocol + base URL, nothing else. */
const PRESETS: { label: string; protocol: string; baseUrl: string }[] = [
  { label: "OpenAI", protocol: "chat_completions", baseUrl: "https://api.openai.com/v1" },
  { label: "OpenRouter", protocol: "chat_completions", baseUrl: "https://openrouter.ai/api/v1" },
  { label: "Anthropic", protocol: "anthropic", baseUrl: "https://api.anthropic.com/v1" },
  { label: "DeepSeek", protocol: "chat_completions", baseUrl: "https://api.deepseek.com/v1" },
  { label: "Ollama", protocol: "chat_completions", baseUrl: "http://localhost:11434/v1" },
  { label: "LM Studio", protocol: "chat_completions", baseUrl: "http://localhost:1234/v1" },
];

const PROTOCOLS: { value: string; label: string }[] = [
  { value: "chat_completions", label: "OpenAI Chat Completions" },
  { value: "responses", label: "OpenAI Responses" },
  { value: "anthropic", label: "Anthropic Messages" },
];

function protocolLabel(value: string): string {
  return PROTOCOLS.find((p) => p.value === value)?.label ?? value;
}

export function ProvidersSection() {
  const [list, setList] = useState<ProviderView[]>([]);
  const [error, setError] = useState<string | null>(null);
  // `undefined` = dialog closed; `null` = creating a new one.
  const [editing, setEditing] = useState<ProviderView | null | undefined>(undefined);

  async function refresh() {
    try {
      setList(await aiApi.providers());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  return (
    <section className="settings__group">
      <div className="settings__group-title">模型服务商（AI 助手）</div>

      {list.length === 0 && (
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__hint" style={{ lineHeight: 1.7 }}>
              还没有配置。支持任意 OpenAI 兼容端点（OpenAI、OpenRouter、DeepSeek、Ollama、LM
              Studio、vLLM……）和 Anthropic，填地址和密钥即可。
            </div>
          </div>
        </div>
      )}

      {list.map((p) => (
        <div key={p.id} className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">{p.name || "未命名"}</div>
            <div className="settings__hint" style={{ fontFamily: "ui-monospace, Menlo, monospace" }}>
              {protocolLabel(p.protocol)} · {p.baseUrl}
            </div>
            <div className="settings__hint">
              {p.models.length > 0 ? `${p.models.length} 个模型：${p.models.join("、")}` : "还没添加模型"}
              {!p.hasKey && " · 未设置密钥"}
            </div>
          </div>
          <div style={{ display: "flex", gap: 6 }}>
            <button className="btn btn--sm" onClick={() => setEditing(p)}>
              编辑
            </button>
            <button
              className="btn btn--sm btn--ghost"
              onClick={async () => {
                try {
                  await aiApi.deleteProvider(p.id);
                  await refresh();
                } catch (e) {
                  setError(errorMessage(e));
                }
              }}
            >
              删除
            </button>
          </div>
        </div>
      ))}

      {error && <div className="error">{error}</div>}

      <div className="footerbar" style={{ marginTop: 10 }}>
        <button className="btn btn--primary" onClick={() => setEditing(null)}>
          <IconPlus size={14} /> 添加服务商
        </button>
      </div>

      {editing !== undefined && (
        <ProviderDialog
          initial={editing}
          onClose={() => setEditing(undefined)}
          onSaved={async () => {
            setEditing(undefined);
            await refresh();
          }}
        />
      )}
    </section>
  );
}

/** The add/edit form, in a nested dialog so the settings page keeps its place. */
function ProviderDialog({
  initial,
  onClose,
  onSaved,
}: {
  initial: ProviderView | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [protocol, setProtocol] = useState<string>(initial?.protocol ?? "chat_completions");
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl ?? "https://api.openai.com/v1");
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [models, setModels] = useState<string[]>(initial?.models ?? []);
  const [modelDraft, setModelDraft] = useState("");
  const [fetched, setFetched] = useState<string[]>([]);
  const [fetching, setFetching] = useState(false);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  /** The draft, as the backend commands expect it. */
  function draft(): ProviderInput {
    return {
      id: initial?.id,
      name: name.trim() || "未命名",
      protocol,
      baseUrl: baseUrl.trim(),
      models,
      // Blank means "keep the stored key", which is what the backend does.
      apiKey: apiKey.trim() || undefined,
    };
  }

  const canSave = name.trim() && baseUrl.trim() && models.length > 0 && !saving;

  function addModel(raw: string) {
    const m = raw.trim();
    if (!m) return;
    setModels((prev) => (prev.includes(m) ? prev : [...prev, m]));
    setModelDraft("");
  }

  async function fetchModels() {
    setFetching(true);
    setError(null);
    setNotice(null);
    try {
      const list = await aiApi.models(draft());
      setFetched(list);
      setNotice(list.length ? `服务商列出 ${list.length} 个模型，点一下即可添加。` : "服务商没有返回任何模型。");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setFetching(false);
    }
  }

  async function testConnection() {
    setTesting(true);
    setError(null);
    setNotice(null);
    try {
      setNotice(await aiApi.test(draft()));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setTesting(false);
    }
  }

  async function save() {
    if (!canSave) return;
    setSaving(true);
    setError(null);
    try {
      await aiApi.saveProvider(draft());
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
      setSaving(false);
    }
  }

  return (
    <Modal
      title={initial ? "编辑服务商" : "添加服务商"}
      onClose={onClose}
      width={640}
      className="provider-dialog"
    >
      <div className="provider-form">
        <div className="pfield">
          <label className="pfield__label">名称</label>
          <div className="pfield__body">
            <input
              className="input"
              placeholder="例如 OpenAI"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
            />
          </div>
        </div>

        <div className="pfield">
          <label className="pfield__label">协议</label>
          <div className="pfield__body">
            <select className="input" value={protocol} onChange={(e) => setProtocol(e.target.value)}>
              {PROTOCOLS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="pfield">
          <label className="pfield__label">基础 URL</label>
          <div className="pfield__body">
            <input
              className="input"
              placeholder="https://api.openai.com/v1"
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              spellCheck={false}
            />
            <div className="pfield__hint">地址要包含 API 版本，例如 https://api.openai.com/v1。</div>
            <div className="preset-row">
              <span className="preset-row__label">快速填入</span>
              {PRESETS.map((p) => (
                <button
                  key={p.label}
                  className="chip chip--sm"
                  onClick={() => {
                    setProtocol(p.protocol);
                    setBaseUrl(p.baseUrl);
                    if (!name.trim()) setName(p.label);
                  }}
                >
                  {p.label}
                </button>
              ))}
            </div>
          </div>
        </div>

        <div className="pfield">
          <label className="pfield__label">API 密钥</label>
          <div className="pfield__body">
            <div className="input-with-action">
              <input
                className="input"
                type={showKey ? "text" : "password"}
                placeholder={
                  initial?.hasKey ? "已保存，留空则不修改" : "不需要密钥的本地服务器可以留空"
                }
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                spellCheck={false}
              />
              <button
                className="iconbtn input-with-action__btn"
                onClick={() => setShowKey((v) => !v)}
                aria-label={showKey ? "隐藏密钥" : "显示密钥"}
                type="button"
              >
                {showKey ? <IconEyeOff size={15} /> : <IconEye size={15} />}
              </button>
            </div>
            <div className="pfield__hint">密钥和主机密码一样，加密后才写入本机数据库。</div>
          </div>
        </div>

        <div className="pfield">
          <label className="pfield__label">模型</label>
          <div className="pfield__body">
            <div className="model-head">
              <span className="pfield__hint" style={{ margin: 0 }}>
                输入模型 ID，或从服务商的列表里选。
              </span>
              <button className="btn btn--sm" onClick={() => void fetchModels()} disabled={fetching || !baseUrl.trim()}>
                <IconRefresh size={13} /> {fetching ? "获取中…" : "获取模型列表"}
              </button>
            </div>

            <div className="model-list">
              {models.length === 0 && (
                <div className="model-list__empty">
                  还没有模型。输入模型 ID，或获取服务商的模型列表。
                </div>
              )}
              {models.map((m) => (
                <div key={m} className="model-item">
                  <span className="model-item__name">{m}</span>
                  <button
                    className="iconbtn"
                    onClick={() => setModels(models.filter((x) => x !== m))}
                    aria-label={`移除 ${m}`}
                  >
                    <IconTrash size={13} />
                  </button>
                </div>
              ))}
            </div>

            {fetched.length > 0 && (
              <div className="fetched">
                <div className="pfield__hint">可用模型（点击添加）：</div>
                <div className="fetched__chips">
                  {fetched
                    .filter((f) => !models.includes(f))
                    .slice(0, 60)
                    .map((f) => (
                      <button key={f} className="chip chip--sm" onClick={() => addModel(f)}>
                        {f}
                      </button>
                    ))}
                  {fetched.every((f) => models.includes(f)) && (
                    <span className="pfield__hint">全部已添加。</span>
                  )}
                </div>
              </div>
            )}

            <div className="model-add">
              <input
                className="input"
                placeholder="输入模型 ID，回车添加"
                value={modelDraft}
                onChange={(e) => setModelDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    addModel(modelDraft);
                  }
                }}
              />
              <button className="btn" onClick={() => addModel(modelDraft)} disabled={!modelDraft.trim()}>
                添加
              </button>
            </div>
          </div>
        </div>

        {notice && <div className="notice notice--ok">{notice}</div>}
        {error && <div className="error">{error}</div>}

        <div className="provider-form__footer">
          <button className="btn" onClick={() => void testConnection()} disabled={testing || !baseUrl.trim()}>
            <IconPlug size={14} /> {testing ? "测试中…" : "测试连接"}
          </button>
          <div className="spacer" />
          <button className="btn btn--ghost" onClick={onClose}>
            取消
          </button>
          <button className="btn btn--primary" onClick={() => void save()} disabled={!canSave}>
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </Modal>
  );
}
