import { create } from "zustand";
import { persist } from "zustand/middleware";

/** What the user picked; `system` follows the OS setting. */
export type ThemeChoice = "system" | "light" | "dark";
/** What is actually on screen. */
export type ResolvedTheme = "light" | "dark";

interface ThemeState {
  choice: ThemeChoice;
  resolved: ResolvedTheme;
  setChoice(choice: ThemeChoice): void;
  /** Re-reads the OS preference; called by the media query listener. */
  syncSystem(): void;
}

const query = () =>
  typeof window !== "undefined" && window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null;

function resolve(choice: ThemeChoice): ResolvedTheme {
  if (choice !== "system") return choice;
  return query()?.matches ? "dark" : "light";
}

/** Applies the theme to the document; CSS keys off `data-theme`. */
function apply(theme: ResolvedTheme) {
  document.documentElement.dataset.theme = theme;
}

export const useTheme = create<ThemeState>()(
  persist(
    (set, get) => ({
      choice: "system",
      resolved: resolve("system"),

      setChoice(choice) {
        const resolved = resolve(choice);
        apply(resolved);
        set({ choice, resolved });
      },

      syncSystem() {
        // Only meaningful while following the OS.
        if (get().choice !== "system") return;
        const resolved = resolve("system");
        apply(resolved);
        set({ resolved });
      },
    }),
    {
      name: "loki-theme",
      // `resolved` is derived; persisting it would fight the OS on next launch.
      partialize: (s) => ({ choice: s.choice }),
      onRehydrateStorage: () => (state) => {
        state?.setChoice(state.choice);
      },
    },
  ),
);

/** Call once at startup: applies the stored theme and starts following the OS. */
export function initTheme() {
  const { choice, setChoice, syncSystem } = useTheme.getState();
  setChoice(choice);

  const mq = query();
  if (!mq) return;
  const onChange = () => syncSystem();
  // `addEventListener` is the modern form; older WebViews only had addListener.
  if (mq.addEventListener) mq.addEventListener("change", onChange);
  else if (mq.addListener) mq.addListener(onChange);
}
