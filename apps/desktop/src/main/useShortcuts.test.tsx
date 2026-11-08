import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const hoisted = vi.hoisted(() => ({
  currentTab: null as null | {
    active: boolean;
    id?: string;
    pinned?: boolean;
    returnToSlotId?: string;
    returnToTabId?: string;
    slotId: string;
    type: string;
  },
  canGoBack: false,
  newNote: vi.fn(),
  newNoteCurrent: vi.fn(),
  newNoteAndListen: vi.fn(),
  close: vi.fn(),
  goBack: vi.fn(),
  handlers: new Map<string, (event?: { defaultPrevented: boolean }) => void>(),
  openCurrent: vi.fn(),
  openNew: vi.fn(),
  select: vi.fn(),
  tabs: [] as {
    active: boolean;
    id?: string;
    returnToSlotId?: string;
    returnToTabId?: string;
    slotId: string;
    type: string;
  }[],
  unpin: vi.fn(),
  setPendingCloseConfirmationTab: vi.fn(),
}));

vi.mock("react-hotkeys-hook", () => ({
  useHotkeys: (
    keys: string,
    handler: (event?: { defaultPrevented: boolean }) => void,
  ) => {
    hoisted.handlers.set(keys, handler);
  },
}));

vi.mock("~/shared/useNewNote", () => ({
  useNewNote: (options?: { behavior?: string }) =>
    options?.behavior === "current" ? hoisted.newNoteCurrent : hoisted.newNote,
  useNewNoteAndListen: () => hoisted.newNoteAndListen,
}));

vi.mock("~/store/zustand/tabs", () => {
  const getTabsState = () => ({
    tabs: hoisted.tabs,
    currentTab: hoisted.currentTab,
    canGoBack: hoisted.canGoBack,
    clearSelection: vi.fn(),
    close: hoisted.close,
    goBack: hoisted.goBack,
    select: hoisted.select,
    selectNext: vi.fn(),
    selectPrev: vi.fn(),
    restoreLastClosedTab: vi.fn(),
    openNew: hoisted.openNew,
    openCurrent: hoisted.openCurrent,
    unpin: hoisted.unpin,
    setPendingCloseConfirmationTab: hoisted.setPendingCloseConfirmationTab,
  });

  const useTabs = ((
    selector: (state: ReturnType<typeof getTabsState>) => unknown,
  ) => selector(getTabsState())) as ((
    selector: (state: ReturnType<typeof getTabsState>) => unknown,
  ) => unknown) & { getState: typeof getTabsState };

  useTabs.getState = getTabsState;

  return {
    uniqueIdfromTab: (tab: {
      id?: string;
      requestId?: string;
      slotId: string;
      type: string;
    }) => {
      switch (tab.type) {
        case "sessions":
        case "task":
        case "daily_summary":
          return `${tab.type}-${tab.id}`;
        case "empty":
          return `empty-${tab.slotId}`;
        default:
          return tab.type;
      }
    },
    useTabs,
    useTabsStore: () => ({ getState: getTabsState }),
  };
});

vi.mock("~/stt/contexts", () => ({
  useListener: (
    selector: (state: {
      live: { sessionId: string | null; status: string };
    }) => unknown,
  ) =>
    selector({
      live: { sessionId: null, status: "idle" },
    }),
}));

import { useClassicMainShortcuts } from "~/main/useShortcuts";

