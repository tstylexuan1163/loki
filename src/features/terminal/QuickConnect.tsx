import { useMemo, useState } from "react";
import { useApp } from "@/app/store";
import { usePrefs } from "@/app/prefs";
import { Modal } from "@/components/ui";
import { IconClock, IconServer, IconTerminal } from "@/components/icons";
import { parseTarget, type HostView } from "@/lib/api";
import { useTerminals } from "@/features/terminal/terminals";

/**
 * Quick connect: type `user@host:port` or pick a saved host, like a command
 * palette. Unsaved targets authenticate with the agent and then a password
 * prompt; nothing is stored except the target string in the recent list.
 */
export function QuickConnect({ onClose }: { onClose: () => void }) {
  const hosts = useApp((s) => s.hosts);
  const recent = usePrefs((s) => s.recentTargets);
  const addRecent = usePrefs((s) => s.addRecent);
  const openHost = useTerminals((s) => s.openHost);
  const openTarget = useTerminals((s) => s.openTarget);
  const openLocal = useTerminals((s) => s.openLocal);
  const [text, setText] = useState("");
  const [cursor, setCursor] = useState(0);

  const q = text.trim().toLowerCase();
  const matches = useMemo(
    () =>
      hosts
        .filter(
          (h) =>
            !q ||
            [h.name, h.address, h.username, ...h.tags].some((f) => f.toLowerCase().includes(q)),
        )
        .slice(0, 8),
    [hosts, q],
  );
  const target = parseTarget(text, "root");
  const recents = recent.filter((r) => !q || r.toLowerCase().includes(q)).slice(0, 5);

  type Row =
    | { kind: "target"; label: string; run: () => void }
    | { kind: "host"; host: HostView; run: () => void }
    | { kind: "recent"; label: string; run: () => void }
    | { kind: "local"; run: () => void };

  const go = (fn: () => void) => {
    fn();
    useApp.setState({ page: { kind: "terminal" } });
    onClose();
  };

  const rows: Row[] = [];
  if (target && text.includes("@")) {
    rows.push({
      kind: "target",
      label: `${target.username}@${target.address}${target.port !== 22 ? `:${target.port}` : ""}`,
      run: () =>
        go(() => {
          addRecent(text.trim());
          openTarget(target);
        }),
    });
  }
  for (const h of matches) rows.push({ kind: "host", host: h, run: () => go(() => openHost(h)) });
  for (const r of recents) {
    const t = parseTarget(r, "root");
    if (t) rows.push({ kind: "recent", label: r, run: () => go(() => openTarget(t)) });
  }
  if (!q || "local 本地".includes(q)) rows.push({ kind: "local", run: () => go(() => openLocal()) });
  const sel = Math.min(cursor, Math.max(rows.length - 1, 0));

  return (
    <Modal onClose={onClose} width={560} className="palette">
      <input
        className="palette__input"
        autoFocus
        placeholder="user@host:port，或搜索已保存的主机"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setCursor(0);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setCursor((c) => Math.min(c + 1, rows.length - 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setCursor((c) => Math.max(c - 1, 0));
          } else if (e.key === "Enter") {
            e.preventDefault();
            if (rows[sel]) rows[sel].run();
            else if (target) {
              go(() => {
                addRecent(text.trim());
                openTarget(target);
              });
            }
          }
        }}
      />
      <div className="palette__list">
        {rows.length === 0 && (
          <div className="palette__empty">
            {text ? "没有匹配的主机。输入 user@host 直接连接。" : "没有保存的主机。"}
          </div>
        )}
        {rows.map((r, i) => (
          <button
            key={i}
            className={`palette__row ${i === sel ? "palette__row--on" : ""}`}
            onMouseEnter={() => setCursor(i)}
            onClick={r.run}
          >
            <span className="palette__icon">
              {r.kind === "host" ? (
                <IconServer size={15} />
              ) : r.kind === "recent" ? (
                <IconClock size={15} />
              ) : (
                <IconTerminal size={15} />
              )}
            </span>
            {r.kind === "host" ? (
              <>
                <span className="palette__main">{r.host.name || r.host.address}</span>
                <span className="palette__sub">
                  {r.host.protocol === "serial"
                    ? r.host.address
                    : `${r.host.username}@${r.host.address}:${r.host.port}`}
                </span>
              </>
            ) : r.kind === "local" ? (
              <>
                <span className="palette__main">本地终端</span>
                <span className="palette__sub">在这台电脑上打开 shell</span>
              </>
            ) : (
              <>
                <span className="palette__main">{r.kind === "target" ? `连接 ${r.label}` : r.label}</span>
                <span className="palette__sub">{r.kind === "target" ? "快速连接，不保存" : "最近连接"}</span>
              </>
            )}
            {i === sel && <kbd className="palette__kbd">↵</kbd>}
          </button>
        ))}
      </div>
    </Modal>
  );
}
