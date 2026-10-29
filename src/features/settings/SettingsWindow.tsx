import { useEffect, useState, type ReactNode } from "react";
import { AppearanceSection } from "@/features/settings/AppearanceSection";
import { TerminalSection } from "@/features/settings/TerminalSection";
import { KnownHostsSection } from "@/features/settings/KnownHostsSection";
import { SecuritySection } from "@/features/settings/SecuritySection";
import { ProvidersSection } from "@/features/settings/ProvidersSection";
import { SyncSection } from "@/features/settings/SyncSection";
import { ChangePasswordSection } from "@/features/settings/ChangePasswordSection";
import { usePrefs } from "@/app/prefs";
import { api } from "@/lib/api";
import { isMac, keyLabel } from "@/app/platform";
import { useUpdates } from "@/app/updates";
import {
  IconCloud,
  IconInfo,
  IconMonitor,
  IconSettings,
  IconShield,
  IconSparkle,
  IconTerminal,
  IconX,
} from "@/components/icons";

type Tab = "general" | "appearance" | "terminal" | "security" | "ai" | "sync" | "about";

const TABS: { id: Tab; label: string; icon: ReactNode }[] = [
  { id: "general", label: "通用", icon: <IconSettings size={15} /> },
  { id: "appearance", label: "外观", icon: <IconMonitor size={15} /> },
  { id: "terminal", label: "终端", icon: <IconTerminal size={15} /> },
  { id: "security", label: "安全", icon: <IconShield size={15} /> },
  { id: "ai", label: "AI", icon: <IconSparkle size={15} /> },
  { id: "sync", label: "同步", icon: <IconCloud size={15} /> },
  { id: "about", label: "关于", icon: <IconInfo size={15} /> },
];

/**
 * Settings, shown as a modal window rather than a page.
 *
 * Taking over the whole window for settings loses the user's place; a modal
 * says "you are still on the host list" and closes back into it.
 */
export function SettingsWindow({ onClose }: { onClose: () => void }) {
  const [tab, setTab] = useState<Tab>("general");

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div
        className="settings"
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-label="设置"
      >
        <div className="settings__head">
          <div className="settings__title">
            {TABS.find((t) => t.id === tab)?.label ?? "设置"}
          </div>
          <button className="iconbtn" onClick={onClose} aria-label="关闭">
            <IconX size={15} />
          </button>
        </div>

        <div className="settings__tabs">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={`settings__tab ${tab === t.id ? "settings__tab--active" : ""}`}
              onClick={() => setTab(t.id)}
            >
              <span className="settings__tab-icon">{t.icon}</span>
              {t.label}
            </button>
          ))}
        </div>

        <div className="settings__body">
          {tab === "general" && <GeneralSection />}
          {tab === "appearance" && <AppearanceSection />}
          {tab === "terminal" && <TerminalSection />}
          {tab === "security" && (
            <>
              <SecuritySection />
              <ChangePasswordSection />
              <KnownHostsSection />
            </>
          )}
          {tab === "ai" && <ProvidersSection />}
          {tab === "sync" && <SyncSection />}
          {tab === "about" && <AboutSection />}
        </div>
      </div>
    </div>
  );
}

function GeneralSection() {
  const autoLockMinutes = usePrefs((s) => s.autoLockMinutes);
  const setPref = usePrefs((s) => s.set);
  const setAutoLockMinutes = (m: number) => setPref("autoLockMinutes", m);

  const options = [
    { value: 0, label: "从不" },
    { value: 5, label: "5 分钟" },
    { value: 15, label: "15 分钟" },
    { value: 30, label: "30 分钟" },
    { value: 60, label: "1 小时" },
  ];

  return (
    <section className="settings__group">
      <div className="settings__group-title">空闲自动锁定</div>
      <div className="settings__row">
        <div className="settings__row-main">
          <div className="settings__label">锁定时间</div>
          <div className="settings__hint">
            超过这段时间没有操作，保险库会自动锁定并断开所有会话。
          </div>
        </div>
        <div className="settings__choices">
          {options.map((o) => (
            <button
              key={o.value}
              className={`chip ${autoLockMinutes === o.value ? "chip--on" : ""}`}
              onClick={() => setAutoLockMinutes(o.value)}
            >
              {o.label}
            </button>
          ))}
        </div>
      </div>
      <div className="settings__group-title" style={{ marginTop: 18 }}>快捷键</div>
      {SHORTCUTS.map(([keys, label]) => (
        <div key={keys} className="settings__row settings__row--tight">
          <div className="settings__row-main">
            <div className="settings__label">{label}</div>
          </div>
          <kbd className="kbd">{keys}</kbd>
        </div>
      ))}
    </section>
  );
}

