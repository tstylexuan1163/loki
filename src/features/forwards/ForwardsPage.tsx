import { useEffect, useState } from "react";
import { useApp } from "@/app/store";
import { useTerminals } from "@/features/terminal/terminals";
import { Segmented, toast } from "@/components/ui";
import { IconCopy, IconForward, IconGlobe, IconStop } from "@/components/icons";
import { errorMessage, forwardApi, type ForwardView } from "@/lib/api";

/**
 * Port forwarding, scoped to a live SSH session.
 *
 * A forward needs a connection to ride on, so the form picks one of the open
 * SSH tabs; with none open it says so instead of failing on submit.
 */
export function ForwardsPage() {
  const tabs = useTerminals((s) => s.tabs);
  const active = useTerminals((s) => s.active);
  const connected = tabs.filter((t) => t.kind === "ssh" && t.status === "connected" && t.sessionId);

  const [sessionId, setSessionId] = useState<string>("");
  const [forwards, setForwards] = useState<ForwardView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [kind, setKind] = useState<"local" | "socks">("local");
  const [localPort, setLocalPort] = useState("8080");
  const [remoteHost, setRemoteHost] = useState("127.0.0.1");
  const [remotePort, setRemotePort] = useState("80");
  const [busy, setBusy] = useState(false);

  // Default to the active tab's session, or the first connected one.
  useEffect(() => {
    if (connected.some((t) => t.sessionId === sessionId)) return;
    const preferred = connected.find((t) => t.id === active) ?? connected[0];
    setSessionId(preferred?.sessionId ?? "");
  }, [connected, active, sessionId]);

  async function refresh() {
    try {
      setForwards(await forwardApi.list());
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  useEffect(() => {
    void refresh();
  }, [tabs.length]);

  async function start() {
    if (!sessionId) return;
    setBusy(true);
    try {
      const view = await forwardApi.start(sessionId, {
        kind,
        localPort: Number(localPort) || 0,
        remoteHost: kind === "local" ? remoteHost : null,
        remotePort: kind === "local" ? Number(remotePort) || 0 : null,
      });
      await refresh();
      setError(null);
      toast(`已在 127.0.0.1:${view.localPort} 开始转发`, "success");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const titleOf = (sid: string) => tabs.find((t) => t.sessionId === sid)?.title ?? "已断开的会话";

  return (
    <div className="page">
      <div className="pagehead">
        <div className="pagehead__title">
          端口转发
          {forwards.length > 0 && <span className="pagehead__count">{forwards.length}</span>}
        </div>
      </div>
      <div className="content">
        <div className="editor">
          <section className="card">
            <div className="card__title">新建转发</div>

            {connected.length === 0 ? (
              <div className="notice">
                端口转发需要一个已连接的 SSH 会话。先在
                <button className="linkbtn" onClick={() => useApp.setState({ page: { kind: "hosts" } })}>
                  主机列表
                </button>
                里连接一台服务器。
              </div>
            ) : (
              <div className="field">
                <label className="field__label">经由会话</label>
                <select className="field__input" value={sessionId} onChange={(e) => setSessionId(e.target.value)}>
                  {connected.map((t) => (
                    <option key={t.id} value={t.sessionId!}>
                      {t.title}
                    </option>
                  ))}
                </select>
              </div>
            )}

            <div className="field">
              <label className="field__label">类型</label>
              <Segmented
                value={kind}
                onChange={setKind}
                options={[
                  { value: "local", label: "本地转发 -L" },
                  { value: "socks", label: "SOCKS5 -D" },
                ]}
              />
              <div className="field__hint">
                {kind === "local"
                  ? "把本机端口转发到远端网络里的某个地址，比如只在内网开放的数据库。"
                  : "在本机开一个 SOCKS5 代理，浏览器的流量经这台服务器出去。"}
              </div>
            </div>

            <div className="forward-form">
              <div className="field">
                <label className="field__label">本地端口</label>
                <input
                  className="field__input mono"
                  value={localPort}
                  placeholder="0 = 自动"
                  onChange={(e) => setLocalPort(e.target.value.replace(/\D/g, ""))}
                />
              </div>
              {kind === "local" && (
                <>
                  <span className="forward-form__arrow">
                    <IconForward size={16} />
                  </span>
                  <div className="field" style={{ flex: 2 }}>
                    <label className="field__label">远端地址</label>
                    <input
                      className="field__input mono"
                      value={remoteHost}
                      onChange={(e) => setRemoteHost(e.target.value)}
                    />
                  </div>
                  <div className="field">
                    <label className="field__label">远端端口</label>
                    <input
                      className="field__input mono"
                      value={remotePort}
                      onChange={(e) => setRemotePort(e.target.value.replace(/\D/g, ""))}
                    />
                  </div>
                </>
              )}
            </div>

            {error && <div className="error">{error}</div>}

            <div className="footerbar">
              <button className="btn btn--primary" disabled={!sessionId || busy} onClick={() => void start()}>
                开始转发
              </button>
            </div>
          </section>

          <section className="card">
            <div className="card__title">运行中</div>
            {forwards.length === 0 && <div className="muted">还没有转发。</div>}
            <div className="fwdlist">
              {forwards.map((f) => (
                <div key={f.id} className="fwdrow">
                  <span className="fwdrow__icon">
                    {f.kind === "socks" ? <IconGlobe size={15} /> : <IconForward size={15} />}
                  </span>
                  <div className="fwdrow__main">
                    <div className="mono">
                      127.0.0.1:{f.localPort}
                      {f.remote ? ` → ${f.remote}` : " · SOCKS5"}
                    </div>
                    <div className="fwdrow__sub">经 {titleOf(f.sessionId)}</div>
                  </div>
                  <button
                    className="iconbtn"
                    title="复制本地地址"
                    onClick={() => {
                      void navigator.clipboard.writeText(`127.0.0.1:${f.localPort}`);
                      toast("已复制");
                    }}
                  >
                    <IconCopy size={14} />
                  </button>
                  <button
                    className="btn btn--sm btn--ghost"
                    onClick={async () => {
                      await forwardApi.stop(f.id).catch(() => {});
                      await refresh();
                    }}
                  >
                    <IconStop size={12} /> 停止
                  </button>
                </div>
              ))}
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
