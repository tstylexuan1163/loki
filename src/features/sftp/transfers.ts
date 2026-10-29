import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import type { TransferEvent } from "@/lib/api";

interface TransfersState {
  items: TransferEvent[];
  dismiss(id: string): void;
}

/** Finished transfers fade out after this long; failures stay until dismissed. */
const DONE_LINGER_MS = 4000;

export const useTransfers = create<TransfersState>((set, get) => ({
  items: [],
  dismiss(id) {
    set({ items: get().items.filter((t) => t.id !== id) });
  },
}));

let started = false;

/** Registered once at module level so events are not lost while no panel is open. */
export function startTransferListener() {
  if (started) return;
  started = true;
  void listen<TransferEvent>("transfer-event", (evt) => {
    const ev = evt.payload;
    const { items } = useTransfers.getState();
    const next = items.some((t) => t.id === ev.id)
      ? items.map((t) => (t.id === ev.id ? ev : t))
      : [...items, ev];
    useTransfers.setState({ items: next });
    if (ev.state === "done" || ev.state === "cancelled") {
      window.setTimeout(() => useTransfers.getState().dismiss(ev.id), DONE_LINGER_MS);
    }
  });
}
