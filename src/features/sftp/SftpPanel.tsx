import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  errorMessage,
  formatBytes,
  sftpApi,
  type DirListing,
  type FileEntry,
} from "@/lib/api";
import { Menu, Modal, confirmDialog, toast, useMenu } from "@/components/ui";
import {
  IconArrowUp,
  IconDownload,
  IconEdit,
  IconFile,
  IconFilePlus,
  IconFolder,
  IconFolderPlus,
  IconHome,
  IconLink,
  IconRefresh,
  IconTrash,
  IconUpload,
  IconX,
  IconCopy,
  IconEye,
  IconEyeOff,
} from "@/components/icons";
import { startTransferListener, useTransfers } from "@/features/sftp/transfers";
import { isMac } from "@/app/platform";

/**
 * The file panel that sits beside a terminal.
 *
 * It talks to the same SSH session as the terminal, so opening a directory here
 * costs no extra login. Transfers run on their own SFTP channel in the backend
 * and report progress as events, so a large copy never freezes the listing.
 */
export function SftpPanel({ sessionId }: { sessionId: string }) {
  const [listing, setListing] = useState<DirListing | null>(null);
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState("");
  const [showHidden, setShowHidden] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<{ path: string; content: string } | null>(null);
  const [naming, setNaming] = useState<
    { kind: "mkdir" | "touch" } | { kind: "rename"; entry: FileEntry } | null
  >(null);
  const [dropping, setDropping] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const crumbsRef = useRef<HTMLDivElement>(null);
  const pathRef = useRef(path);
  pathRef.current = path;
  const rowMenu = useMenu<FileEntry>();
  const transfers = useTransfers((s) => s.items).filter((t) => t.sessionId === sessionId);
  const dismiss = useTransfers((s) => s.dismiss);

  const load = useCallback(
    async (target: string) => {
      setBusy(true);
      try {
        const l = await sftpApi.list(sessionId, target);
        setListing(l);
        setPath(l.path);
        setError(null);
        setSelected(null);
      } catch (e) {
        setError(errorMessage(e));
      } finally {
        setBusy(false);
      }
    },
    [sessionId],
  );

  useEffect(() => {
    startTransferListener();
    void load("");
  }, [load]);

  // An upload landing in the open directory should show up without a manual
  // refresh.
  const doneCount = transfers.filter((t) => t.state === "done" && t.direction === "upload").length;
  useEffect(() => {
    if (doneCount > 0) void load(pathRef.current);
  }, [doneCount, load]);

  // Files dragged from Finder onto the panel upload into the current directory.
  useEffect(() => {
    let un: (() => void) | undefined;
    let disposed = false;
    const inside = (pos: { x: number; y: number }) => {
      const r = rootRef.current?.getBoundingClientRect();
      if (!r) return false;
      const x = pos.x / window.devicePixelRatio;
      const y = pos.y / window.devicePixelRatio;
      return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
    };
    void getCurrentWebview()
      .onDragDropEvent((evt) => {
        const p = evt.payload;
        if (p.type === "over") setDropping(inside(p.position));
        else if (p.type === "leave") setDropping(false);
        else if (p.type === "drop") {
          setDropping(false);
          if (inside(p.position) && p.paths.length) void upload(p.paths);
        }
      })
      .then((f) => {
        if (disposed) f();
        else un = f;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      un?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  async function upload(paths: string[]) {
    if (!pathRef.current) return;
    try {
      await sftpApi.upload(sessionId, paths, pathRef.current);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  async function pickUpload(directory: boolean) {
    const picked = await openDialog({ multiple: true, directory, title: "选择要上传的" + (directory ? "文件夹" : "文件") });
    if (!picked) return;
    void upload(Array.isArray(picked) ? picked : [picked]);
  }

  async function download(entry: FileEntry, ask: boolean) {
    try {
      if (ask) {
        const dest = await saveDialog({ defaultPath: entry.name, title: "保存到" });
        if (!dest) return;
        await sftpApi.download(sessionId, entry.path, dest);
      } else {
        await sftpApi.downloadToDownloads(sessionId, entry.path);
        toast(`正在下载 ${entry.name} 到「下载」文件夹`);
      }
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  function up() {
    if (!path || path === "/") return;
    const trimmed = path.replace(/\/+$/, "");
    const idx = trimmed.lastIndexOf("/");
    void load(idx <= 0 ? "/" : trimmed.slice(0, idx));
  }

  async function openEntry(entry: FileEntry) {
    if (entry.kind === "dir" || entry.kind === "link") {
      // A link is most often to a directory; if it is a file, listing fails
      // and we fall back to opening it.
      if (entry.kind === "link") {
        try {
          const l = await sftpApi.list(sessionId, entry.path);
          setListing(l);
          setPath(l.path);
          return;
        } catch {
          /* not a directory */
        }
      } else {
        void load(entry.path);
        return;
      }
    }
    try {
      const content = await sftpApi.read(sessionId, entry.path);
      setEditing({ path: entry.path, content });
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  async function guard(fn: () => Promise<void>, ok?: string) {
    setBusy(true);
    try {
      await fn();
      await load(path);
      if (ok) toast(ok, "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  }

  async function remove(entry: FileEntry) {
    const ok = await confirmDialog({
      title: `删除 ${entry.name}？`,
      body:
        entry.kind === "dir"
          ? "这个文件夹及其中所有内容会从服务器上永久删除，无法撤销。"
          : "文件会从服务器上永久删除，无法撤销。",
      confirmLabel: "删除",
      danger: true,
    });
    if (ok) void guard(() => sftpApi.remove(sessionId, entry.path, entry.kind === "dir"), `已删除 ${entry.name}`);
  }

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return (listing?.entries ?? []).filter(
      (e) => (showHidden || !e.name.startsWith(".")) && (!q || e.name.toLowerCase().includes(q)),
    );
  }, [listing, filter, showHidden]);

  // Keep the deepest directory in view when the path is longer than the bar.
  useEffect(() => {
    const el = crumbsRef.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [path]);

  const crumbs = useMemo(() => {
    const parts = path.split("/").filter(Boolean);
    return [
      { label: "/", path: "/" },
      ...parts.map((p, i) => ({ label: p, path: "/" + parts.slice(0, i + 1).join("/") })),
    ];
  }, [path]);

  return (
    <div className={`sftp ${dropping ? "sftp--drop" : ""}`} ref={rootRef}>
      <div className="panel-head">
        <span className="panel-head__title">文件</span>
        <div className="spacer" />
        <button className="iconbtn" title="上传文件" onClick={() => void pickUpload(false)} disabled={!path}>
          <IconUpload size={15} />
        </button>
        <button className="iconbtn" title="新建文件夹" onClick={() => setNaming({ kind: "mkdir" })} disabled={!path}>
          <IconFolderPlus size={15} />
        </button>
        <button className="iconbtn" title="新建文件" onClick={() => setNaming({ kind: "touch" })} disabled={!path}>
          <IconFilePlus size={15} />
        </button>
        <button
          className={`iconbtn ${showHidden ? "iconbtn--on" : ""}`}
          title={showHidden ? "隐藏点文件" : "显示点文件"}
          onClick={() => setShowHidden(!showHidden)}
        >
          {showHidden ? <IconEye size={15} /> : <IconEyeOff size={15} />}
        </button>
      </div>

      <div className="sftp__nav">
        <button className="iconbtn" onClick={up} disabled={!path || path === "/"} title="上一级">
          <IconArrowUp size={14} />
        </button>
        <button className="iconbtn" onClick={() => void load(listing?.home ?? "/")} title="主目录">
          <IconHome size={14} />
        </button>
        <div className="crumbs" title={path} ref={crumbsRef}>
          {crumbs.map((c, i) => (
            <button key={c.path} className="crumbs__item" onClick={() => void load(c.path)}>
              {i > 1 && <span className="crumbs__sep">/</span>}
              {i === 0 ? <span className="crumbs__root">/</span> : c.label}
            </button>
          ))}
        </div>
        <button className="iconbtn" onClick={() => void load(path)} disabled={busy} title="刷新">
          <IconRefresh size={14} className={busy ? "spin" : ""} />
        </button>
      </div>

      <div className="sftp__filter">
        <input
          className="input input--sm"
          placeholder="筛选当前目录"
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </div>

      {error && (
        <div className="error error--inline" onClick={() => setError(null)}>
          {error}
        </div>
      )}

      <div className="sftp__list">
        {listing && visible.length === 0 && (
          <div className="sftp__empty">{filter ? "没有匹配的文件" : "空目录"}</div>
        )}
        {visible.map((e) => (
          <div
            key={e.path}
            className={`sftp__row ${selected === e.path ? "sftp__row--sel" : ""}`}
            onClick={() => setSelected(e.path)}
            onDoubleClick={() => void openEntry(e)}
            onContextMenu={(ev) => {
              setSelected(e.path);
              rowMenu.open(ev, e);
            }}
            title={e.path}
          >
            <span className={`sftp__icon sftp__icon--${e.kind}`}>
              {e.kind === "dir" ? <IconFolder size={15} /> : e.kind === "link" ? <IconLink size={14} /> : <IconFile size={14} />}
            </span>
            <span className="sftp__name">{e.name}</span>
            <span className="sftp__meta">
              {e.kind === "dir" ? "" : formatBytes(e.size)}
            </span>
            <span className="sftp__meta sftp__meta--date">
              {e.modified ? formatDate(e.modified) : ""}
            </span>
          </div>
        ))}
      </div>

      {transfers.length > 0 && (
        <div className="transfers">
          {transfers.map((t) => {
            const pct = t.total > 0 ? Math.min(100, (t.bytes / t.total) * 100) : t.state === "done" ? 100 : 0;
            return (
              <div key={t.id} className={`transfer transfer--${t.state}`}>
                <span className="transfer__icon">
                  {t.direction === "upload" ? <IconUpload size={13} /> : <IconDownload size={13} />}
                </span>
                <div className="transfer__main">
                  <div className="transfer__line">
                    <span className="transfer__name">{t.name}</span>
                    <span className="transfer__state">
                      {t.state === "running"
                        ? `${formatBytes(t.bytes)} / ${formatBytes(t.total)}`
                        : t.state === "done"
                          ? "完成"
                          : t.state === "cancelled"
                            ? "已取消"
                            : "失败"}
                    </span>
                  </div>
                  {t.state === "failed" ? (
                    <div className="transfer__err">{t.error}</div>
                  ) : (
                    <div className="progress">
                      <span className="progress__fill" style={{ width: `${pct}%` }} />
                    </div>
                  )}
                </div>
                <button
                  className="iconbtn"
                  title={t.state === "running" ? "取消" : "移除"}
                  onClick={() => (t.state === "running" ? void sftpApi.cancel(t.id) : dismiss(t.id))}
                >
                  <IconX size={13} />
                </button>
              </div>
            );
          })}
        </div>
      )}

      {dropping && (
        <div className="sftp__dropzone">
          <IconUpload size={22} />
          松开以上传到 <code>{path}</code>
        </div>
      )}

      {rowMenu.menu && (
        <Menu
          x={rowMenu.menu.x}
          y={rowMenu.menu.y}
          onClose={rowMenu.close}
          items={(() => {
            const e = rowMenu.menu!.data;
            return [
              {
                label: e.kind === "dir" ? "打开" : "在编辑器中打开",
                icon: e.kind === "dir" ? <IconFolder size={14} /> : <IconEdit size={14} />,
                onSelect: () => void openEntry(e),
              },
              {
                label: "下载到「下载」",
                icon: <IconDownload size={14} />,
                onSelect: () => void download(e, false),
              },
              {
                label: "下载到…",
                icon: <IconDownload size={14} />,
                onSelect: () => void download(e, true),
              },
              "sep" as const,
              {
                label: "重命名",
                icon: <IconEdit size={14} />,
                onSelect: () => setNaming({ kind: "rename", entry: e }),
              },
              {
                label: "复制路径",
                icon: <IconCopy size={14} />,
                onSelect: () => {
                  void navigator.clipboard.writeText(e.path);
                  toast("已复制路径");
                },
              },
              "sep" as const,
              {
                label: "删除",
                danger: true,
                icon: <IconTrash size={14} />,
                onSelect: () => void remove(e),
              },
            ];
          })()}
        />
      )}

      {naming && (
        <NameDialog
          title={
            naming.kind === "mkdir" ? "新建文件夹" : naming.kind === "touch" ? "新建文件" : "重命名"
          }
          initial={naming.kind === "rename" ? naming.entry.name : ""}
          onClose={() => setNaming(null)}
          onSubmit={(name) => {
            const target = joinPath(path, name);
            const current = naming;
            setNaming(null);
            if (current.kind === "rename") void guard(() => sftpApi.rename(sessionId, current.entry.path, target));
            else if (current.kind === "mkdir") void guard(() => sftpApi.mkdir(sessionId, target));
            else void guard(() => sftpApi.write(sessionId, target, ""));
          }}
        />
      )}

      {editing && (
        <FileEditor
          path={editing.path}
          initial={editing.content}
          onClose={() => setEditing(null)}
          onSave={async (content) => {
            await sftpApi.write(sessionId, editing.path, content);
            toast(`已保存 ${editing.path}`, "success");
            setEditing(null);
          }}
        />
      )}
    </div>
  );
}

function joinPath(dir: string, name: string) {
  return dir.endsWith("/") ? `${dir}${name}` : `${dir}/${name}`;
}

function formatDate(secs: number): string {
  const d = new Date(secs * 1000);
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  if (d.getFullYear() === now.getFullYear()) {
    return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function NameDialog({
  title,
  initial,
  onClose,
  onSubmit,
}: {
  title: string;
  initial: string;
  onClose: () => void;
  onSubmit: (name: string) => void;
}) {
  const [name, setName] = useState(initial);
  const valid = name.trim().length > 0 && !name.includes("/");
  return (
    <Modal title={title} onClose={onClose} width={400}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) onSubmit(name.trim());
        }}
      >
        <input
          className="field__input"
          autoFocus
          value={name}
          onFocus={(e) => {
            // Select the stem so typing replaces the name but keeps `.conf`.
            const dot = e.target.value.lastIndexOf(".");
            e.target.setSelectionRange(0, dot > 0 ? dot : e.target.value.length);
          }}
          onChange={(e) => setName(e.target.value)}
        />
        <div className="footerbar">
          <button type="button" className="btn btn--ghost" onClick={onClose}>
            取消
          </button>
          <button type="submit" className="btn btn--primary" disabled={!valid}>
            确定
          </button>
        </div>
      </form>
    </Modal>
  );
}

function FileEditor({
  path,
  initial,
  onClose,
  onSave,
}: {
  path: string;
  initial: string;
  onClose: () => void;
  onSave: (content: string) => Promise<void>;
}) {
  const [content, setContent] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dirty = content !== initial;
  const lines = content.split("\n").length;

  const save = async () => {
    if (!dirty || busy) return;
    setBusy(true);
    try {
      await onSave(content);
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  const close = async () => {
    if (
      dirty &&
      !(await confirmDialog({ title: "放弃未保存的修改？", confirmLabel: "放弃", danger: true }))
    )
      return;
    onClose();
  };

  return (
    <Modal
      title={<span className="mono">{path}</span>}
      subtitle={`${lines} 行${dirty ? " · 有未保存的修改" : ""}`}
      onClose={() => void close()}
      width="min(920px, 92vw)"
      className="modal--editor"
    >
      <textarea
        className="code-editor"
        value={content}
        onChange={(e) => setContent(e.target.value)}
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        autoFocus
        onKeyDown={(e) => {
          // Ctrl+S is free inside an editor, so it needs no Shift on Windows.
          if ((isMac ? e.metaKey : e.ctrlKey) && e.key.toLowerCase() === "s") {
            e.preventDefault();
            void save();
          }
          // Tab inserts two spaces instead of leaving the editor.
          if (e.key === "Tab" && !e.shiftKey) {
            e.preventDefault();
            const el = e.currentTarget;
            const { selectionStart: a, selectionEnd: b } = el;
            const next = content.slice(0, a) + "  " + content.slice(b);
            setContent(next);
            requestAnimationFrame(() => el.setSelectionRange(a + 2, a + 2));
          }
        }}
      />
      {error && <div className="error">{error}</div>}
      <div className="footerbar">
        <span className="muted" style={{ marginRight: "auto" }}>
          {isMac ? "⌘S" : "Ctrl+S"} 保存
        </span>
        <button className="btn btn--ghost" onClick={() => void close()}>
          取消
        </button>
        <button className="btn btn--primary" disabled={!dirty || busy} onClick={() => void save()}>
          {busy ? "保存中…" : "保存"}
        </button>
      </div>
    </Modal>
  );
}
