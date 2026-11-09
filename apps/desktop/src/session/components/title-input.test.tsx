import { onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode, type ComponentProps } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@hypr/ui/components/ui/tooltip";

import { TitleInput } from "./title-input";

import {
  createSessionSourceHost,
  sessionMetadata,
} from "~/session/session-source.test-support";
import { NativeDraftStore } from "~/shared/application/draft-store";
import { sessionEditorFlushKey } from "~/shared/application/session-cleanup-guard";
import { registerWriteFlusher } from "~/shared/write-queue";
import {
  useLiveTitle,
  liveSessionTitleDraft,
} from "~/store/zustand/live-title";

const hoisted = vi.hoisted(() => ({
  generating: vi.fn(() => false),
  storeTitle: "Untitled" as string | undefined,
}));
vi.mock("usehooks-ts", () => ({ useResizeObserver: vi.fn() }));
vi.mock("~/ai/hooks", () => ({ useTitleGenerating: hoisted.generating }));
const inventory = vi.spyOn(NativeDraftStore.prototype, "inventory");
const clearTitle = vi.spyOn(useLiveTitle.getState(), "clearTitle");
let host: ReturnType<typeof createSessionSourceHost>;
const tab = {
  active: true,
  id: "session-1",
  pinned: false,
  slotId: "slot-1",
  state: { autoStart: null, view: null },
  type: "sessions",
} as const;
async function renderTitleInput(
  props: Partial<ComponentProps<typeof TitleInput>> = {},
) {
  host.setMetadata(sessionMetadata(hoisted.storeTitle ?? ""));
  const view = render(
    <host.Host>
      <TooltipProvider>
        <TitleInput tab={tab} {...props} />
      </TooltipProvider>
    </host.Host>,
  );
  await waitFor(() =>
    expect(
      (screen.getByPlaceholderText("Untitled") as HTMLInputElement).readOnly,
    ).toBe(false),
  );
  return view;
}

