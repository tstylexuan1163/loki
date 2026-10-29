import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { errorMessage, keyApi, type KeyView } from "@/lib/api";
import { Menu, Modal, Segmented, confirmDialog, toast, useMenu } from "@/components/ui";
import { IconCopy, IconEdit, IconImport, IconKey, IconMore, IconPlus, IconTrash } from "@/components/icons";

/**
 * Key management.
 *
 * The private half of every key lives encrypted in the vault and is never sent
 * to the WebView — only the public key and its fingerprint are.
 */
export function KeysPage() {
  const [keys, setKeys] = useState<KeyView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"generate" | "import" | null>(null);
  const [renaming, setRenaming] = useState<KeyView | null>(null);
  const menu = useMenu<KeyView>();

  async function refresh() {
    try {
      setKeys(await keyApi.list());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function remove(k: KeyView) {
    const ok = await confirmDialog({
      title: `删除密钥 ${k.name}？`,
      body: "私钥会从保险库中永久删除。引用它的主机将无法再用它登录。",
      confirmLabel: "删除",
      danger: true,
    });
    if (!ok) return;
    try {
      await keyApi.remove(k.id);
      await refresh();
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  const copy = (k: KeyView) => {
    void navigator.clipboard.writeText(k.publicKey);
    toast("公钥已复制，可粘贴到服务器的 ~/.ssh/authorized_keys", "success");
  };

  const list = keys ?? [];

  return (
    <div className="page">
      <div className="pagehead">
        <div className="pagehead__title">
          密钥库
          {list.length > 0 && <span className="pagehead__count">{list.length}</span>}
        </div>
        <div className="spacer" />
        <button className="btn btn--ghost" onClick={() => setDialog("import")}>
          <IconImport size={14} /> 导入私钥
        </button>
        <button className="btn btn--primary" onClick={() => setDialog("generate")}>
          <IconPlus size={14} /> 生成密钥
        </button>
      </div>

      <div className="content">
        {error && <div className="error">{error}</div>}

        {keys && list.length === 0 && !error && (
          <div className="content--center" style={{ height: "70%" }}>
            <div className="empty">
              <div className="empty__icon">
                <IconKey size={28} />
              </div>
              <div className="empty__title">还没有密钥</div>
              <div className="empty__hint">
                生成一把 Ed25519 密钥，把公钥放到服务器上，就可以免密码登录。也可以导入已有的私钥。
              </div>
              <div className="empty__actions">
                <button className="btn btn--primary" onClick={() => setDialog("generate")}>
                  <IconPlus size={14} /> 生成密钥
                </button>
                <button className="btn" onClick={() => setDialog("import")}>
                  <IconImport size={14} /> 导入私钥
                </button>
              </div>
            </div>
          </div>
        )}

        <div className="keylist">
          {list.map((k) => (
            <div key={k.id} className="keycard" onContextMenu={(e) => menu.open(e, k)}>
              <span className="keycard__icon">
                <IconKey size={18} />
              </span>
              <div className="keycard__main">
                <div className="keycard__name">
                  {k.name}
                  <span className="pill">{k.kind.toUpperCase()}</span>
                </div>
                <div className="keycard__fp mono">{k.fingerprint}</div>
                <div className="keycard__pub mono" title={k.publicKey}>
                  {k.publicKey}
                </div>
              </div>
              <div className="keycard__actions">
                <button className="btn btn--sm" onClick={() => copy(k)}>
                  <IconCopy size={13} /> 复制公钥
                </button>
                <button className="iconbtn" onClick={(e) => menu.open(e, k)} aria-label="更多">
                  <IconMore size={16} />
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>

      {menu.menu && (
        <Menu
          x={menu.menu.x}
          y={menu.menu.y}
          onClose={menu.close}
          items={[
            { label: "复制公钥", icon: <IconCopy size={14} />, onSelect: () => copy(menu.menu!.data) },
            {
              label: "复制指纹",
              icon: <IconCopy size={14} />,
              onSelect: () => {
                void navigator.clipboard.writeText(menu.menu!.data.fingerprint);
                toast("指纹已复制");
              },
            },
            { label: "重命名", icon: <IconEdit size={14} />, onSelect: () => setRenaming(menu.menu!.data) },
            "sep",
            { label: "删除", danger: true, icon: <IconTrash size={14} />, onSelect: () => void remove(menu.menu!.data) },
          ]}
        />
      )}

      {dialog === "generate" && (
        <GenerateDialog
          onClose={() => setDialog(null)}
          onCreated={async (k) => {
            setDialog(null);
            await refresh();
            toast(`已生成 ${k.name}`, "success");
          }}
        />
      )}
      {dialog === "import" && (
        <ImportKeyDialog
          onClose={() => setDialog(null)}
          onCreated={async (k) => {
            setDialog(null);
            await refresh();
            toast(`已导入 ${k.name}`, "success");
          }}
        />
      )}
      {renaming && (
        <RenameDialog
          keyView={renaming}
          onClose={() => setRenaming(null)}
          onDone={async () => {
            setRenaming(null);
            await refresh();
          }}
        />
      )}
    </div>
  );
}

function GenerateDialog({ onClose, onCreated }: { onClose: () => void; onCreated: (k: KeyView) => void }) {
  const today = new Date().toISOString().slice(0, 10);
  const [name, setName] = useState(`loki-${today}`);
  const [kind, setKind] = useState("ed25519");
  const [comment, setComment] = useState("loki");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function generate() {
    setBusy(true);
    setError(null);
    try {
      onCreated(await keyApi.generate({ name: name.trim() || "loki-key", kind, comment }));
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <Modal title="生成新密钥" onClose={onClose} width={460}>
      <div className="field">
        <label className="field__label">名称</label>
        <input className="field__input" value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field">
        <label className="field__label">类型</label>
        <Segmented
          value={kind}
          onChange={setKind}
          options={[
            { value: "ed25519", label: "Ed25519" },
            { value: "rsa4096", label: "RSA 4096" },
          ]}
        />
        <div className="field__hint">
          {kind === "ed25519" ? "推荐：更短、更快，现代服务器普遍支持。" : "兼容老服务器；生成需要几秒。"}
        </div>
      </div>
      <div className="field">
        <label className="field__label">注释</label>
        <input className="field__input" value={comment} onChange={(e) => setComment(e.target.value)} />
      </div>
      {error && <div className="error">{error}</div>}
      <div className="footerbar">
        <button className="btn btn--ghost" onClick={onClose}>
          取消
        </button>
        <button className="btn btn--primary" disabled={busy} onClick={() => void generate()}>
          {busy ? "生成中…" : "生成"}
        </button>
      </div>
    </Modal>
  );
}

function ImportKeyDialog({ onClose, onCreated }: { onClose: () => void; onCreated: (k: KeyView) => void }) {
  const [name, setName] = useState("");
  const [pem, setPem] = useState("");
  // A picked file is read by Rust, so the private key never enters the WebView.
  const [path, setPath] = useState<string | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [needsPass, setNeedsPass] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function pickFile() {
    const picked = await openDialog({ multiple: false, title: "选择私钥文件" });
    if (!picked || Array.isArray(picked)) return;
    setPath(picked);
    setPem("");
    if (!name) setName(picked.split("/").pop() ?? "");
  }

  async function run() {
    setBusy(true);
    setError(null);
    try {
      onCreated(
        await keyApi.import({
          name,
          privateKey: pem || undefined,
          path: pem ? undefined : (path ?? undefined),
          passphrase: passphrase || undefined,
        }),
      );
    } catch (e) {
      const msg = errorMessage(e);
      if (msg.includes("口令")) setNeedsPass(true);
      setError(msg);
      setBusy(false);
    }
  }

  return (
    <Modal
      title="导入私钥"
      subtitle="支持 OpenSSH、PEM（PKCS#1/PKCS#8）格式。私钥会加密存进保险库。"
      onClose={onClose}
      width={560}
    >
      <div className="field">
        <label className="field__label">名称</label>
        <input className="field__input" placeholder="id_ed25519" value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field">
        <label className="field__label">
          私钥
          <span className="field__aside">
            <button type="button" className="linkbtn" onClick={() => void pickFile()}>
              从文件读取…
            </button>
          </span>
        </label>
        {path && !pem ? (
          <div className="stored">
            <span className="mono stored__label">{path}</span>
            <button type="button" className="btn btn--sm btn--ghost" onClick={() => setPath(null)}>
              改为粘贴
            </button>
          </div>
        ) : (
          <textarea
            className="field__input field__input--code"
                    spellCheck={false}
                    autoCorrect="off"
                    autoCapitalize="off"
            style={{ height: 160 }}
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
            value={pem}
            onChange={(e) => setPem(e.target.value)}
          />
        )}
      </div>
      {needsPass && (
        <div className="field">
          <label className="field__label">口令</label>
          <input
            className="field__input"
            type="password"
            autoFocus
            value={passphrase}
            onChange={(e) => setPassphrase(e.target.value)}
          />
        </div>
      )}
      {error && <div className="error">{error}</div>}
      <div className="footerbar">
        <button className="btn btn--ghost" onClick={onClose}>
          取消
        </button>
        <button className="btn btn--primary" disabled={busy || (!pem.trim() && !path)} onClick={() => void run()}>
          {busy ? "导入中…" : "导入"}
        </button>
      </div>
    </Modal>
  );
}

function RenameDialog({ keyView, onClose, onDone }: { keyView: KeyView; onClose: () => void; onDone: () => void }) {
  const [name, setName] = useState(keyView.name);
  return (
    <Modal title="重命名密钥" onClose={onClose} width={400}>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          if (!name.trim()) return;
          try {
            await keyApi.rename(keyView.id, name.trim());
            onDone();
          } catch (err) {
            toast(errorMessage(err), "error");
          }
        }}
      >
        <input className="field__input" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        <div className="footerbar">
          <button type="button" className="btn btn--ghost" onClick={onClose}>
            取消
          </button>
          <button type="submit" className="btn btn--primary" disabled={!name.trim()}>
            保存
          </button>
        </div>
      </form>
    </Modal>
  );
}
