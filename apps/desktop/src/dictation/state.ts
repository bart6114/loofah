import { useQuery, type QueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

import type { DictationStatus } from "~/types/tauri.gen";

export const dictationQueryKey = ["dictation-status"] as const;

let shortcutRegistration = Promise.resolve();

export function configureDictationShortcut(shortcut: string) {
  const pending = shortcutRegistration.then(() =>
    invoke<void>("plugin:windows|dictation_shortcut_configure", { shortcut }),
  );
  shortcutRegistration = pending.catch(() => {});
  return pending;
}

export const useDictationShortcut = create<{
  capturing: boolean;
  unavailable: boolean;
}>(() => ({ capturing: false, unavailable: false }));

export function acceptDictationStatus(
  client: QueryClient,
  next: DictationStatus,
) {
  client.setQueryData<DictationStatus>(dictationQueryKey, (current) =>
    !current || next.revision > current.revision ? next : current,
  );
}

export function useDictationStatus() {
  return useQuery({
    queryKey: dictationQueryKey,
    queryFn: () => invoke<DictationStatus>("dictation_get_status"),
    staleTime: Infinity,
    structuralSharing: (previous, next) => {
      const oldStatus = previous as DictationStatus | undefined;
      const newStatus = next as DictationStatus;
      return oldStatus && oldStatus.revision > newStatus.revision
        ? oldStatus
        : newStatus;
    },
  });
}

export function isDictationCapturing(status: DictationStatus | undefined) {
  return status?.phase === "starting" || status?.phase === "recording";
}

export function isDictationActive(status: DictationStatus | undefined) {
  return Boolean(status?.busy || isDictationCapturing(status));
}