describe("TitleInput", async () => {
  beforeEach(() => {
    vi.clearAllMocks();
    hoisted.generating.mockReturnValue(false);
    onlineManager.setOnline(true);
    hoisted.storeTitle = "Untitled";
    host = createSessionSourceHost();
    inventory.mockReset().mockResolvedValue([]);
    useLiveTitle.setState({ titles: {} });
  });

  afterEach(() => {
    cleanup();
    host.dispose();
    onlineManager.setOnline(true);
  });

  it("does not route escape from the title field into tab navigation", async () => {
    await renderTitleInput();

    fireEvent.keyDown(screen.getByPlaceholderText("Untitled"), {
      key: "Escape",
    });

    expect(clearTitle).not.toHaveBeenCalled();
  });

  it("does not handle IME confirmation keys as title navigation", async () => {
    const onTransferContentToEditor = vi.fn();
    const onFocusEditorAtStart = vi.fn();
    await renderTitleInput({
      onFocusEditorAtStart,
      onTransferContentToEditor,
    });

    const input = screen.getByPlaceholderText("Untitled");
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "안" } });
    fireEvent.keyDown(input, {
      key: "Enter",
      keyCode: 229,
    });

    expect(host.saveMetadata).not.toHaveBeenCalled();
    expect(clearTitle).not.toHaveBeenCalled();
    expect(onTransferContentToEditor).not.toHaveBeenCalled();
    expect(onFocusEditorAtStart).not.toHaveBeenCalled();
  });

  it("left-aligns the empty title field without a generate button", async () => {
    hoisted.storeTitle = "";

    await renderTitleInput();

    const input = screen.getByPlaceholderText("Untitled");
    expect(input.parentElement?.className).toContain("relative");
    expect(input.parentElement?.className).toContain("max-w-full");
    expect(input.parentElement?.className).toContain("text-[26px]");
    expect(input.parentElement?.className).toContain("font-semibold");
    expect(input.parentElement?.classList.contains("w-full")).toBe(false);
    expect(input.className).toContain("text-left");
    expect(
      screen.queryByRole("button", { name: "Regenerate title" }),
    ).toBeNull();
  });

  it("persists the full title on Enter when no editor callbacks are wired", async () => {
    await renderTitleInput();

    const input = screen.getByPlaceholderText("Untitled");
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "Weekly Sync" } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.blur(input, { target: { value: "Weekly Sync" } });

    await waitFor(() =>
      expect(host.saveMetadata).toHaveBeenCalledWith("session-1", {
        patch: { title: "Weekly Sync" },
        expected_revision: "m0",
      }),
    );
    await waitFor(() => expect(clearTitle).toHaveBeenCalled());
    expect(host.saveMetadata).toHaveBeenCalledOnce();
  });

  it("keeps the title field out of the header drag region", async () => {
    await renderTitleInput();

    const input = screen.getByPlaceholderText("Untitled");

    expect(input.getAttribute("data-tauri-drag-region")).toBe("false");
    expect(input.parentElement?.getAttribute("data-tauri-drag-region")).toBe(
      "false",
    );
  });

  it("uses sans-serif styling for breadcrumb titles", async () => {
    await renderTitleInput({ variant: "breadcrumb" });

    const input = screen.getByPlaceholderText("Untitled");

    expect(input.parentElement?.className).toContain("text-sm");
    expect(input.parentElement?.className).toContain("leading-5");
    expect(input.parentElement?.className).not.toContain("font-mono");
    expect(input.className).toContain("text-sm");
    expect(input.className).toContain("leading-5");
    expect(input.className).toContain("appearance-none");
    expect(input.className).toContain("p-0");
    expect(input.className).toContain("truncate");
    expect(input.className).not.toContain("font-mono");
  });

  it("keeps focused breadcrumb titles horizontally scrollable", async () => {
    await renderTitleInput({ variant: "breadcrumb" });

    const input = screen.getByPlaceholderText("Untitled");
    fireEvent.focus(input);

    expect(input.className).not.toContain("truncate");
    expect(input.className).toContain("overflow-x-auto");
    expect(input.className).toContain("whitespace-nowrap");
  });

  it("uses the flexible title layout for whitespace-only titles", async () => {
    hoisted.storeTitle = "          ";

    await renderTitleInput();

    const input = screen.getByPlaceholderText("Untitled");
    expect(input.className).toContain("w-full");
    expect(input.parentElement?.style.width).toBe("calc(10ch + 2px)");
    expect(
      screen.queryByRole("button", { name: "Regenerate title" }),
    ).toBeNull();
  });

  it("reveals overflowing titles with a hover scroll overlay", async () => {
    const title =
      "Product Discovery Pace and Headless Agent Usage Strategy Review";

    await renderTitleInput();

    const input = screen.getByPlaceholderText("Untitled");
    Object.defineProperty(input, "clientWidth", {
      configurable: true,
      value: 160,
    });
    Object.defineProperty(input, "scrollWidth", {
      configurable: true,
      value: 420,
    });

    fireEvent.change(input, { target: { value: title } });

    const hoverTitle = screen.getByText(title);
    const overlay = hoverTitle.parentElement;
    expect(input.className).toContain("text-transparent");
    expect(input.parentElement?.style.maskImage).toBe(
      "linear-gradient(to right, black 0, black calc(100% - 28px), transparent 100%)",
    );
    expect(overlay?.className).toContain("justify-start");
    expect(hoverTitle.className).toContain(
      "group-hover/title-input:animate-title-hover-scroll",
    );
    expect(
      hoverTitle.style.getPropertyValue("--title-hover-scroll-distance"),
    ).toBe("-260px");
  });

  it("updates when persisted metadata arrives after mount", async () => {
    let finish!: () => void;
    host.readMetadata.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finish = () => done(sessionMetadata("Spotify Leadership Transition"));
        }),
    );
    render(
      <host.Host>
        <TitleInput tab={tab} />
      </host.Host>,
    );
    expect(
      (screen.getByPlaceholderText("Untitled") as HTMLInputElement).value,
    ).toBe("");
    await waitFor(() => expect(host.readMetadata).toHaveBeenCalledOnce());
    await act(async () => finish());
    await waitFor(() =>
      expect(
        (screen.getByPlaceholderText("Untitled") as HTMLInputElement).value,
      ).toBe("Spotify Leadership Transition"),
    );
    expect(host.readNoteEditor).not.toHaveBeenCalled();
  });

  it("updates title fades based on horizontal scroll position", async () => {
    await renderTitleInput();

    const input = screen.getByPlaceholderText("Untitled");
    Object.defineProperty(input, "clientWidth", {
      configurable: true,
      value: 160,
    });
    Object.defineProperty(input, "scrollWidth", {
      configurable: true,
      value: 420,
    });

    fireEvent.change(input, {
      target: {
        value:
          "Product Discovery Pace and Headless Agent Usage Strategy Review",
      },
    });

    const titleInputShell = input.parentElement;
    expect(titleInputShell?.style.maskImage).toBe(
      "linear-gradient(to right, black 0, black calc(100% - 28px), transparent 100%)",
    );

    input.scrollLeft = 130;
    fireEvent.scroll(input);

    expect(titleInputShell?.style.maskImage).toBe(
      "linear-gradient(to right, transparent 0, black 28px, black calc(100% - 28px), transparent 100%)",
    );

    input.scrollLeft = 260;
    fireEvent.scroll(input);

    expect(titleInputShell?.style.maskImage).toBe(
      "linear-gradient(to right, transparent 0, black 28px, black 100%)",
    );
  });
});

