import { useEffect } from "react";
import { idleExpired, noteActivity, useApp, lockVaultNow } from "@/app/store";
import { usePrefs } from "@/app/prefs";

/** Any of these means the user is still there. */
const ACTIVITY_EVENTS = [
  "mousedown",
  "mousemove",
  "keydown",
  "wheel",
  "touchstart",
  "focus",
] as const;

/**
 * Locks the vault after `autoLockMinutes` of no input.
 *
 * Activity is tracked with a module-level timestamp rather than React state, so
 * moving the mouse does not re-render the whole app. The check runs on a timer
 * AND on visibility change: a laptop that was asleep must not come back to a
 * still-unlocked vault.
 */
export function useAutoLock() {
  const phase = useApp((s) => s.phase);
  const minutes = usePrefs((s) => s.autoLockMinutes);

  useEffect(() => {
    if (phase !== "unlocked" || minutes <= 0) return;

    const mark = () => noteActivity();
    for (const ev of ACTIVITY_EVENTS) {
      window.addEventListener(ev, mark, { passive: true });
    }

    const timer = window.setInterval(() => {
      if (idleExpired(minutes)) void lockVaultNow();
    }, 5_000);

    // Coming back from sleep or a hidden window: decide immediately rather than
    // waiting for the next tick.
    const onVisibility = () => {
      if (document.visibilityState === "visible") {
        if (idleExpired(minutes)) void lockVaultNow();
      }
    };
    document.addEventListener("visibilitychange", onVisibility);

    return () => {
      for (const ev of ACTIVITY_EVENTS) window.removeEventListener(ev, mark);
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [phase, minutes]);
}
