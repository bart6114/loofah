import { create } from "zustand";

import { id } from "~/shared/utils";

export type AudioImportSource =
  | { kind: "path"; path: string; name: string }
  | { kind: "file"; file: File; name: string };

export type AudioImportStatus =
  | "pending"
  | "preparing"
  | "importing"
  | "transcribing"
  | "incomplete"
  | "done"
  | "failed";

export type AudioImportItem = {
  id: string;
  source: AudioImportSource;
  sessionId: string | null;
  status: AudioImportStatus;
  percentage: number;
  error: string | null;
  attempt: number;
};

const ACTIVE_STATUSES: AudioImportStatus[] = [
  "preparing",
  "importing",
  "transcribing",
];

export function isFinishedAudioImportStatus(status: AudioImportStatus) {
  return status === "done" || status === "failed" || status === "incomplete";
}

interface AudioImportState {
  items: AudioImportItem[];
  activeItemId: string | null;
  dialogOpen: boolean;
  completionAnnounced: boolean;
  setDialogOpen: (open: boolean) => void;
  enqueue: (sources: AudioImportSource[]) => void;
  claimNext: () => void;
  setItemSession: (itemId: string, sessionId: string) => void;
  setItemStatus: (itemId: string, status: "importing" | "transcribing") => void;
  setItemProgress: (itemId: string, percentage: number) => void;
  finishItem: (
    itemId: string,
    error?: string | null,
    status?: "failed" | "incomplete",
  ) => void;
  retryItem: (itemId: string) => void;
  clearFinished: () => void;
  markCompletionAnnounced: () => void;
}

function updateItem(
  items: AudioImportItem[],
  itemId: string,
  patch: Partial<AudioImportItem>,
) {
  return items.map((item) =>
    item.id === itemId ? { ...item, ...patch } : item,
  );
}

export const useAudioImport = create<AudioImportState>((set, get) => ({
  items: [],
  activeItemId: null,
  dialogOpen: false,
  completionAnnounced: false,

  setDialogOpen: (dialogOpen) => set({ dialogOpen }),

  enqueue: (sources) =>
    set((state) => ({
      completionAnnounced: false,
      items: [
        ...state.items,
        ...sources.map((source) => ({
          id: id(),
          source,
          sessionId: null,
          status: "pending" as const,
          percentage: 0,
          error: null,
          attempt: 0,
        })),
      ],
    })),

  claimNext: () => {
    const state = get();
    if (state.activeItemId) {
      return;
    }

    const next = state.items.find((item) => item.status === "pending");
    if (!next) {
      return;
    }

    set({
      activeItemId: next.id,
      items: updateItem(state.items, next.id, {
        status: "preparing",
        percentage: 0,
        error: null,
      }),
    });
  },

  setItemSession: (itemId, sessionId) =>
    set((state) => ({ items: updateItem(state.items, itemId, { sessionId }) })),

  setItemStatus: (itemId, status) =>
    set((state) => ({
      items: updateItem(state.items, itemId, { status, percentage: 0 }),
    })),

  setItemProgress: (itemId, percentage) =>
    set((state) => ({
      items: updateItem(state.items, itemId, { percentage }),
    })),

  finishItem: (itemId, error, status = "failed") =>
    set((state) => ({
      activeItemId: state.activeItemId === itemId ? null : state.activeItemId,
      items: updateItem(
        state.items,
        itemId,
        error
          ? { status, error }
          : { status: "done", percentage: 1, error: null },
      ),
    })),

  retryItem: (itemId) =>
    set((state) => {
      const item = state.items.find((entry) => entry.id === itemId);
      if (!item || item.status !== "failed") {
        return state;
      }

      return {
        ...state,
        completionAnnounced: false,
        items: updateItem(state.items, itemId, {
          status: "pending",
          percentage: 0,
          error: null,
          attempt: item.attempt + 1,
        }),
      };
    }),

  clearFinished: () =>
    set((state) => ({
      items: state.items.filter(
        (item) => !isFinishedAudioImportStatus(item.status),
      ),
    })),

  markCompletionAnnounced: () => set({ completionAnnounced: true }),
}));

// Read by the batch-completion path so queue items don't fire one
// notification per file; the worker sends a single aggregate one instead.
export function isActiveAudioImportSession(sessionId: string) {
  return useAudioImport
    .getState()
    .items.some(
      (item) =>
        item.sessionId === sessionId && ACTIVE_STATUSES.includes(item.status),
    );
}
