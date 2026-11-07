import { useCallback, useRef } from "react";
import { useHotkeys } from "react-hotkeys-hook";

import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { useNewNote, useNewNoteAndListen } from "~/shared/useNewNote";
import { uniqueIdfromTab, useTabs, useTabsStore } from "~/store/zustand/tabs";

export function useMainShortcuts() {
  const runEscapeShortcut = useMainEscapeShortcutAction();
  const currentTab = useTabs((state) => state.currentTab);

  const newNote = useNewNote();
  const newNoteCurrent = useNewNote({ behavior: "current" });

  const escapeShortcutRef = useRef(runEscapeShortcut);
  escapeShortcutRef.current = runEscapeShortcut;

  useMountEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        return;
      }

      const fromProseMirrorEditor = isFromProseMirrorEditor(event.target);
      const fromSessionTitleInput = isFromSessionTitleInput(event.target);
      const fromSessionSurface = isFromSessionSurface(event.target);
      const hadEditorEscapeConsumer =
        fromProseMirrorEditor &&
        document.querySelector("[data-editor-escape-consumer]") !== null;
      const hadMeaningfulFocus = hasMeaningfulFocus(event.target);

      window.setTimeout(() => {
        if (
          shouldSkipEscapeShortcut(event, {
            fromProseMirrorEditor,
            fromSessionTitleInput,
            fromSessionSurface,
            hadEditorEscapeConsumer,
            hadMeaningfulFocus,
          })
        ) {
          return;
        }

        escapeShortcutRef.current();
      });
    };

    window.addEventListener("keydown", handleKeyDown, { capture: true });
    return () => {
      window.removeEventListener("keydown", handleKeyDown, { capture: true });
    };
  });

  useHotkeys(
    "mod+n",
    () => {
      if (currentTab?.type === "empty") {
        newNoteCurrent();
      } else {
        newNote();
      }
    },
    {
      preventDefault: true,
      enableOnFormTags: true,
      enableOnContentEditable: true,
    },
    [currentTab, newNote, newNoteCurrent],
  );

  const newNoteAndListen = useNewNoteAndListen();

  useHotkeys(
    "mod+shift+n",
    () => newNoteAndListen(),
    {
      preventDefault: true,
      enableOnFormTags: true,
      enableOnContentEditable: true,
    },
    [newNoteAndListen],
  );

  return { runEscapeShortcut };
}

export function useMainEscapeShortcutAction() {
  const store = useTabsStore();
  return useCallback(() => {
    const { tabs, currentTab, openCurrent, select, goBack, canGoBack } =
      store.getState();

    if (currentTab?.type === "onboarding" || currentTab?.type === "empty") {
      return;
    }

    const returnToSlotId = currentTab?.returnToSlotId;
    const returnToTab = returnToSlotId
      ? tabs.find(
          (tab) =>
            tab.slotId === returnToSlotId &&
            tab.slotId !== currentTab?.slotId &&
            (!currentTab?.returnToTabId ||
              uniqueIdfromTab(tab) === currentTab.returnToTabId),
        )
      : null;
    if (returnToTab) {
      select(returnToTab);
      return;
    }

    if (returnToSlotId === currentTab?.slotId && canGoBack) {
      goBack();
      return;
    }

    const existingHomeTab = tabs.find((tab) => tab.type === "empty");
    if (existingHomeTab) {
      select(existingHomeTab);
      return;
    }

    openCurrent({ type: "empty" });
  }, [store]);
}

function shouldSkipEscapeShortcut(
  event: KeyboardEvent,
  {
    fromProseMirrorEditor,
    fromSessionTitleInput,
    fromSessionSurface,
    hadEditorEscapeConsumer,
    hadMeaningfulFocus,
  }: {
    fromProseMirrorEditor: boolean;
    fromSessionTitleInput: boolean;
    fromSessionSurface: boolean;
    hadEditorEscapeConsumer: boolean;
    hadMeaningfulFocus: boolean;
  },
) {
  if (!event.defaultPrevented) {
    return false;
  }

  if (!hadMeaningfulFocus) {
    return false;
  }

  if (fromSessionTitleInput || fromSessionSurface) {
    return false;
  }

  if (!fromProseMirrorEditor) {
    return true;
  }

  return hadEditorEscapeConsumer;
}

function hasMeaningfulFocus(target: EventTarget | null) {
  if (isMeaningfulEscapeTarget(target)) {
    return true;
  }

  const activeElement = document.activeElement;

  return (
    activeElement instanceof HTMLElement &&
    activeElement !== document.body &&
    activeElement !== document.documentElement
  );
}

function isMeaningfulEscapeTarget(target: EventTarget | null) {
  const element =
    target instanceof Element
      ? target
      : target instanceof Node
        ? target.parentElement
        : null;

  return (
    element !== null &&
    element !== document.body &&
    element !== document.documentElement
  );
}

function isFromProseMirrorEditor(target: EventTarget | null) {
  const element =
    target instanceof Element
      ? target
      : target instanceof Node
        ? target.parentElement
        : null;

  return element !== null && element.closest(".ProseMirror") !== null;
}

function isFromSessionTitleInput(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest("[data-session-title-input]") !== null
  );
}

function isFromSessionSurface(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.closest("[data-session-surface]") !== null
  );
}
