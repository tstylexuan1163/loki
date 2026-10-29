import { useEffect, useState } from "react";
import { errorMessage, syncApi, type SyncReport, type SyncStatus } from "@/lib/api";

/**
 * Optional sync with a Cloudflare Worker the user deploys themselves.
 *
 * The Worker only ever holds ciphertext, so this screen can be honest about a
 * limited threat model: whoever runs the Worker can see how many items exist and
 * when they change, but not what they are.
 */
export function SyncSection() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [report, setReport] = useState<SyncReport | null>(null);

  async function refresh() {
    try {
      setStatus(await syncApi.status());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoaded(true);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function connect() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      setNote(await syncApi.connect(url.trim(), token.trim()));
      setToken("");
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function run() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      setReport(await syncApi.run());
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  if (!loaded) return null;

  return (
    <div className="editor__section">
      <div className="editor__legend">同步（可选）</div>

      {!status ? (
        <>
          <div className="muted" style={{ lineHeight: 1.7, marginBottom: 12 }}>
            把加密后的保险库同步到你自己的 Cloudflare 账号。
            <br />
            服务端只存密文和版本号；谁能访问你的 D1，就能看到有多少条、何时改动，但看不到内容。
          </div>

          <div className="field">
            <label className="field__label">Worker 地址</label>
            <input
              className="field__input"
              placeholder="https://loki-sync.你的账号.workers.dev"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
            />
          </div>

          <div className="field">
            <label className="field__label">访问令牌</label>
            <input
              className="field__input"
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
          </div>

          <div className="footerbar">
            <button
              className="btn btn--primary"
              disabled={busy || !url.trim() || !token.trim()}
              onClick={() => void connect()}
            >
              {busy ? "连接中…" : "连接"}
            </button>
          </div>
        </>
      ) : (
        <>
          <div className="muted" style={{ marginBottom: 10 }}>
            已连接：<code>{status.url}</code>
            <br />
            游标位置 {status.cursor}
          </div>

          <div className="footerbar" style={{ justifyContent: "flex-start" }}>
            <button className="btn btn--primary" disabled={busy} onClick={() => void run()}>
              {busy ? "同步中…" : "立即同步"}
            </button>
            <button
              className="btn btn--ghost"
              disabled={busy}
              onClick={async () => {
                await syncApi.disconnect();
                setStatus(null);
                setReport(null);
              }}
            >
              断开
            </button>
          </div>

          {report && (
            <div className="notice" style={{ marginTop: 12 }}>
              推送 {report.pushed} 条，拉取 {report.pulled} 条
              {report.rejected > 0 ? `，被拒绝 ${report.rejected} 条（远端更新）` : ""}。
            </div>
          )}
        </>
      )}

      {note && <div className="notice">{note}</div>}
      {error && <div className="error">{error}</div>}
    </div>
  );
}