describe("captured title persistence", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    hoisted.generating.mockReturnValue(false);
    hoisted.storeTitle = "Untitled";
    onlineManager.setOnline(true);
    host = createSessionSourceHost();
    inventory.mockReset().mockResolvedValue([]);
    useLiveTitle.setState({ titles: {} });
  });
  afterEach(() => {
    cleanup();
    host.dispose();
    onlineManager.setOnline(true);
  });
  const input = () =>
    screen.getByRole("textbox", { name: "Session title" }) as HTMLInputElement;
  const type = (title: string) => {
    fireEvent.focus(input());
    fireEvent.change(input(), { target: { value: title } });
  };
  it("pins metadata while typing, then requires explicit review and CAS retry after an external update", async () => {
    await renderTitleInput();
    type("Keep my typed title");
    host.setMetadata(
      sessionMetadata("Other writer", "session-1", "m-external"),
    );
    act(() =>
      host.emit({
        kind: "change",
        change: {
          kind: "index_changed",
          entity: "sessions",
          ids: ["session-1"],
        },
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Other writer"),
    );
    expect(input().value).toBe("Keep my typed title");
    fireEvent.blur(input());
    expect(host.saveMetadata).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Review saved title" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(input().value).toBe("Keep my typed title");
    expect(host.saveMetadata).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Save title" }));
    await waitFor(() =>
      expect(host.saveMetadata).toHaveBeenCalledWith("session-1", {
        patch: { title: "Keep my typed title" },
        expected_revision: "m-external",
      }),
    );
  });
  it("retains typing after a real CAS conflict and never silently retries", async () => {
    await renderTitleInput();
    type("Mine");
    host.setMetadata(
      sessionMetadata("Conflicting title", "session-1", "conflict"),
    );
    fireEvent.blur(input());
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain(
        "Conflicting title",
      ),
    );
    expect(input().value).toBe("Mine");
    expect(host.saveMetadata).toHaveBeenCalledOnce();
    expect(liveSessionTitleDraft(host.scope, "session-1")?.uncertain).toBe(
      true,
    );
  });
  it("retains an unknown save through remount, reviews canonical title and avoids duplicate submission", async () => {
    await renderTitleInput();
    type("Committed, reply lost");
    host.saveMetadata.mockImplementationOnce(async (_id, request) => {
      host.setMetadata(
        sessionMetadata(request.patch.title ?? "", "session-1", "committed"),
      );
      throw new Error("Reply lost");
    });
    fireEvent.blur(input());
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Reply lost"),
    );
    cleanup();
    render(
      <host.Host>
        <TitleInput tab={tab} />
      </host.Host>,
    );
    await waitFor(() => expect(input().value).toBe("Committed, reply lost"));
    fireEvent.click(
      screen.getByRole("button", { name: "Continue editing draft" }),
    );
    expect(
      screen.getByRole("button", { name: "Review saved title" }),
    ).toBeDefined();
    fireEvent.blur(input());
    expect(host.saveMetadata).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Review saved title" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(liveSessionTitleDraft(host.scope, "session-1")).toBeUndefined();
    expect(host.saveMetadata).toHaveBeenCalledOnce();
  });
  it("waits for pending body flush before dispatch and preserves first-500ms typing", async () => {
    let finish!: () => void;
    const stop = registerWriteFlusher(
      sessionEditorFlushKey(host.client.scope, "session-1"),
      () =>
        new Promise<void>((done) => {
          finish = done;
        }),
    );
    try {
      await renderTitleInput();
      type("Immediate title");
      fireEvent.blur(input());
      await waitFor(() => expect(finish).toBeDefined());
      expect(host.saveMetadata).not.toHaveBeenCalled();
      expect(input().value).toBe("Immediate title");
      await act(async () => finish());
      await waitFor(() => expect(host.saveMetadata).toHaveBeenCalledOnce());
    } finally {
      stop();
    }
  });
  it("preserves an unsent title when body flush or native inventory cannot be confirmed", async () => {
    const stop = registerWriteFlusher(
      sessionEditorFlushKey(host.client.scope, "session-1"),
      async () => {
        throw new Error("Pending body save lost");
      },
    );
    try {
      await renderTitleInput();
      type("Keep title");
      fireEvent.blur(input());
      await waitFor(() =>
        expect(screen.getByRole("alert").textContent).toContain(
          "Pending body save lost",
        ),
      );
      expect(host.saveMetadata).not.toHaveBeenCalled();
      expect(input().value).toBe("Keep title");
    } finally {
      stop();
    }
  });
  it("never replaces another mounted field's pending draft merely by focusing, and offers explicit handoff", async () => {
    let finish!: () => void;
    const stop = registerWriteFlusher(
      sessionEditorFlushKey(host.client.scope, "session-1"),
      () =>
        new Promise<void>((done) => {
          finish = done;
        }),
    );
    const content = (first: boolean) => (
      <host.Host>
        {first ? <TitleInput key="first" tab={tab} /> : null}
        <TitleInput key="second" tab={{ ...tab, slotId: "second" }} />
      </host.Host>
    );
    try {
      const view = render(content(true));
      await waitFor(() =>
        expect(
          (screen.getAllByRole("textbox")[0] as HTMLInputElement).readOnly,
        ).toBe(false),
      );
      const [first, second] = screen.getAllByRole(
        "textbox",
      ) as HTMLInputElement[];
      fireEvent.focus(first);
      fireEvent.change(first, { target: { value: "Private unsent title" } });
      fireEvent.blur(first);
      await waitFor(() => expect(finish).toBeDefined());
      const original = liveSessionTitleDraft(host.scope, "session-1");
      fireEvent.focus(second);
      expect(second.readOnly).toBe(true);
      expect(second.value).toBe("Private unsent title");
      expect(liveSessionTitleDraft(host.scope, "session-1")).toBe(original);
      expect(
        (
          screen.getAllByRole("button", {
            name: "Continue editing draft",
          })[0] as HTMLButtonElement
        ).disabled,
      ).toBe(true);
      view.rerender(content(false));
      expect(liveSessionTitleDraft(host.scope, "session-1")?.title).toBe(
        "Private unsent title",
      );
      await act(async () => finish());
      await waitFor(() =>
        expect(
          (
            screen.getByRole("button", {
              name: "Continue editing draft",
            }) as HTMLButtonElement
          ).disabled,
        ).toBe(false),
      );
      expect(host.saveMetadata).not.toHaveBeenCalled();
      fireEvent.click(
        screen.getByRole("button", { name: "Continue editing draft" }),
      );
      expect(input().readOnly).toBe(false);
      expect(input().value).toBe("Private unsent title");
      expect(liveSessionTitleDraft(host.scope, "session-1")?.revision).toBe(
        original?.revision,
      );
    } finally {
      stop();
    }
  });
  it("does not let a delayed reply clear newer typing while a second title field observes read-only", async () => {
    let finish!: () => void;
    host.saveMetadata.mockImplementationOnce(
      (_id, request) =>
        new Promise((done) => {
          finish = () => {
            const snapshot = sessionMetadata(
              request.patch.title ?? "",
              "session-1",
              "saved",
            );
            host.setMetadata(snapshot);
            done({ kind: "saved", snapshot });
          };
        }),
    );
    const content = (
      <host.Host>
        <TitleInput tab={tab} />
        <TitleInput tab={{ ...tab, slotId: "other-slot" }} />
      </host.Host>
    );
    render(content);
    await waitFor(() =>
      expect(
        (screen.getAllByRole("textbox")[0] as HTMLInputElement).readOnly,
      ).toBe(false),
    );
    const [first, second] = screen.getAllByRole(
      "textbox",
    ) as HTMLInputElement[];
    fireEvent.focus(first);
    fireEvent.change(first, { target: { value: "First request" } });
    fireEvent.blur(first);
    await waitFor(() => expect(host.saveMetadata).toHaveBeenCalledOnce());
    expect(second.readOnly).toBe(true);
    fireEvent.focus(first);
    fireEvent.change(first, { target: { value: "Newer owner draft" } });
    await act(async () => finish());
    expect(first.value).toBe("Newer owner draft");
    expect(second.value).toBe("Newer owner draft");
    expect(liveSessionTitleDraft(host.scope, "session-1")?.title).toBe(
      "Newer owner draft",
    );
    expect(host.saveMetadata).toHaveBeenCalledOnce();
  });
  it("retains a newer revert intention after an earlier title commits, including on unmount", async () => {
    let finish!: () => void;
    host.saveMetadata.mockImplementationOnce(
      (_id, request) =>
        new Promise((done) => {
          finish = () => {
            const snapshot = sessionMetadata(
              request.patch.title ?? "",
              "session-1",
              "saved",
            );
            host.setMetadata(snapshot);
            done({ kind: "saved", snapshot });
          };
        }),
    );
    const view = await renderTitleInput();
    type("Earlier request");
    fireEvent.blur(input());
    await waitFor(() => expect(host.saveMetadata).toHaveBeenCalledOnce());
    type("Untitled");
    await act(async () => finish());
    expect(input().value).toBe("Untitled");
    expect(
      screen.getByRole("button", { name: "Review saved title" }),
    ).toBeDefined();
    fireEvent.blur(input());
    view.unmount();
    expect(liveSessionTitleDraft(host.scope, "session-1")?.title).toBe(
      "Untitled",
    );
  });
  it.each([50, 150, 500])(
    "isolates a late %ims title reply across identical IDs and same-string replacement",
    async (delay) => {
      const old = host;
      let finish!: () => void;
      old.saveMetadata.mockImplementationOnce(
        (_id, request) =>
          new Promise((done) => {
            finish = () =>
              setTimeout(
                () =>
                  done({
                    kind: "saved",
                    snapshot: sessionMetadata(request.patch.title ?? ""),
                  }),
                delay,
              );
          }),
      );
      const view = await renderTitleInput();
      type("Old private draft");
      fireEvent.blur(input());
      await waitFor(() => expect(old.saveMetadata).toHaveBeenCalledOnce());
      const replacement = createSessionSourceHost("always", {
        scope: old.client.scope,
      });
      host = replacement;
      replacement.setMetadata(sessionMetadata("Replacement canonical"));
      old.scope.dispose();
      view.rerender(
        <replacement.Host>
          <StrictMode>
            <TitleInput tab={tab} />
          </StrictMode>
        </replacement.Host>,
      );
      await waitFor(() => expect(input().value).toBe("Replacement canonical"));
      type("Replacement typing");
      await act(async () => {
        finish();
        await new Promise((done) => setTimeout(done, delay + 20));
      });
      expect(input().value).toBe("Replacement typing");
      expect(liveSessionTitleDraft(replacement.scope, "session-1")?.title).toBe(
        "Replacement typing",
      );
      expect(replacement.saveMetadata).not.toHaveBeenCalled();
      old.dispose();
    },
  );
  it("preserves default local offline save and refuses remote offline, reader and unsupported writes", async () => {
    await renderTitleInput();
    act(() => onlineManager.setOnline(false));
    type("Offline local");
    fireEvent.blur(input());
    await waitFor(() => expect(host.saveMetadata).toHaveBeenCalledOnce());
    expect(host.readNoteEditor).not.toHaveBeenCalled();
    cleanup();
    host.dispose();
    onlineManager.setOnline(true);
    host = createSessionSourceHost("online");
    await renderTitleInput();
    act(() => onlineManager.setOnline(false));
    expect(input().readOnly).toBe(true);
    fireEvent.change(input(), { target: { value: "Forbidden" } });
    fireEvent.blur(input());
    expect(host.saveMetadata).not.toHaveBeenCalled();
    act(() => {
      onlineManager.setOnline(true);
      Object.assign(host.client, { canEdit: false });
      host.emit({ kind: "connection", state: "connected", error: null });
    });
    expect(input().readOnly).toBe(true);
    Object.assign(host.client, { canEdit: true });
    Object.assign(host.client, { features: ["content_read"] });
    act(() =>
      host.emit({ kind: "connection", state: "connected", error: null }),
    );
    expect(input().readOnly).toBe(true);
    expect(host.saveMetadata).not.toHaveBeenCalled();
  });
  it("keeps the typed input mounted during trusted legacy generation and ignores untrusted global status", async () => {
    await renderTitleInput();
    type("My title");
    expect(hoisted.generating).not.toHaveBeenCalled();
    cleanup();
    host.dispose();
    host = createSessionSourceHost("always", {
      features: ["content_read", "metadata_edit", "legacy_desktop_enhance"],
    });
    const view = await renderTitleInput();
    type("Local draft");
    hoisted.generating.mockReturnValue(true);
    view.rerender(
      <host.Host>
        <TitleInput tab={tab} />
      </host.Host>,
    );
    expect(input().value).toBe("Local draft");
    expect(screen.queryByText("Generating title...")).toBeNull();
  });
  it("preserves Enter splitting and transfers trailing text without a second blur write", async () => {
    const transfer = vi.fn();
    await renderTitleInput({ onTransferContentToEditor: transfer });
    type("BeforeAfter");
    input().setSelectionRange(6, 6);
    fireEvent.keyDown(input(), { key: "Enter" });
    fireEvent.blur(input());
    await waitFor(() => expect(transfer).toHaveBeenCalledWith("After"));
    await waitFor(() =>
      expect(host.saveMetadata).toHaveBeenCalledWith("session-1", {
        patch: { title: "Before" },
        expected_revision: "m0",
      }),
    );
    expect(host.saveMetadata).toHaveBeenCalledOnce();
  });
  it("refuses a title dispatch after permission is lost during pending body flush", async () => {
    let finish!: () => void;
    const stop = registerWriteFlusher(
      sessionEditorFlushKey(host.client.scope, "session-1"),
      () =>
        new Promise<void>((done) => {
          finish = done;
        }),
    );
    try {
      await renderTitleInput();
      type("Keep this private");
      fireEvent.blur(input());
      await waitFor(() => expect(finish).toBeDefined());
      Object.assign(host.client, { canEdit: false });
      await act(async () => finish());
      await waitFor(() => expect(input().readOnly).toBe(true));
      expect(host.saveMetadata).not.toHaveBeenCalled();
      expect(liveSessionTitleDraft(host.scope, "session-1")?.title).toBe(
        "Keep this private",
      );
    } finally {
      stop();
    }
  });
  it("guards delayed keyboard transfer after its original slot unmounts", async () => {
    const transfer = vi.fn();
    const view = await renderTitleInput({
      onTransferContentToEditor: transfer,
    });
    type("BeforeAfter");
    input().setSelectionRange(6, 6);
    fireEvent.keyDown(input(), { key: "Enter" });
    view.unmount();
    await act(async () => {
      await new Promise((done) => setTimeout(done, 5));
    });
    expect(transfer).not.toHaveBeenCalled();
    expect(host.saveMetadata).not.toHaveBeenCalled();
  });
});
