/**
 * Device-local preferences. Nothing secret lives here, so plain localStorage
 * via zustand's persist middleware is fine.
 */
import { create } from "zustand";
import { persist } from "zustand/middleware";

export type CursorStyle = "block" | "bar" | "underline";
export type HostsView = "cards" | "list";
/** Which tab the quick-connect palette opens with. */
export type QuickProtocol = "ssh" | "telnet" | "serial" | "local";

export interface Prefs {
  /** Minutes of inactivity before the vault locks itself; 0 disables it. */
  autoLockMinutes: number;
  /** Terminal colour scheme id, see `features/terminal/themes.ts`. */
  terminalScheme: string;
  fontSize: number;
  fontFamily: string;
  lineHeight: number;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
  scrollback: number;
  copyOnSelect: boolean;
  /** Poll CPU/memory/disk for the active SSH tab's status bar. */
  showStats: boolean;
  hostsView: HostsView;
  /** Category sections folded on the hosts page, by category id ("" = 未分类). */
  collapsedCategories: string[];
  /** Width of the side panel (files / assistant) in pixels. */
  panelWidth: number;
  /**
   * Recent quick-connect targets, newest first. Telnet entries are stored as
   * `telnet://host:port`; anything without a scheme prefix is SSH (0.10 wrote
   * bare `user@host:port` strings).
   */
  recentTargets: string[];
  /** Protocol the quick-connect palette last used. */
  quickProtocol: QuickProtocol;
}

export const FONT_CHOICES = [
  { value: 'ui-monospace, "SF Mono", Menlo, monospace', label: "SF Mono" },
  { value: 'Menlo, monospace', label: "Menlo" },
  { value: '"JetBrains Mono", ui-monospace, monospace', label: "JetBrains Mono" },
  { value: '"Fira Code", ui-monospace, monospace', label: "Fira Code" },
  { value: '"Cascadia Code", ui-monospace, monospace', label: "Cascadia Code" },
];

const DEFAULTS: Prefs = {
  autoLockMinutes: 15,
  terminalScheme: "loki",
  fontSize: 13,
  fontFamily: FONT_CHOICES[0].value,
  lineHeight: 1.25,
  cursorStyle: "block",
  cursorBlink: true,
  scrollback: 10_000,
  copyOnSelect: false,
  showStats: true,
  hostsView: "cards",
  collapsedCategories: [],
  panelWidth: 420,
  recentTargets: [],
  quickProtocol: "ssh",
};

interface PrefsState extends Prefs {
  set<K extends keyof Prefs>(key: K, value: Prefs[K]): void;
  addRecent(target: string): void;
}

export const usePrefs = create<PrefsState>()(
  persist(
    (set, get) => ({
      ...DEFAULTS,
      set(key, value) {
        set({ [key]: value } as Partial<Prefs>);
      },
      addRecent(target) {
        const rest = get().recentTargets.filter((t) => t !== target);
        set({ recentTargets: [target, ...rest].slice(0, 8) });
      },
    }),
    { name: "loki-prefs", version: 1 },
  ),
);