describe("useClassicMainShortcuts", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    hoisted.close.mockClear();
    hoisted.currentTab = null;
    hoisted.canGoBack = false;
    hoisted.goBack.mockClear();
    hoisted.handlers.clear();
    hoisted.newNote.mockClear();
    hoisted.newNoteCurrent.mockClear();
    hoisted.newNoteAndListen.mockClear();
    hoisted.openCurrent.mockClear();
    hoisted.openNew.mockClear();
    hoisted.select.mockClear();
    hoisted.unpin.mockClear();
    hoisted.setPendingCloseConfirmationTab.mockClear();
    hoisted.tabs = [];
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it.each(["empty", "sessions"])(
    "routes Cmd/Ctrl+N from %s through the expected scoped creation intent",
    (type) => {
      hoisted.currentTab = { active: true, id: "note", slotId: "slot", type };
      renderHook(() => useClassicMainShortcuts());
      hoisted.handlers.get("mod+n")!();
      expect(
        type === "empty" ? hoisted.newNoteCurrent : hoisted.newNote,
      ).toHaveBeenCalledOnce();
      expect(
        type === "empty" ? hoisted.newNote : hoisted.newNoteCurrent,
      ).not.toHaveBeenCalled();
      expect(hoisted.newNoteAndListen).not.toHaveBeenCalled();
    },
  );

  it("keeps the explicit recording shortcut on its existing capture action", () => {
    renderHook(() => useClassicMainShortcuts());
    hoisted.handlers.get("mod+shift+n")!();
    expect(hoisted.newNoteAndListen).toHaveBeenCalledOnce();
    expect(hoisted.newNote).not.toHaveBeenCalled();
    expect(hoisted.newNoteCurrent).not.toHaveBeenCalled();
  });

  it("does not bind removed shortcuts", () => {
    renderHook(() => useClassicMainShortcuts());

    expect(hoisted.handlers.has("mod+t")).toBe(false);
    expect(hoisted.handlers.has("mod+w")).toBe(false);
    expect(
      hoisted.handlers.has(
        "mod+1, mod+2, mod+3, mod+4, mod+5, mod+6, mod+7, mod+8, mod+9",
      ),
    ).toBe(false);
    expect(hoisted.handlers.has("mod+alt+left")).toBe(false);
    expect(hoisted.handlers.has("mod+alt+right")).toBe(false);
    expect(hoisted.handlers.has("mod+shift+t")).toBe(false);
    expect(hoisted.handlers.has("mod+shift+comma")).toBe(false);
  });

  it("selects an existing home view from a note on escape", () => {
    const homeTab = {
      active: false,
      slotId: "slot-home",
      type: "empty",
    };
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };
    hoisted.tabs = [homeTab, hoisted.currentTab];

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape();
    vi.runOnlyPendingTimers();

    expect(hoisted.select).toHaveBeenCalledWith(homeTab);
    expect(hoisted.openCurrent).not.toHaveBeenCalled();
  });

  it("returns an escape shortcut action that opens home from a note", () => {
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };

    const { result } = renderHook(() => useClassicMainShortcuts());

    result.current.runEscapeShortcut();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("uses the latest tab state when the returned escape action runs", () => {
    hoisted.currentTab = {
      active: true,
      slotId: "slot-home",
      type: "empty",
    };

    const { result } = renderHook(() => useClassicMainShortcuts());

    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };

    result.current.runEscapeShortcut();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("opens home from a note when the editor stops escape propagation", () => {
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };
    const editor = document.createElement("div");
    editor.contentEditable = "true";
    editor.addEventListener("keydown", (event) => event.stopPropagation());
    document.body.append(editor);

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(editor);
    vi.runOnlyPendingTimers();
    editor.remove();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("opens home from a note when the editor prevents default escape handling", () => {
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };
    const editor = document.createElement("div");
    editor.className = "ProseMirror";
    editor.contentEditable = "true";
    editor.addEventListener("keydown", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    document.body.append(editor);

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(editor);
    vi.runOnlyPendingTimers();
    editor.remove();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("opens home from a note when ProseMirror handles escape from a text node", () => {
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };
    const editor = document.createElement("div");
    editor.className = "ProseMirror";
    editor.contentEditable = "true";
    const text = document.createTextNode("note");
    editor.append(text);
    editor.addEventListener("keydown", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    document.body.append(editor);

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(text);
    vi.runOnlyPendingTimers();
    editor.remove();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("does not duplicate home navigation when a focused target handles escape directly", () => {
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };
    const { result } = renderHook(() => useClassicMainShortcuts());
    const target = document.createElement("input");
    target.addEventListener("keydown", (event) => {
      event.preventDefault();
      event.stopPropagation();
      result.current.runEscapeShortcut();
    });
    document.body.append(target);

    dispatchEscape(target);
    vi.runOnlyPendingTimers();
    target.remove();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.openCurrent).toHaveBeenCalledTimes(1);
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("opens home from a note when the title field handles escape", () => {
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };
    const target = document.createElement("input");
    target.dataset.sessionTitleInput = "true";
    target.addEventListener("keydown", (event) => {
      event.preventDefault();
    });
    document.body.append(target);

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(target);
    vi.runOnlyPendingTimers();
    target.remove();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("opens home from a note when the session surface handles escape", () => {
    hoisted.currentTab = {
      active: true,
      id: "session-1",
      slotId: "slot-session",
      type: "sessions",
    };
    const surface = document.createElement("div");
    surface.dataset.sessionSurface = "true";
    const target = document.createElement("div");
    target.addEventListener("keydown", (event) => {
      event.preventDefault();
    });
    surface.append(target);
    document.body.append(surface);

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(target);
    vi.runOnlyPendingTimers();
    surface.remove();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("lets editor escape consumers handle the key before opening home", () => {
    hoisted.currentTab = {
      active: true,
      slotId: "slot-session",
      type: "sessions",
    };
    const editor = document.createElement("div");
    editor.className = "ProseMirror";
    editor.contentEditable = "true";
    editor.addEventListener("keydown", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    const menu = document.createElement("div");
    menu.dataset.editorEscapeConsumer = "true";
    document.body.append(editor, menu);

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(editor);
    vi.runOnlyPendingTimers();
    editor.remove();
    menu.remove();

    expect(hoisted.openCurrent).not.toHaveBeenCalled();
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("does not open home when an editor escape consumer unmounts before the shortcut runs", () => {
    hoisted.currentTab = {
      active: true,
      slotId: "slot-session",
      type: "sessions",
    };
    const editor = document.createElement("div");
    editor.className = "ProseMirror";
    editor.contentEditable = "true";
    const menu = document.createElement("div");
    menu.dataset.editorEscapeConsumer = "true";
    editor.addEventListener("keydown", (event) => {
      event.preventDefault();
      event.stopPropagation();
      menu.remove();
    });
    document.body.append(editor, menu);

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(editor);
    vi.runOnlyPendingTimers();
    editor.remove();

    expect(hoisted.openCurrent).not.toHaveBeenCalled();
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("selects an existing home tab on escape", () => {
    const homeTab = {
      active: false,
      slotId: "slot-home",
      type: "empty",
    };
    hoisted.currentTab = {
      active: true,
      slotId: "slot-settings",
      type: "settings",
    };
    hoisted.tabs = [homeTab, hoisted.currentTab];

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape();
    vi.runOnlyPendingTimers();

    expect(hoisted.select).toHaveBeenCalledWith(homeTab);
    expect(hoisted.openCurrent).not.toHaveBeenCalled();
  });

  it("returns settings to the tab it opened from on escape", () => {
    const sessionTab = {
      active: false,
      slotId: "slot-session",
      type: "sessions",
    };
    hoisted.currentTab = {
      active: true,
      returnToSlotId: "slot-session",
      slotId: "slot-settings",
      type: "settings",
    };
    hoisted.tabs = [
      sessionTab,
      {
        active: false,
        slotId: "slot-home",
        type: "empty",
      },
      hoisted.currentTab,
    ];

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape();
    vi.runOnlyPendingTimers();

    expect(hoisted.select).toHaveBeenCalledWith(sessionTab);
    expect(hoisted.openCurrent).not.toHaveBeenCalled();
    expect(hoisted.goBack).not.toHaveBeenCalled();
  });

  it("uses history when an origin-tracked tab replaced the current slot", () => {
    hoisted.currentTab = {
      active: true,
      returnToSlotId: "slot-session",
      slotId: "slot-session",
      type: "settings",
    };
    hoisted.canGoBack = true;
    hoisted.tabs = [hoisted.currentTab];

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape();
    vi.runOnlyPendingTimers();

    expect(hoisted.goBack).toHaveBeenCalledTimes(1);
    expect(hoisted.select).not.toHaveBeenCalled();
    expect(hoisted.openCurrent).not.toHaveBeenCalled();
  });

  it("opens home when the return origin is gone instead of using unrelated history", () => {
    hoisted.currentTab = {
      active: true,
      returnToSlotId: "slot-session",
      slotId: "slot-settings",
      type: "settings",
    };
    hoisted.canGoBack = true;
    hoisted.tabs = [hoisted.currentTab];

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape();
    vi.runOnlyPendingTimers();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.goBack).not.toHaveBeenCalled();
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("opens home when the origin slot now contains a different tab", () => {
    const reusedSlotTab = {
      active: false,
      id: "new-session",
      slotId: "slot-session",
      type: "sessions",
    };
    hoisted.currentTab = {
      active: true,
      returnToSlotId: "slot-session",
      returnToTabId: "sessions-old-session",
      slotId: "slot-settings",
      type: "settings",
    };
    hoisted.tabs = [reusedSlotTab, hoisted.currentTab];

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape();
    vi.runOnlyPendingTimers();

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
    expect(hoisted.goBack).not.toHaveBeenCalled();
  });

  it("opens home from a note when escape is prevented without a focused target", () => {
    hoisted.currentTab = {
      active: true,
      slotId: "slot-session",
      type: "sessions",
    };

    renderHook(() => useClassicMainShortcuts());

    const preventEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
      }
    };
    window.addEventListener("keydown", preventEscape);

    dispatchEscape();
    vi.runOnlyPendingTimers();
    window.removeEventListener("keydown", preventEscape);

    expect(hoisted.openCurrent).toHaveBeenCalledWith({ type: "empty" });
    expect(hoisted.select).not.toHaveBeenCalled();
  });

  it("lets focused targets consume escape", () => {
    hoisted.currentTab = {
      active: true,
      slotId: "slot-session",
      type: "sessions",
    };
    const input = document.createElement("input");
    input.addEventListener("keydown", (event) => {
      event.preventDefault();
    });
    document.body.append(input);
    input.focus();

    renderHook(() => useClassicMainShortcuts());

    dispatchEscape(input);
    vi.runOnlyPendingTimers();
    input.remove();

    expect(hoisted.openCurrent).not.toHaveBeenCalled();
    expect(hoisted.select).not.toHaveBeenCalled();
  });
});

function dispatchEscape(target: EventTarget = window) {
  target.dispatchEvent(
    new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      key: "Escape",
    }),
  );
}
