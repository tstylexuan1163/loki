import { useEffect, useState } from "react";
import { useApp } from "@/app/store";
import { Modal, toast } from "@/components/ui";
import { errorMessage, importApi, type SshConfigCandidate } from "@/lib/api";

/** Imports hosts from `~/.ssh/config`, letting the user pick which. */
export function ImportDialog({ onClose }: { onClose: () => void }) {
  const refreshHosts = useApp((s) => s.refreshHosts);
  const [items, setItems] = useState<SshConfigCandidate[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    importApi
      .scan()
      .then((list) => {
        setItems(list);
        setPicked(new Set(list.filter((c) => !c.exists).map((c) => c.alias)));
      })
      .catch((e) => setError(errorMessage(e)));
  }, []);

  const toggle = (alias: string) => {
    const next = new Set(picked);
    if (next.has(alias)) next.delete(alias);
    else next.add(alias);
    setPicked(next);
  };

  async function run() {
    setBusy(true);
    try {
      const report = await importApi.run([...picked]);
      await refreshHosts();
      toast(
        `已导入 ${report.hosts} 台主机${report.keys ? `、${report.keys} 把密钥` : ""}`,
        "success",
      );
      for (const n of report.notes) toast(n);
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <Modal
      title="从 ~/.ssh/config 导入"
      subtitle="私钥文件会读入密钥库并加密保存；需要口令的私钥会留给 ssh-agent。"
      onClose={onClose}
      width={620}
    >
      {error && <div className="error">{error}</div>}
      {!items && !error && <div className="muted" style={{ padding: 16 }}>正在读取…</div>}
      {items && items.length === 0 && (
        <div className="muted" style={{ padding: 16 }}>配置文件里没有可导入的 Host。</div>
      )}
      {items && items.length > 0 && (
        <>
          <div className="importlist__bar">
            <label className="check">
              <input
                type="checkbox"
                checked={picked.size === items.length}
                onChange={(e) =>
                  setPicked(e.target.checked ? new Set(items.map((i) => i.alias)) : new Set())
                }
              />
              全选
            </label>
            <span className="muted">已选 {picked.size} / {items.length}</span>
          </div>
          <div className="importlist">
            {items.map((c) => (
              <label key={c.alias} className={`importrow ${c.exists ? "importrow--dim" : ""}`}>
                <input type="checkbox" checked={picked.has(c.alias)} onChange={() => toggle(c.alias)} />
                <span className="importrow__name">{c.alias}</span>
                <span className="importrow__addr mono">
                  {c.user}@{c.hostname}
                  {c.port !== 22 ? `:${c.port}` : ""}
                </span>
                <span className="importrow__tags">
                  {c.identityFile && <span className="pill">密钥</span>}
                  {c.proxyJump && <span className="pill">跳板 {c.proxyJump}</span>}
                  {c.exists && <span className="pill pill--warn">已存在</span>}
                </span>
              </label>
            ))}
          </div>
        </>
      )}
      <div className="footerbar">
        <button className="btn btn--ghost" onClick={onClose}>
          取消
        </button>
        <button className="btn btn--primary" disabled={busy || picked.size === 0} onClick={() => void run()}>
          {busy ? "导入中…" : `导入 ${picked.size} 台`}
        </button>
      </div>
    </Modal>
  );
}
