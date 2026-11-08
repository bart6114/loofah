import { create } from "zustand";

import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import type { VaultScope } from "~/shared/application/vault-scope";

export type LiveTitleDraft = {
  scope: VaultScope;
  sessionId: string;
  owner: object;
  title: string;
  revision: string;
  baselineTitle: string;
  uncertain: boolean;
  legacy: boolean;
};

function key(scope: VaultScope, sessionId: string) {
  return JSON.stringify([scope.client.scope, sessionId]);
}

export const useLiveTitle = create<{
  titles: Record<string, LiveTitleDraft[]>;
  setTitle: (draft: LiveTitleDraft) => void;
  clearTitle: (draft: LiveTitleDraft) => void;
  replaceTitle: (previous: LiveTitleDraft, draft: LiveTitleDraft) => void;
}>((set) => ({
  titles: {},
  setTitle: (draft) =>
    set((state) => {
      const id = key(draft.scope, draft.sessionId);
      const previous = state.titles[id] ?? [];
      return {
        titles: {
          ...state.titles,
          [id]: [
            ...previous.filter((value) => value.scope !== draft.scope),
            draft,
          ],
        },
      };
    }),
  replaceTitle: (previous, draft) =>
    set((state) => {
      const id = key(previous.scope, previous.sessionId);
      const entries = state.titles[id];
      if (
        !entries?.includes(previous) ||
        draft.scope !== previous.scope ||
        draft.sessionId !== previous.sessionId ||
        draft.owner !== previous.owner
      )
        return state;
      return {
        titles: {
          ...state.titles,
          [id]: entries.map((value) => (value === previous ? draft : value)),
        },
      };
    }),
  clearTitle: (draft) =>
    set((state) => {
      const id = key(draft.scope, draft.sessionId);
      const entries = state.titles[id];
      if (!entries?.includes(draft)) return state;
      const remaining = entries.filter((value) => value !== draft);
      if (remaining.length)
        return { titles: { ...state.titles, [id]: remaining } };
      const { [id]: _, ...rest } = state.titles;
      return { titles: rest };
    }),
}));

export function liveSessionTitleDraft(scope: VaultScope, sessionId: string) {
  return useLiveTitle
    .getState()
    .titles[key(scope, sessionId)]?.find((value) => value.scope === scope);
}

export function hasLiveSessionTitleDraft(sessionId: string): boolean {
  return Object.values(useLiveTitle.getState().titles).some((entries) =>
    entries.some((draft) => draft.legacy && draft.sessionId === sessionId),
  );
}

export function useLiveSessionTitleDraft(scope: VaultScope, sessionId: string) {
  return useLiveTitle((state) =>
    state.titles[key(scope, sessionId)]?.find((value) => value.scope === scope),
  );
}

export function useSessionTitle(
  sessionId: string,
  storeTitle: string | undefined,
): string {
  const owner = useVaultReadScope();
  const draft = useLiveSessionTitleDraft(owner.scope, sessionId);
  if (!owner.readable) return "Untitled";
  return draft?.title ?? storeTitle ?? "Untitled";
}
