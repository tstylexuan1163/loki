import { useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import logoUrl from "@/assets/logo.png";
import { useApp } from "@/app/store";
import { Gate } from "@/features/gate/Gate";
import { HostEditor } from "@/features/hosts/HostEditor";
import { HostsPage } from "@/features/hosts/HostsPage";
import { ForwardsPage } from "@/features/forwards/ForwardsPage";
import { KeysPage } from "@/features/keys/KeysPage";
import { SettingsWindow } from "@/features/settings/SettingsWindow";
import { TerminalView } from "@/features/terminal/TerminalView";
import { QuickConnect } from "@/features/terminal/QuickConnect";
import { useTerminals, type TermTab } from "@/features/terminal/terminals";
import { useAutoLock } from "@/app/useAutoLock";
import { WindowControls } from "@/app/WindowControls";
import { UpdateBanner } from "@/components/UpdateBanner";
import { useUpdates } from "@/app/updates";
import { isAppShortcut, isMac, isWindows, keyLabel, shortcutKey } from "@/app/platform";
import { ConfirmHost, Toasts, toast } from "@/components/ui";
import {
  IconForward,
  IconGlobe,
  IconKey,
  IconLock,
  IconPlus,
  IconSearch,
  IconServer,
  IconSettings,
  IconTerminal,
  IconUsb,
} from "@/components/icons";

export function App() {
  // Per-field selectors: a whole-store subscription does not re-render on
  // `set` in zustand v5.
  const phase = useApp((s) => s.phase);
  const page = useApp((s) => s.page);
  const error = useApp((s) => s.error);
  const boot = useApp((s) => s.boot);
  const lock = useApp((s) => s.lock);
  const clearError = useApp((s) => s.clearError);
  const recoveryCode = useApp((s) => s.recoveryCode);

  const [settingsOpen, setSettingsOpen] = useState(false);
  const [quickOpen, setQuickOpen] = useState(false);

  useAutoLock();

  useEffect(() => {
    void boot();
  }, [boot]);

  // Update progress arrives as an event; the engine owns the download.
  useEffect(() => {
    let disposed = false;
    let un: (() => void) | undefined;
    void listen<{ downloaded: number; total: number | null }>("update-progress", (e) => {
      useUpdates.getState().setProgress(e.payload);
    }).then((f) => {
      if (disposed) f();
      else un = f;
    });
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  // One background check per launch, a few seconds in so it never competes with
  // startup work. A failure stays silent; the user can retry from 关于.
  useEffect(() => {
    if (phase !== "unlocked") return;
    const timer = window.setTimeout(() => void useUpdates.getState().check(true), 4000);
    return () => window.clearTimeout(timer);
  }, [phase]);

  // Store-level errors surface as toasts instead of a banner that pushes the
  // whole page down.
  useEffect(() => {
    if (error) {
      toast(error, "error");
      clearError();
    }
  }, [error, clearError]);

  const unlocked = phase === "unlocked" && recoveryCode === null;

  // Window-wide shortcuts. Terminal-only ones live in TerminalView.
  useEffect(() => {
    if (!unlocked) return;
    const onKey = (e: KeyboardEvent) => {
      if (!isAppShortcut(e)) return;
      const key = shortcutKey(e);
      const terms = useTerminals.getState();
      const go = (kind: "terminal") => useApp.setState({ page: { kind } });
      // On macOS ⇧ distinguishes tab cycling from other ⌘ chords; on other
      // platforms ⇧ is already part of every app shortcut.
      const shifted = !isMac || e.shiftKey;
      if (key === "k" || key === "t") {
        e.preventDefault();
        setQuickOpen(true);
      } else if (key === ",") {
        e.preventDefault();
        setSettingsOpen(true);
      } else if (key === "l" && (!isMac || !e.shiftKey)) {
        e.preventDefault();
        void lock();
      } else if (/^[1-9]$/.test(key) && terms.tabs.length) {
        e.preventDefault();
        const idx = Math.min(Number(key) - 1, terms.tabs.length - 1);
        terms.activate(terms.tabs[idx].id);
        go("terminal");
      } else if (shifted && (key === "[" || key === "]")) {
        e.preventDefault();
        terms.cycle(key === "[" ? -1 : 1);
        go("terminal");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [unlocked, lock]);

  if (phase === "boot") return null;

  // A freshly created vault holds its recovery code until the user acknowledges
  // it; the main UI must not appear before that, or the code is lost.
  if (!unlocked) {
    return (
      <>
        <TitleBar phase={phase} />
        <div className="gate-wrap">
          <Gate />
        </div>
        <Toasts />
      </>
    );
  }

  const editing = page.kind === "host-edit";

  return (
    <>
      <TitleBar phase={phase} onSearch={() => setQuickOpen(true)} />

      <div className="shell">
        <Sidebar onSettings={() => setSettingsOpen(true)} onLock={() => void lock()} onNew={() => setQuickOpen(true)} />

        <main className="main">
          {page.kind === "keys" ? (
            <KeysPage />
          ) : page.kind === "forwards" ? (
            <ForwardsPage />
          ) : editing ? (
            <HostEditor
              key={page.hostId ?? `new-${page.prefill?.address ?? ""}-${page.prefill?.protocol ?? ""}`}
              hostId={page.hostId}
              prefill={page.prefill}
            />
          ) : page.kind === "hosts" ? (
            <HostsPage />
          ) : null}
          {/* Always mounted so leaving the page never tears down a terminal. */}
          <TerminalView visible={page.kind === "terminal"} />
        </main>
      </div>

      <UpdateBanner />

      {settingsOpen && <SettingsWindow onClose={() => setSettingsOpen(false)} />}
      {quickOpen && <QuickConnect onClose={() => setQuickOpen(false)} />}
      <ConfirmHost />
      <Toasts />
    </>
  );
}

const TAB_ICON = {
  ssh: <IconServer size={14} />,
  telnet: <IconGlobe size={14} />,
  serial: <IconUsb size={14} />,
  local: <IconTerminal size={14} />,
};

function Sidebar({
  onSettings,
  onLock,
  onNew,
}: {
  onSettings: () => void;
  onLock: () => void;
  onNew: () => void;
}) {
  const page = useApp((s) => s.page);
  const hostsCount = useApp((s) => s.hosts.length);
  const tabs = useTerminals((s) => s.tabs);
  const active = useTerminals((s) => s.active);
  const activate = useTerminals((s) => s.activate);
  const closeTab = useTerminals((s) => s.closeTab);
  const go = (p: typeof page) => useApp.setState({ page: p });

  const isHosts = page.kind === "hosts" || page.kind === "host-edit";

  return (
    <nav className="sidebar">
      <div className="sidebar__section">
        <NavItem
          icon={<IconServer size={16} />}
          label="主机"
          count={hostsCount || undefined}
          active={isHosts}
          onClick={() => go({ kind: "hosts" })}
        />
        <NavItem
          icon={<IconForward size={16} />}
          label="端口转发"
          active={page.kind === "forwards"}
          onClick={() => go({ kind: "forwards" })}
        />
        <NavItem
          icon={<IconKey size={16} />}
          label="密钥库"
          active={page.kind === "keys"}
          onClick={() => go({ kind: "keys" })}
        />
      </div>

      <div className="sidebar__head">
        <span>会话</span>
        <button className="iconbtn iconbtn--xs" onClick={onNew} title={`新建连接 (${keyLabel("T")})`}>
          <IconPlus size={13} />
        </button>
      </div>
      <div className="sidebar__sessions">
        {tabs.length === 0 && <div className="sidebar__hint">没有打开的会话</div>}
        {tabs.map((t: TermTab) => (
          <button
            key={t.id}
            className={`session ${page.kind === "terminal" && t.id === active ? "session--active" : ""}`}
            onClick={() => {
              activate(t.id);
              go({ kind: "terminal" });
            }}
            title={t.title}
          >
            <span className="session__icon">{TAB_ICON[t.kind]}</span>
            <span className="session__title">{t.title}</span>
            <span className={`session__dot session__dot--${t.status}`} />
            <span
              className="session__close"
              role="button"
              onClick={(e) => {
                e.stopPropagation();
                void closeTab(t.id);
              }}
            >
              ×
            </span>
          </button>
        ))}
      </div>

      <div className="sidebar__spacer" />

      <div className="sidebar__section sidebar__section--bottom">
        <NavItem icon={<IconSettings size={16} />} label="设置" hint={keyLabel(",")} active={false} onClick={onSettings} />
        <NavItem icon={<IconLock size={16} />} label="锁定" hint={keyLabel("L")} active={false} onClick={onLock} />
      </div>
    </nav>
  );
}

function NavItem({
  icon,
  label,
  active,
  count,
  hint,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  active: boolean;
  count?: number;
  hint?: string;
  onClick: () => void;
}) {
  return (
    <button className={`nav ${active ? "nav--active" : ""}`} onClick={onClick}>
      <span className="nav__icon">{icon}</span>
      <span className="nav__label">{label}</span>
      {count != null && <span className="nav__count">{count}</span>}
      {hint && <span className="nav__hint">{hint}</span>}
    </button>
  );
}

function TitleBar({ phase, onSearch }: { phase: string; onSearch?: () => void }) {
  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar__brand" data-tauri-drag-region>
        <img className="titlebar__mark" src={logoUrl} alt="" />
        <span className="titlebar__name">loki</span>
      </div>
      {onSearch ? (
        <button className="titlebar__search" onClick={onSearch}>
          <IconSearch size={13} />
          <span>搜索主机或输入 user@host</span>
          <kbd>{keyLabel("K")}</kbd>
        </button>
      ) : (
        <div className="spacer" data-tauri-drag-region />
      )}
      <span className={`titlebar__status titlebar__status--${phase}`} data-tauri-drag-region>
        {phase === "onboarding" ? "初始化" : phase === "locked" ? "已锁定" : "已解锁"}
      </span>
      {/* Only Windows runs frameless today; macOS keeps its traffic lights and
          Linux still has a native frame, so neither draws these. */}
      {isWindows && <WindowControls />}
    </div>
  );
}