const SHORTCUTS: [string, string][] = [
  [keyLabel("T"), "新建连接（含本地终端）"],
  [keyLabel("K"), "快速连接；终端内为清屏"],
  [keyLabel("W"), "关闭当前标签"],
  [`${keyLabel("1")} – 9`, "切换到第 N 个标签"],
  [isMac ? "⌘⇧[  /  ⌘⇧]" : "Ctrl+Shift+[  /  ]", "上一个 / 下一个标签"],
  [keyLabel("F"), "在终端中查找"],
  [`${keyLabel("C")} / ${keyLabel("V")}`, "终端内复制 / 粘贴"],
  [keyLabel(","), "打开设置"],
  [keyLabel("L"), "锁定"],
];

function AboutSection() {
  const [version, setVersion] = useState("");
  const phase = useUpdates((s) => s.phase);
  const info = useUpdates((s) => s.info);
  const message = useUpdates((s) => s.message);
  const progress = useUpdates((s) => s.progress);
  const check = useUpdates((s) => s.check);
  const install = useUpdates((s) => s.install);

  useEffect(() => {
    void api.appInfo().then((i) => setVersion(i.version)).catch(() => {});
  }, []);

  const status = (() => {
    switch (phase) {
      case "checking":
        return "正在检查…";
      case "current":
        return "已是最新版本";
      case "available":
        return `发现新版本 ${info?.version ?? ""}`;
      case "downloading":
        return progress?.total
          ? `正在下载 ${Math.round((progress.downloaded / progress.total) * 100)}%`
          : "正在下载…";
      case "ready":
        return "更新已安装，重启后生效";
      case "error":
        return message ?? "检查失败";
      default:
        return "";
    }
  })();

  const busy = phase === "checking" || phase === "downloading";

  return (
    <section className="settings__group">
      <div className="settings__group-title">关于 loki</div>

      <div className="settings__row">
        <div className="settings__row-main">
          <div className="settings__label">版本</div>
          <div className="settings__hint">
            {version || "…"} · 基于 Hatoba 架构蓝图重实现
            {status && <span className="settings__hint"> · {status}</span>}
          </div>
        </div>
        <div style={{ display: "flex", gap: 6 }}>
          <button className="btn btn--sm" disabled={busy} onClick={() => void check(false)}>
            {phase === "checking" ? "检查中…" : "检查更新"}
          </button>
          {phase === "available" && (
            <button className="btn btn--primary btn--sm" onClick={() => void install()}>
              下载并安装
            </button>
          )}
        </div>
      </div>

      <div className="settings__row">
        <div className="settings__row-main">
          <div className="settings__label">数据位置</div>
          <div className="settings__hint" style={{ fontFamily: "ui-monospace, Menlo, monospace" }}>
            {dataDir()}
          </div>
        </div>
      </div>

      <div className="settings__row">
        <div className="settings__row-main">
          <div className="settings__hint" style={{ lineHeight: 1.7 }}>
            所有主机凭据、私钥、API Key 在本机用 AES-256-GCM 加密后才落盘；
            后端进程之外任何地方都拿不到明文。同步只在你自己部署的 Cloudflare
            Worker 上存放密文。
          </div>
        </div>
      </div>
    </section>
  );
}

/** Where the vault lives, per platform (Tauri's `app_data_dir`). */
function dataDir(): string {
  return isMac
    ? "~/Library/Application Support/app.loki.desktop"
    : "%APPDATA%\\app.loki.desktop";
}
