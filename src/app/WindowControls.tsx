import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Minimise / maximise / close buttons for the frameless Windows window.
 *
 * macOS keeps its native traffic lights (the window is configured with
 * `titleBarStyle: Overlay`), so this component is only mounted on other
 * platforms. Otherwise the OS title bar and the app's own bar would both
 * appear, which is the double-bar bug this fixes.
 */
export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      try {
        const state = await win.isMaximized();
        if (!disposed) setMaximized(state);
      } catch {
        /* not available outside Tauri */
      }
      try {
        const un = await win.onResized(async () => {
          try {
            const state = await win.isMaximized();
            if (!disposed) setMaximized(state);
          } catch {
            /* ignore */
          }
        });
        if (disposed) un();
        else unlisten = un;
      } catch {
        /* ignore */
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const win = getCurrentWindow();
  const act = (fn: () => Promise<unknown>) => (e: React.MouseEvent) => {
    // Keep the click from reaching the drag region underneath.
    e.stopPropagation();
    void fn().catch(() => {});
  };

  return (
    <div className="wctl">
      <button className="wctl__btn" onClick={act(() => win.minimize())} aria-label="最小化" title="最小化">
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M0 5h10" stroke="currentColor" strokeWidth="1" />
        </svg>
      </button>
      <button
        className="wctl__btn"
        onClick={act(() => win.toggleMaximize())}
        aria-label={maximized ? "还原" : "最大化"}
        title={maximized ? "还原" : "最大化"}
      >
        {maximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <path d="M2.5 2.5V0.5h7v7h-2" fill="none" stroke="currentColor" strokeWidth="1" />
            <rect x="0.5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" strokeWidth="1" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1" />
          </svg>
        )}
      </button>
      <button
        className="wctl__btn wctl__btn--close"
        onClick={act(() => win.close())}
        aria-label="关闭"
        title="关闭"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M0 0l10 10M10 0L0 10" stroke="currentColor" strokeWidth="1" />
        </svg>
      </button>
    </div>
  );
}
