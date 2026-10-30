import { create } from "zustand";

/**
 * A one-slot handoff from another panel into the assistant's input box.
 *
 * The files panel writes here when "send to assistant" is chosen; the panel
 * picks it up and clears it. A store rather than a window event so it does not
 * depend on any component being mounted at the right moment.
 */
interface AiComposerState {
  pendingInsert: string | null;
  setPendingInsert(text: string | null): void;
}

export const useAiComposer = create<AiComposerState>((set) => ({
  pendingInsert: null,
  setPendingInsert: (pendingInsert) => set({ pendingInsert }),
}));
