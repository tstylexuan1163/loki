import { formatBytes, useUpdates } from "@/app/updates";

/**
 * Bottom-right prompt shown when an update is available.
 *
 * Deliberately not a modal: the user may be mid-session in a terminal, and being
 * forced to deal with an update is hostile. "稍后" hides it for this version.
 */
export function UpdateBanner() {
  const phase = useUpdates((s) => s.phase);
  const info = useUpdates((s) => s.info);
  const progress = useUpdates((s) => s.progress);
  const message = useUpdates((s) => s.message);
  const dismissed = useUpdates((s) => s.dismissed);
  const install = useUpdates((s) => s.install);
  const dismiss = useUpdates((s) => s.dismiss);

  if (dismissed) return null;
  if (phase !== "available" && phase !== "downloading" && phase !== "ready" && phase !== "error") {
    return null;
  }

  const pct =
    progress && progress.total && progress.total > 0
      ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100))
      : null;

  return (
    <div className="updatebanner" role="status">
      <div className="updatebanner__head">
        <span className="updatebanner__title">
          {phase === "ready" ? "更新已安装" : `发现新版本 ${info?.version ?? ""}`}
        </span>
        <button className="iconbtn" onClick={dismiss} aria-label="关闭">
          ×
        </button>
      </div>

      {phase === "available" && (
        <>
          {info?.notes && (
            <div className="updatebanner__notes">{info.notes.slice(0, 280)}</div>
          )}
          <div className="updatebanner__actions">
            <button className="btn btn--ghost btn--sm" onClick={dismiss}>
              稍后
            </button>
            <button className="btn btn--primary btn--sm" onClick={() => void install()}>
              下载并安装
            </button>
          </div>
        </>
      )}

      {phase === "downloading" && (
        <>
          <div className="updatebanner__bar">
            <div className="updatebanner__fill" style={{ width: pct === null ? "35%" : `${pct}%` }} />
          </div>
          <div className="updatebanner__hint">
            {pct === null
              ? `正在下载 ${progress ? formatBytes(progress.downloaded) : ""}…`
              : `正在下载 ${pct}%（${formatBytes(progress!.downloaded)}${
                  progress?.total ? ` / ${formatBytes(progress.total)}` : ""
                }）`}
          </div>
        </>
      )}

      {phase === "ready" && (
        <>
          <div className="updatebanner__notes">
            新版本已经就位。macOS 会自动重启；Windows 上安装程序会接手，若没有请手动重新打开。
          </div>
          <div className="updatebanner__actions">
            <button className="btn btn--primary btn--sm" onClick={dismiss}>
              知道了
            </button>
          </div>
        </>
      )}

      {phase === "error" && (
        <>
          <div className="updatebanner__notes updatebanner__notes--err">{message}</div>
          <div className="updatebanner__actions">
            <button className="btn btn--ghost btn--sm" onClick={dismiss}>
              关闭
            </button>
            <button className="btn btn--sm" onClick={() => void install()}>
              重试
            </button>
          </div>
        </>
      )}
    </div>
  );
}
