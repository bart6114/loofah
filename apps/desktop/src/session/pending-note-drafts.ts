import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";

import {
  areEquivalentEditorContents,
  type JSONContent,
  type EditorSaveResult,
} from "@hypr/editor/note";

const drafts = new Map<
  string,
  {
    getContent: () => JSONContent;
    saved: boolean;
    sourceKey?: string;
    readConfirmedContent?: () => JSONContent;
    hasConfirmedSource?: boolean;
  }
>();
const listeners = new Map<string, Set<() => void>>();

function notify(key: string) {
  listeners.get(key)?.forEach((listener) => listener());
}

export function captureNoteDraft(
  key: string,
  getContent: () => JSONContent,
  sourceKey?: string,
) {
  // Capture the immutable editor document before React can render a replacement
  // editor; serialization and subscriber updates wait until save or remount.
  drafts.set(key, { getContent, saved: false, sourceKey });
}

export async function persistNoteDraft(
  key: string,
  content: JSONContent,
  write: () => Promise<EditorSaveResult>,
  sourceKey?: string,
): Promise<EditorSaveResult> {
  const draft = { getContent: () => content, saved: false, sourceKey };
  drafts.set(key, draft);
  notify(key);

  const confirmedContent = await write();
  let lastConfirmedContent =
    typeof confirmedContent === "function"
      ? confirmedContent()
      : (confirmedContent ?? content);
  const readConfirmedContent = () => {
    if (typeof confirmedContent === "function") {
      try {
        lastConfirmedContent = confirmedContent();
      } catch {
        // An unmounted note can outlive its inactive query cache.
      }
    }
    return lastConfirmedContent;
  };

  if (drafts.get(key) === draft) {
    drafts.set(key, {
      ...draft,
      saved: true,
      readConfirmedContent,
      hasConfirmedSource: typeof confirmedContent === "function",
    });
    notify(key);
  }
  return confirmedContent;
}

export function usePendingNoteDraft(
  key: string | undefined,
  persistedContent: JSONContent,
  sourceKey?: string,
) {
  const subscribe = useCallback(
    (listener: () => void) => {
      if (!key) return () => {};
      let subscribers = listeners.get(key);
      if (!subscribers) {
        subscribers = new Set();
        listeners.set(key, subscribers);
      }
      subscribers.add(listener);
      return () => {
        subscribers.delete(listener);
        if (subscribers.size === 0) listeners.delete(key);
      };
    },
    [key],
  );
  const getSnapshot = useCallback(
    () => (key ? drafts.get(key) : undefined),
    [key],
  );
  const draft = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  useEffect(() => {
    if (
      key &&
      draft?.saved &&
      drafts.get(key) === draft &&
      (draft.sourceKey !== sourceKey ||
        areEquivalentEditorContents(
          draft.readConfirmedContent?.() ?? draft.getContent(),
          persistedContent,
        ))
    ) {
      drafts.delete(key);
      notify(key);
    }
  }, [key, draft, persistedContent, sourceKey]);

  const initialDraft = useCallback(() => {
    const pending = key ? drafts.get(key) : undefined;
    if (pending?.saved && pending.sourceKey !== sourceKey) return undefined;
    return pending?.readConfirmedContent?.() ?? pending?.getContent();
  }, [key, draft, sourceKey]);
  const persistedContentRef = useRef(persistedContent);
  persistedContentRef.current = persistedContent;
  const readPersistedContent = useCallback(
    () => persistedContentRef.current,
    [],
  );
  return {
    initialDraft,
    confirmedDraft:
      draft?.saved && draft.hasConfirmedSource
        ? draft.sourceKey === sourceKey
          ? draft.readConfirmedContent
          : readPersistedContent
        : undefined,
  };
}
