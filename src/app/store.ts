import { create } from "zustand";
import { api, errorMessage, type CategoryView, type HostView, type VaultStatus } from "@/lib/api";
import { useTerminals } from "@/features/terminal/terminals";

export type Phase = "boot" | "onboarding" | "locked" | "unlocked";
/**
 * Connection details handed to the host editor when "save as host" is used from
 * a quick-connect tab. Only non-secret fields: the editor still asks for
 * credentials itself.
 */
export interface HostPrefill {
  protocol: "ssh" | "telnet" | "serial";
  name?: string;
  address: string;
  port?: number;
  username?: string;
  baud?: number;
  dataBits?: number;
  parity?: string;
  stopBits?: number;
}

export type Page =
  | { kind: "hosts" }
  | { kind: "host-edit"; hostId: string | null; prefill?: HostPrefill }
  | { kind: "terminal" }
  | { kind: "keys" }
  | { kind: "forwards" };

interface AppState {
  phase: Phase;
  vault: VaultStatus | null;
  hosts: HostView[];
  /** Host categories in display order. */
  categories: CategoryView[];
  page: Page;
  error: string | null;
  busy: boolean;
  /** Non-null right after creation: the recovery code, shown exactly once. */
  recoveryCode: string | null;

  boot(): Promise<void>;
  createVault(password: string): Promise<boolean>;
  dismissRecoveryCode(): void;
  recover(code: string): Promise<boolean>;
  unlock(password: string): Promise<boolean>;
  lock(): Promise<void>;
  refreshHosts(): Promise<void>;
  openNewHost(): void;
  openHost(id: string): void;
  backToHosts(): void;
  clearError(): void;
  setError(message: string): void;
  setBusy(busy: boolean): void;
}

export const useApp = create<AppState>((set, get) => ({
  phase: "boot",
  vault: null,
  hosts: [],
  categories: [],
  page: { kind: "hosts" },
  error: null,
  busy: false,
  recoveryCode: null,

  async boot() {
    try {
      const vault = await api.vaultStatus();
      // The store starts locked even when a vault exists; the key is only in
      // memory after an explicit unlock.
      set({
        vault,
        phase: vault.exists ? "locked" : "onboarding",
      });
    } catch (e) {
      set({ error: errorMessage(e), phase: "onboarding" });
    }
  },

  async createVault(password) {
    set({ busy: true, error: null });
    try {
      const res = await api.vaultCreate(password);
      await get().refreshHosts();
      set({
        vault: res.status,
        phase: "unlocked",
        busy: false,
        recoveryCode: res.recoveryCode,
      });
      return true;
    } catch (e) {
      set({ error: errorMessage(e), busy: false });
      return false;
    }
  },

  async unlock(password) {
    set({ busy: true, error: null });
    try {
      const vault = await api.vaultUnlock(password);
      await get().refreshHosts();
      set({ vault, phase: "unlocked", busy: false });
      return true;
    } catch (e) {
      set({ error: errorMessage(e), busy: false });
      return false;
    }
  },

  dismissRecoveryCode() {
    // The code is only in memory for the onboarding step; clearing it means a
    // later screenshot or devtools session cannot recover it.
    set({ recoveryCode: null });
  },

  async recover(code) {
    set({ busy: true, error: null });
    try {
      const vault = await api.vaultRecover(code);
      await get().refreshHosts();
      set({ vault, phase: "unlocked", busy: false });
      return true;
    } catch (e) {
      set({ error: errorMessage(e), busy: false });
      return false;
    }
  },

  async lock() {
    await lockNow(set);
  },

  async refreshHosts() {
    try {
      // Categories carry host counts and renames touch hosts, so the two are
      // always fetched together.
      const [hosts, categories] = await Promise.all([api.hostsList(), api.categoriesList()]);
      set({ hosts, categories });
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  openNewHost() {
    set({ page: { kind: "host-edit", hostId: null }, error: null });
  },

  openHost(id) {
    set({ page: { kind: "host-edit", hostId: id }, error: null });
  },

  backToHosts() {
    set({ page: { kind: "hosts" }, error: null });
  },

  clearError() {
    set({ error: null });
  },

  setError(message) {
    set({ error: message });
  },

  setBusy(busy) {
    set({ busy });
  },
}));

/// Last time the user did something; the auto-lock hook reads it.
let lastActivityAt = Date.now();

/// Locks the vault and returns the app to the unlock screen.
///
/// Every in-memory secret-bearing view is dropped: host plaintext, terminal
/// tabs, and the recovery code shown during onboarding.
async function lockNow(
  set: (partial: Partial<AppState>) => void,
): Promise<void> {
  // The backend closes every connection on lock; the tabs must go with them
  // or the strip would show sessions that no longer exist.
  useTerminals.getState().reset();
  try {
    const vault = await api.vaultLock();
    set({
      vault,
      phase: "locked",
      hosts: [],
      categories: [],
      page: { kind: "hosts" },
      recoveryCode: null,
    });
  } catch {
    // Locking must always succeed from the user's point of view; if the backend
    // call fails the UI still hides what it holds.
    set({ phase: "locked", hosts: [], categories: [], page: { kind: "hosts" }, recoveryCode: null });
  }
}

/// Exposed so the idle watcher can trigger a lock without touching UI state.
export const lockVaultNow = () => lockNow(useApp.setState as (p: Partial<AppState>) => void);

/// Marks activity, for the idle watcher.
export const noteActivity = () => {
  lastActivityAt = Date.now();
};

/// True when the idle deadline has passed.
export const idleExpired = (minutes: number) =>
  minutes > 0 && Date.now() - lastActivityAt >= minutes * 60_000;
