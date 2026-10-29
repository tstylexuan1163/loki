import { useEffect, useState } from "react";
import { confirmDialog, toast } from "@/components/ui";
import { IconTrash } from "@/components/icons";
import { errorMessage, knownHostsApi, type KnownHostView } from "@/lib/api";

/**
 * Trusted host keys. Removing one makes the next connection ask again, which
 * is the fix when a server was legitimately reinstalled.
 */
export function KnownHostsSection() {
  const [items, setItems] = useState<KnownHostView[] | null>(null);

  const refresh = () =>
    void knownHostsApi
      .list()
      .then(setItems)
      .catch((e) => toast(errorMessage(e), "error"));

  useEffect(refresh, []);

  async function remove(k: KnownHostView) {
    const ok = await confirmDialog({
      title: `忘记 ${k.host}:${k.port} 的主机密钥？`,
      body: "下次连接时会重新询问是否信任它的指纹。",
      confirmLabel: "忘记",
      danger: true,
    });
    if (!ok) return;
    await knownHostsApi.remove(k.id).catch((e) => toast(errorMessage(e), "error"));
    refresh();
  }

  return (
    <section className="settings__group">
      <div className="settings__group-title">已信任的主机密钥</div>
      {items && items.length === 0 && (
        <div className="settings__row">
          <div className="settings__hint">还没有。第一次连接一台主机并确认指纹后会出现在这里。</div>
        </div>
      )}
      {items?.map((k) => (
        <div key={k.id} className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label mono">
              {k.host}
              {k.port !== 22 ? `:${k.port}` : ""}
            </div>
            <div className="settings__hint mono">
              {k.keyType} · {k.fingerprint}
            </div>
          </div>
          <button className="iconbtn" title="忘记" onClick={() => void remove(k)}>
            <IconTrash size={14} />
          </button>
        </div>
      ))}
    </section>
  );
}
