import { create } from "zustand";
import { errorMessage, updateApi, type UpdateInfo } from "@/lib/api";

/**
 * Update state, shared by the banner and the About section.
 *
 * The engine lives in Rust (signature verification, platform installer); this
 * only tracks what to show. `silent` checks power the startup check: a failure
 * there must not nag the user, so it goes back to `idle` instead of `error`.
 */
export type UpdatePhase =
  | "idle"
  | "checking"
  | "current"
  | "available"
  | "downloading"
  | "ready"
  | "error";

interface Progress {
  downloaded: number;
  total: number | null;
}

interface UpdatesState {
  phase: UpdatePhase;
  info: UpdateInfo | null;
  progress: Progress | null;
  message: string | null;
  /** Whether the user closed the banner for this version. */
  dismissed: boolean;

  check(silent?: boolean): Promise<void>;
  install(): Promise<void>;
  dismiss(): void;
  setProgress(p: Progress): void;
  markReady(): void;
}

export const useUpdates = create<UpdatesState>((set, get) => ({
  phase: "idle",
  info: null,
  progress: null,
  message: null,
  dismissed: false,

  async check(silent = false) {
    // Never stack checks, and never interrupt a download.
    const phase = get().phase;
    if (phase === "checking" || phase === "downloading") return;

    set({ phase: "checking", message: null });
    try {
      const info = await updateApi.check();
      if (info) {
        set({ phase: "available", info, dismissed: false });
      } else if (silent) {
        // Nothing to say on a background check.
        set({ phase: "idle" });
      } else {
        set({ phase: "current" });
      }
    } catch (e) {
      if (silent) set({ phase: "idle" });
      else set({ phase: "error", message: errorMessage(e) });
    }
  },

  async install() {
    if (get().phase === "downloading") return;
    set({ phase: "downloading", progress: { downloaded: 0, total: null }, message: null });
    try {
      await updateApi.install();
      // macOS restarts by itself; Windows hands off to the installer. Either
      // way, reaching here means the payload was verified and installed.
      set({ phase: "ready" });
    } catch (e) {
      set({ phase: "error", message: errorMessage(e) });
    }
  },

  dismiss() {
    set({ dismissed: true });
  },

  setProgress(p) {
    set({ progress: p });
  },

  markReady() {
    set({ phase: "ready" });
  },
}));

/** Human-readable size for the download progress line. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
