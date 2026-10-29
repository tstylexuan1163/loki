import { useEffect, useState } from "react";
import { usePrefs } from "@/app/prefs";
import { IconClock, IconCpu, IconDisk, IconMemory, IconShield } from "@/components/icons";
import { formatBytes, sessionApi, type ServerStats } from "@/lib/api";
import type { TermTab } from "@/features/terminal/terminals";

function uptime(secs: number | null): string {
  if (secs == null) return "—";
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}天${h}时`;
  if (h > 0) return `${h}时${m}分`;
  return `${m}分`;
}

function Meter({ value }: { value: number | null }) {
  const v = Math.max(0, Math.min(100, value ?? 0));
  const tone = v >= 90 ? "danger" : v >= 70 ? "warn" : "ok";
  return (
    <span className="meter">
      <span className={`meter__fill meter__fill--${tone}`} style={{ width: `${v}%` }} />
    </span>
  );
}

/**
 * The strip under the terminal: what this tab is connected to, and — for SSH —
 * a light resource reading refreshed every few seconds while visible.
 */
export function StatusBar({ tab }: { tab: TermTab }) {
  const showStats = usePrefs((s) => s.showStats);
  const [stats, setStats] = useState<ServerStats | null>(null);
  const [unsupported, setUnsupported] = useState(false);

  const live = tab.kind === "ssh" && tab.status === "connected" && !!tab.sessionId;

  useEffect(() => {
    setStats(null);
    setUnsupported(false);
    if (!live || !showStats || !tab.sessionId) return;
    const sid = tab.sessionId;
    let stopped = false;
    let timer: number | undefined;

    const tick = async () => {
      try {
        const s = await sessionApi.stats(sid);
        if (stopped) return;
        setStats(s);
        // A system the script does not know yields an empty reading; stop
        // polling rather than opening a channel every few seconds for nothing.
        if (s.cpuPercent == null && s.memTotal == null) {
          setUnsupported(true);
          return;
        }
      } catch {
        if (stopped) return;
      }
      timer = window.setTimeout(tick, 5000);
    };
    void tick();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [live, showStats, tab.sessionId]);

  const memPct =
    stats?.memTotal && stats.memUsed != null ? (stats.memUsed / stats.memTotal) * 100 : null;
  const diskPct =
    stats?.diskTotal && stats.diskUsed != null ? (stats.diskUsed / stats.diskTotal) * 100 : null;

  const label =
    tab.kind === "local"
      ? "本地 shell"
      : tab.kind === "telnet"
        ? "Telnet · 明文"
        : tab.kind === "serial"
          ? "串口"
          : tab.target
            ? "快速连接"
            : "SSH";

  return (
    <div className="statusbar">
      <span className={`statusbar__state statusbar__state--${tab.status}`}>
        <i />
        {tab.status === "connected"
          ? "已连接"
          : tab.status === "connecting"
            ? "连接中"
            : tab.status === "closed"
              ? "已断开"
              : "失败"}
      </span>
      <span className="statusbar__item">
        {tab.kind === "ssh" && <IconShield size={12} />}
        {label}
      </span>

      <span className="spacer" />

      {live && showStats && stats && !unsupported && (
        <>
          <span className="statusbar__item" title={`CPU（${stats.cpus ?? "?"} 核）`}>
            <IconCpu size={12} />
            {stats.cpuPercent != null ? `${Math.round(stats.cpuPercent)}%` : "—"}
            <Meter value={stats.cpuPercent} />
          </span>
          <span
            className="statusbar__item"
            title={`内存 ${formatBytes(stats.memUsed)} / ${formatBytes(stats.memTotal)}`}
          >
            <IconMemory size={12} />
            {formatBytes(stats.memUsed)}
            <Meter value={memPct} />
          </span>
          <span
            className="statusbar__item"
            title={`根分区 ${formatBytes(stats.diskUsed)} / ${formatBytes(stats.diskTotal)}`}
          >
            <IconDisk size={12} />
            {diskPct != null ? `${Math.round(diskPct)}%` : "—"}
            <Meter value={diskPct} />
          </span>
          {stats.netRxRate != null && (
            <span className="statusbar__item" title="网络（接收 / 发送）">
              ↓{formatBytes(stats.netRxRate)}/s ↑{formatBytes(stats.netTxRate)}/s
            </span>
          )}
          {stats.load && (
            <span className="statusbar__item" title="负载（1 / 5 / 15 分钟）">
              {stats.load.map((l) => l.toFixed(2)).join(" ")}
            </span>
          )}
          <span className="statusbar__item" title="运行时间">
            <IconClock size={12} />
            {uptime(stats.uptimeSecs)}
          </span>
        </>
      )}
    </div>
  );
}
