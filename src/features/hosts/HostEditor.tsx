import { useEffect, useMemo, useState } from "react";
import {
  api,
  directApi,
  errorMessage,
  keyApi,
  categoryColor,
  type KeyView,
  type Protocol,
  type SerialPortInfo,
} from "@/lib/api";
import { useApp, type HostPrefill } from "@/app/store";
import { Switch, confirmDialog, toast } from "@/components/ui";
import {
  IconArrowUp,
  IconCheck,
  IconGlobe,
  IconKey,
  IconRefresh,
  IconServer,
  IconTrash,
  IconUsb,
} from "@/components/icons";

const BAUD_RATES = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400];

const PROTOCOLS: { id: Protocol; label: string; hint: string; icon: React.ReactNode }[] = [
  { id: "ssh", label: "SSH", hint: "加密远程登录", icon: <IconServer size={16} /> },
  { id: "telnet", label: "Telnet", hint: "明文，仅限内网设备", icon: <IconGlobe size={16} /> },
  { id: "serial", label: "串口", hint: "交换机、开发板控制台", icon: <IconUsb size={16} /> },
];

export function HostEditor({
  hostId,
  prefill,
}: {
  hostId: string | null;
  /** Values from "save as host" on a quick-connect tab. */
  prefill?: HostPrefill;
}) {
  const hosts = useApp((s) => s.hosts);
  const categories = useApp((s) => s.categories);
  const backToHosts = useApp((s) => s.backToHosts);
  const refreshHosts = useApp((s) => s.refreshHosts);
  const existing = useMemo(() => hosts.find((h) => h.id === hostId) ?? null, [hosts, hostId]);

  // A saved host wins; otherwise the prefill from a quick-connect tab seeds the
  // form so the user only has to add credentials and a name.
  const seed = useMemo(() => {
    if (existing) {
      return {
        protocol: existing.protocol,
        name: existing.name,
        address: existing.address,
        port: existing.port,
        username: existing.username,
        baud: existing.baud ?? undefined,
        dataBits: existing.data_bits ?? undefined,
        parity: existing.parity ?? undefined,
        stopBits: existing.stop_bits ?? undefined,
      };
    }
    return prefill;
  }, [existing, prefill]);

  const [protocol, setProtocol] = useState<Protocol>(seed?.protocol ?? "ssh");
  const [name, setName] = useState(seed?.name ?? "");
  const [address, setAddress] = useState(seed?.address ?? "");
  const [port, setPort] = useState(String(seed?.port ?? (seed?.protocol === "telnet" ? 23 : 22)));
  const [username, setUsername] = useState(seed?.username ?? "");
  const [baud, setBaud] = useState(String(seed?.baud ?? 115200));
  const [dataBits, setDataBits] = useState(String(seed?.dataBits ?? 8));
  const [parity, setParity] = useState(seed?.parity ?? "none");
  const [stopBits, setStopBits] = useState(String(seed?.stopBits ?? 1));
  const [ports, setPorts] = useState<SerialPortInfo[]>([]);
  const [tags, setTags] = useState<string[]>(existing?.tags ?? []);
  const [tagDraft, setTagDraft] = useState("");
  const [password, setPassword] = useState("");
  const [clearPassword, setClearPassword] = useState(false);
  const [privateKey, setPrivateKey] = useState("");
  const [clearKey, setClearKey] = useState(false);
  const [showPaste, setShowPaste] = useState(false);
  const [keyId, setKeyId] = useState<string>(existing?.key_id ?? "");
  const [useAgent, setUseAgent] = useState(existing?.use_agent ?? true);
  const [jumpHostId, setJumpHostId] = useState<string>(existing?.jump_host_id ?? "");
  const [startup, setStartup] = useState(existing?.startup_command ?? "");
  const [keys, setKeys] = useState<KeyView[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void keyApi.list().then(setKeys).catch(() => setKeys([]));
  }, []);

  const refreshPorts = () =>
    void directApi
      .serialPorts()
      .then(setPorts)
      .catch(() => setPorts([]));

  // Only worth asking for the port list once the user picks serial.
  useEffect(() => {
    if (protocol === "serial") refreshPorts();
  }, [protocol]);

  const isSsh = protocol === "ssh";
  const jumpCandidates = hosts.filter((h) => h.protocol === "ssh" && h.id !== hostId);

  // A login name is optional: the connection asks for it when it is missing.
  const canSave = name.trim().length > 0 && address.trim().length > 0;

  // Every known category, plus names picked here that are not saved yet.
  const choices = useMemo(() => {
    const names = categories.map((c) => c.name);
    for (const t of tags) if (!names.some((n) => n.toLowerCase() === t.toLowerCase())) names.push(t);
    return names;
  }, [categories, tags]);

  function toggleTag(t: string) {
    setTags((cur) =>
      cur.some((x) => x.toLowerCase() === t.toLowerCase())
        ? cur.filter((x) => x.toLowerCase() !== t.toLowerCase())
        : [...cur, t],
    );
  }

  function commitTag() {
    const t = tagDraft.trim().replace(/,$/, "");
    // Typing an existing name selects it rather than creating a twin.
    const known = choices.find((c) => c.toLowerCase() === t.toLowerCase());
    if (t && !tags.some((x) => x.toLowerCase() === t.toLowerCase())) setTags([...tags, known ?? t]);
    setTagDraft("");
  }

  async function save() {
    if (!canSave || saving) return;
    setSaving(true);
    setError(null);
    try {
      await api.hostSave({
        id: hostId ?? undefined,
        name: name.trim(),
        protocol,
        address: address.trim(),
        port: Number(port) || (protocol === "ssh" ? 22 : 23),
        username: username.trim(),
        tags,
        baud: protocol === "serial" ? Number(baud) || 115200 : null,
        dataBits: protocol === "serial" ? Number(dataBits) || 8 : null,
        parity: protocol === "serial" ? parity : null,
        stopBits: protocol === "serial" ? Number(stopBits) || 1 : null,
        // Blank means "keep what is stored"; editing must not wipe a secret the
        // user never retyped. Clearing is an explicit action.
        password: password.length > 0 ? password : undefined,
        privateKey: privateKey.trim().length > 0 ? privateKey : undefined,
        clearPassword: clearPassword && !password,
        clearKey: clearKey && !privateKey.trim(),
        keyId: isSsh && keyId ? keyId : null,
        jumpHostId: isSsh && jumpHostId ? jumpHostId : null,
        useAgent: isSsh ? useAgent : false,
        startupCommand: isSsh && startup.trim() ? startup : null,
      });
      await refreshHosts();
      toast(hostId ? "已保存" : `已添加 ${name.trim()}`, "success");
      backToHosts();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (!existing) return;
    const ok = await confirmDialog({
      title: `删除 ${existing.name || existing.address}？`,
      body: "主机和它保存的密码、私钥都会从保险库中删除。",
      confirmLabel: "删除",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.hostDelete(existing.id);
      await refreshHosts();
      backToHosts();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const storedKey = existing?.has_key && !clearKey;
  const storedPassword = existing?.has_password && !clearPassword;

  return (
    <div className="page">
      <div className="pagehead">
        <button className="iconbtn" onClick={backToHosts} title="返回主机列表">
          <IconArrowUp size={16} style={{ transform: "rotate(-90deg)" }} />
        </button>
        <div className="pagehead__title">{hostId ? existing?.name || "编辑主机" : "新建主机"}</div>
        <div className="spacer" />
        {existing && (
          <button className="btn btn--ghost btn--danger-text" onClick={() => void remove()}>
            <IconTrash size={14} /> 删除
          </button>
        )}
        <button className="btn btn--ghost" onClick={backToHosts}>
          取消
        </button>
        <button className="btn btn--primary" disabled={!canSave || saving} onClick={() => void save()}>
          {saving ? "保存中…" : "保存"}
        </button>
      </div>

      <div className="content">
        <form
          className="editor"
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          {error && <div className="error">{error}</div>}

          <section className="card">
            <div className="card__title">连接方式</div>
            <div className="proto-picker">
              {PROTOCOLS.map((p) => (
                <button
                  type="button"
                  key={p.id}
                  className={`proto ${protocol === p.id ? "proto--on" : ""}`}
                  onClick={() => {
                    setProtocol(p.id);
                    if (p.id === "ssh" && port === "23") setPort("22");
                    if (p.id === "telnet" && port === "22") setPort("23");
                  }}
                >
                  <span className="proto__icon">{p.icon}</span>
                  <span className="proto__label">{p.label}</span>
                  <span className="proto__hint">{p.hint}</span>
                </button>
              ))}
            </div>
          </section>

          <section className="card">
            <div className="card__title">基本信息</div>

            <div className="field">
              <label className="field__label">名称</label>
              <input
                className="field__input"
                autoFocus={!hostId}
                placeholder="例如 prod-api-tokyo"
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </div>

            {protocol === "serial" ? (
              <div className="field">
                <label className="field__label">串口设备</label>
                <div className="row">
                  <input
                    className="field__input"
                    placeholder="/dev/tty.usbserial-1420"
                    value={address}
                    onChange={(e) => setAddress(e.target.value)}
                    style={{ flex: 1 }}
                  />
                  <select
                    className="field__input"
                    style={{ width: 200 }}
                    value=""
                    onChange={(e) => {
                      if (e.target.value) setAddress(e.target.value);
                    }}
                  >
                    <option value="">{ports.length ? `检测到 ${ports.length} 个` : "未检测到设备"}</option>
                    {ports.map((p) => (
                      <option key={p.path} value={p.path}>
                        {p.path}
                      </option>
                    ))}
                  </select>
                  <button type="button" className="iconbtn" onClick={refreshPorts} title="重新扫描">
                    <IconRefresh size={15} />
                  </button>
                </div>
              </div>
            ) : (
              <div className="grid-addr">
                <div className="field">
                  <label className="field__label">地址</label>
                  <input
                    className="field__input"
                    placeholder="域名或 IP 地址"
                    value={address}
                    onChange={(e) => setAddress(e.target.value)}
                  />
                </div>
                <div className="field">
                  <label className="field__label">端口</label>
                  <input
                    className="field__input"
                    inputMode="numeric"
                    value={port}
                    onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
                  />
                </div>
              </div>
            )}

            {isSsh && (
              <div className="field">
                <label className="field__label">用户名</label>
                <input
                  className="field__input"
                  placeholder="留空则连接时询问"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                />
              </div>
            )}

            {protocol === "serial" && (
              <div className="grid4">
                <div className="field">
                  <label className="field__label">波特率</label>
                  <select className="field__input" value={baud} onChange={(e) => setBaud(e.target.value)}>
                    {BAUD_RATES.map((b) => (
                      <option key={b} value={b}>
                        {b}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field">
                  <label className="field__label">数据位</label>
                  <select className="field__input" value={dataBits} onChange={(e) => setDataBits(e.target.value)}>
                    {[5, 6, 7, 8].map((b) => (
                      <option key={b} value={b}>
                        {b}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field">
                  <label className="field__label">校验</label>
                  <select className="field__input" value={parity} onChange={(e) => setParity(e.target.value)}>
                    <option value="none">无</option>
                    <option value="odd">奇</option>
                    <option value="even">偶</option>
                  </select>
                </div>
                <div className="field">
                  <label className="field__label">停止位</label>
                  <select className="field__input" value={stopBits} onChange={(e) => setStopBits(e.target.value)}>
                    <option value="1">1</option>
                    <option value="2">2</option>
                  </select>
                </div>
              </div>
            )}

            {protocol === "telnet" && (
              <div className="notice">
                Telnet 是<strong>明文</strong>协议：密码和屏幕内容都可能被同网段的人看到。只建议用于内网设备或临时调试。
              </div>
            )}
          </section>

          {isSsh && (
            <section className="card">
              <div className="card__title">认证</div>
              <div className="card__hint">
                连接时依次尝试：私钥 → 密钥库中的密钥 → ssh-agent → 密码。都不行时会弹出密码输入框。
              </div>

              <div className="field">
                <label className="field__label">
                  密码
                  {storedPassword && (
                    <span className="field__aside">
                      已保存 ·{" "}
                      <button type="button" className="linkbtn" onClick={() => setClearPassword(true)}>
                        移除
                      </button>
                    </span>
                  )}
                </label>
                <input
                  className="field__input"
                  type="password"
                  autoComplete="new-password"
                  placeholder={
                    storedPassword
                      ? "输入新密码以替换，留空保持不变"
                      : clearPassword
                        ? "保存后将移除已存的密码"
                        : "留空则连接时再问"
                  }
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
              </div>

              <div className="field">
                <label className="field__label">密钥库中的密钥</label>
                <select className="field__input" value={keyId} onChange={(e) => setKeyId(e.target.value)}>
                  <option value="">不使用</option>
                  {keys.map((k) => (
                    <option key={k.id} value={k.id}>
                      {k.name} · {k.kind} · {k.fingerprint.slice(7, 19)}…
                    </option>
                  ))}
                </select>
                {keys.length === 0 && (
                  <div className="field__hint">密钥库是空的，可以到「密钥库」生成或导入。</div>
                )}
              </div>

              <div className="field">
                <label className="field__label">此主机专用私钥</label>
                {storedKey && !showPaste ? (
                  <div className="stored">
                    <IconKey size={14} />
                    <span className="stored__label">已保存私钥</span>
                    <button type="button" className="btn btn--sm btn--ghost" onClick={() => setShowPaste(true)}>
                      替换
                    </button>
                    <button type="button" className="btn btn--sm btn--ghost" onClick={() => setClearKey(true)}>
                      移除
                    </button>
                  </div>
                ) : showPaste ? (
                  <textarea
                    className="field__input field__input--code"
                    spellCheck={false}
                    autoCorrect="off"
                    autoCapitalize="off"
                    autoFocus
                    placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
                    value={privateKey}
                    onChange={(e) => setPrivateKey(e.target.value)}
                  />
                ) : (
                  <button type="button" className="btn btn--ghost btn--dashed" onClick={() => setShowPaste(true)}>
                    {clearKey ? "保存后将移除已存的私钥 · 粘贴新私钥…" : "粘贴私钥…"}
                  </button>
                )}
                <div className="field__hint">密码与私钥都和主机信息一起加密存储，前端永远拿不到明文。</div>
              </div>

              <div className="setting">
                <div className="setting__main">
                  <div className="setting__label">使用 ssh-agent</div>
                  <div className="setting__hint">尝试本机 ssh-agent 中的密钥（1Password、Secretive 等也走这里）。</div>
                </div>
                <Switch checked={useAgent} onChange={setUseAgent} label="使用 ssh-agent" />
              </div>
            </section>
          )}

          {isSsh && (
            <section className="card">
              <div className="card__title">高级</div>
              <div className="field">
                <label className="field__label">跳板机（ProxyJump）</label>
                <select
                  className="field__input"
                  value={jumpHostId}
                  onChange={(e) => setJumpHostId(e.target.value)}
                >
                  <option value="">直接连接</option>
                  {jumpCandidates.map((h) => (
                    <option key={h.id} value={h.id}>
                      {h.name} · {h.username}@{h.address}
                    </option>
                  ))}
                </select>
                <div className="field__hint">先登录跳板机，再经它的隧道连到这台主机。</div>
              </div>
              <div className="field">
                <label className="field__label">登录后执行</label>
                <input
                  className="field__input mono"
                  placeholder="例如 cd /srv/app && tmux attach"
                  value={startup}
                  onChange={(e) => setStartup(e.target.value)}
                />
              </div>
            </section>
          )}

          <section className="card">
            <div className="card__title">分类</div>
            <div className="card__hint">可以选多个；都不选就放在「未分类」。</div>
            <div className="catpick">
              {choices.map((t) => {
                const on = tags.some((x) => x.toLowerCase() === t.toLowerCase());
                return (
                  <button
                    key={t}
                    type="button"
                    className={`chip ${on ? "chip--on" : ""}`}
                    aria-pressed={on}
                    onClick={() => toggleTag(t)}
                  >
                    <i className="tag__dot" style={{ background: categoryColor(t, categories) }} />
                    {t}
                    {on && <IconCheck size={11} />}
                  </button>
                );
              })}
              <input
                className="catpick__new"
                placeholder="新建分类，回车添加"
                maxLength={32}
                value={tagDraft}
                onChange={(e) => setTagDraft(e.target.value)}
                onBlur={commitTag}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === ",") {
                    e.preventDefault();
                    commitTag();
                  }
                }}
              />
            </div>
          </section>
          <button type="submit" hidden />
        </form>
      </div>
    </div>
  );
}
