import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { SearchHit } from "~/search/contexts/engine";

const mocks = vi.hoisted(() => ({
  engineSearch: vi.fn(),
  openCurrent: vi.fn(),
  onOpenChange: vi.fn(),
  sessions: [] as { id: string; title: string; created_at: string }[],
  recentlyOpenedSessionIds: [] as string[],
  scope: null as null | {
    client: { scope: string };
    queryClient: QueryClient;
    getSnapshot: () => { state: string };
  },
  ready: true,
}));

vi.mock("~/search/contexts/engine", () => ({
  useSearchEngine: () => ({
    search: mocks.engineSearch,
    scope: mocks.scope,
    ready: mocks.ready,
  }),
}));

vi.mock("~/session/queries", () => ({
  useSessionSummaries: () => mocks.sessions,
}));

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (
    selector: (state: {
      openCurrent: typeof mocks.openCurrent;
      recentlyOpenedSessionIds: string[];
    }) => unknown,
  ) =>
    selector({
      openCurrent: mocks.openCurrent,
      recentlyOpenedSessionIds: mocks.recentlyOpenedSessionIds,
    }),
}));

import { OpenNoteDialog } from "./open-note-dialog";

function sessionHit(
  id: string,
  title: string,
  fragment: string,
  highlights: { start: number; end: number }[] = [],
  type = "session",
): SearchHit {
  return {
    score: 1,
    document: {
      id,
      type: type as never,
      title,
      content: fragment,
      created_at: 0,
    },
    titleSnippet: null,
    contentSnippet: { fragment, highlights },
  };
}

function renderDialog() {
  const queryClient = mocks.scope!.queryClient;
  return render(
    <QueryClientProvider client={queryClient}>
      <OpenNoteDialog open onOpenChange={mocks.onOpenChange} />
    </QueryClientProvider>,
  );
}

async function typeQuery(value: string) {
  fireEvent.change(screen.getByPlaceholderText("Find a note..."), {
    target: { value },
  });
}

describe("OpenNoteDialog", () => {
  beforeEach(() => {
    mocks.scope = {
      client: { scope: "local:fixture" },
      queryClient: new QueryClient({
        defaultOptions: { queries: { retry: false, networkMode: "always" } },
      }),
      getSnapshot: () => ({ state: "connected" }),
    };
    mocks.ready = true;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
    window.HTMLElement.prototype.scrollIntoView = vi.fn();
    mocks.engineSearch.mockReset().mockResolvedValue([]);
    mocks.openCurrent.mockReset();
    mocks.onOpenChange.mockReset();
    mocks.sessions = [
      {
        id: "s1",
        title: "Planning session",
        created_at: "2026-01-02T00:00:00Z",
      },
      { id: "s2", title: "Weekly sync", created_at: "2026-01-01T00:00:00Z" },
    ];
    mocks.recentlyOpenedSessionIds = ["s2"];
  });

  afterEach(() => {
    cleanup();
    mocks.scope?.queryClient.clear();
    onlineManager.setOnline(true);
  });

  it("shows recent and all notes when the query is empty", () => {
    renderDialog();

    expect(screen.getByText("Recent")).toBeTruthy();
    expect(screen.getByText("All Notes")).toBeTruthy();
    expect(screen.getByText("Weekly sync")).toBeTruthy();
    expect(screen.getByText("Planning session")).toBeTruthy();
    expect(mocks.engineSearch).not.toHaveBeenCalled();
  });

  it("opens a session note from All Notes", () => {
    renderDialog();

    fireEvent.click(screen.getByRole("option", { name: "Planning session" }));

    expect(mocks.onOpenChange).toHaveBeenCalledWith(false);
    expect(mocks.openCurrent).toHaveBeenCalledWith({
      type: "sessions",
      id: "s1",
    });
  });

  it("runs a full-text search with snippets for the typed query", async () => {
    renderDialog();
    await typeQuery("zebra");

    expect(mocks.engineSearch).toHaveBeenCalledWith(
      "zebra",
      null,
      {
        limit: 20,
        snippets: true,
        snippetMaxChars: 120,
      },
      expect.any(AbortSignal),
    );
  });

  it("shows sessions matched only by content, with a highlighted snippet", async () => {
    mocks.engineSearch.mockResolvedValue([
      sessionHit("s2", "Weekly sync", "spotted a zebra today", [
        { start: 10, end: 15 },
      ]),
    ]);

    renderDialog();
    await typeQuery("zebra");

    expect(await screen.findByText("Weekly sync")).toBeTruthy();
    expect(screen.queryByText("Planning session")).toBeNull();

    const mark = await screen.findByText("zebra");
    expect(mark.tagName).toBe("MARK");
    expect(screen.getByText("spotted a")).toBeTruthy();
  });

  it("keeps title substring matches even when the engine returns nothing", async () => {
    mocks.engineSearch.mockResolvedValue([]);

    renderDialog();
    await typeQuery("plan");

    expect(await screen.findByText("Planning session")).toBeTruthy();
    expect(screen.queryByText("Weekly sync")).toBeNull();
    expect(screen.queryByText("Recent")).toBeNull();
  });

  it("dedupes sessions matching both title and content, keeping the snippet", async () => {
    mocks.engineSearch.mockResolvedValue([
      sessionHit("s2", "Weekly sync", "sync notes from standup", [
        { start: 0, end: 4 },
      ]),
    ]);

    renderDialog();
    await typeQuery("sync");

    expect(await screen.findByText("notes from standup")).toBeTruthy();
    expect(screen.getAllByText("Weekly sync")).toHaveLength(1);
  });

  it("ignores hits for unknown sessions and non-session documents", async () => {
    mocks.engineSearch.mockResolvedValue([
      sessionHit("ghost", "Deleted note", "zebra"),
      sessionHit("h1", "Some Human", "zebra", [], "human"),
    ]);

    renderDialog();
    await typeQuery("zebra");

    expect(await screen.findByText("No notes found.")).toBeTruthy();
    expect(screen.queryByText("Deleted note")).toBeNull();
    expect(screen.queryByText("Some Human")).toBeNull();
  });

  it("opens the selected session and closes the dialog", async () => {
    mocks.engineSearch.mockResolvedValue([
      sessionHit("s2", "Weekly sync", "spotted a zebra today"),
    ]);

    renderDialog();
    await typeQuery("zebra");

    fireEvent.click(await screen.findByText("Weekly sync"));

    expect(mocks.openCurrent).toHaveBeenCalledWith({
      type: "sessions",
      id: "s2",
    });
    expect(mocks.onOpenChange).toHaveBeenCalledWith(false);
  });

  it("shows the empty state when nothing matches", async () => {
    mocks.engineSearch.mockResolvedValue([]);

    renderDialog();
    await typeQuery("xyzzy");

    expect(await screen.findByText("No notes found.")).toBeTruthy();
  });

  it("caps the empty-query All Notes list to the most recent entries", () => {
    mocks.sessions = Array.from({ length: 60 }, (_, index) => ({
      id: `s-${index}`,
      title: `Note ${index}`,
      created_at: new Date(Date.UTC(2026, 0, 1, 0, index)).toISOString(),
    }));

    renderDialog();

    // Newest-first: the 50 most recent survive the cap, the 10 oldest don't.
    expect(screen.getByText("Note 59")).toBeTruthy();
    expect(screen.getByText("Note 10")).toBeTruthy();
    expect(screen.queryByText("Note 9")).toBeNull();
  });

  it("uses the local scope query client while browser networking is offline", async () => {
    onlineManager.setOnline(false);
    renderDialog();
    await typeQuery("zebra");
    await waitFor(() => expect(mocks.engineSearch).toHaveBeenCalledOnce());
  });

  it("refreshes mounted search hits when the active scope invalidates its search projection", async () => {
    mocks.engineSearch.mockResolvedValueOnce([
      sessionHit("s2", "Weekly sync", "OLD PROJECTED CONTENT"),
    ]);
    renderDialog();
    await typeQuery("zebra");
    await screen.findByText("OLD PROJECTED CONTENT");
    mocks.engineSearch.mockResolvedValueOnce([
      sessionHit("s2", "Weekly sync", "UPDATED PROJECTED CONTENT"),
    ]);
    await act(async () => {
      await mocks.scope!.queryClient.invalidateQueries({
        queryKey: ["vault", mocks.scope!.client.scope, "search"],
      });
    });
    await screen.findByText("UPDATED PROJECTED CONTENT");
    expect(screen.queryByText("OLD PROJECTED CONTENT")).toBeNull();
  });

  it("does not dispatch searches while disconnected and hides private rows after access loss", async () => {
    const view = renderDialog();
    mocks.ready = false;
    mocks.scope!.getSnapshot = () => ({ state: "reconnecting" });
    view.rerender(<OpenNoteDialog open onOpenChange={mocks.onOpenChange} />);
    await typeQuery("zebra");
    expect(mocks.engineSearch).not.toHaveBeenCalled();
    mocks.scope!.getSnapshot = () => ({ state: "access_denied" });
    view.rerender(<OpenNoteDialog open onOpenChange={mocks.onOpenChange} />);
    expect(screen.queryByPlaceholderText("Find a note...")).toBeNull();
    expect(screen.queryByText("Planning session")).toBeNull();
  });

  it("never reuses results or previous-data placeholders for an identical ID in a different scope", async () => {
    mocks.engineSearch.mockResolvedValue([
      sessionHit("s2", "Weekly sync", "FIRST VAULT PRIVATE SNIPPET"),
    ]);
    const view = renderDialog();
    await typeQuery("zebra");
    await screen.findByText("FIRST VAULT PRIVATE SNIPPET");
    const first = mocks.scope!;
    mocks.scope = {
      client: { scope: "remote:second" },
      queryClient: new QueryClient({
        defaultOptions: { queries: { retry: false, networkMode: "always" } },
      }),
      getSnapshot: () => ({ state: "connected" }),
    };
    mocks.sessions = [
      {
        id: "s2",
        title: "Second vault note",
        created_at: "2026-01-02T00:00:00Z",
      },
    ];
    let finish!: (hits: SearchHit[]) => void;
    mocks.engineSearch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    view.rerender(<OpenNoteDialog open onOpenChange={mocks.onOpenChange} />);
    expect(screen.queryByText("FIRST VAULT PRIVATE SNIPPET")).toBeNull();
    expect(screen.getByPlaceholderText("Find a note...")).toHaveProperty(
      "value",
      "",
    );
    await typeQuery("zebra");
    await waitFor(() => expect(mocks.engineSearch).toHaveBeenCalledTimes(2));
    expect(screen.queryByText("FIRST VAULT PRIVATE SNIPPET")).toBeNull();
    await act(async () =>
      finish([sessionHit("s2", "Second vault note", "SECOND VAULT SNIPPET")]),
    );
    await screen.findByText("SECOND VAULT SNIPPET");
    expect(screen.queryByText("FIRST VAULT PRIVATE SNIPPET")).toBeNull();
    first.queryClient.clear();
  });

  it("ignores the first scope's delayed reply after switching vaults", async () => {
    let finish!: (hits: SearchHit[]) => void;
    mocks.engineSearch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const view = renderDialog();
    await typeQuery("zebra");
    await waitFor(() => expect(mocks.engineSearch).toHaveBeenCalledOnce());
    const first = mocks.scope!;
    mocks.scope = {
      client: { scope: "remote:second" },
      queryClient: new QueryClient({
        defaultOptions: { queries: { retry: false, networkMode: "always" } },
      }),
      getSnapshot: () => ({ state: "connected" }),
    };
    mocks.sessions = [
      {
        id: "s2",
        title: "Second vault note",
        created_at: "2026-01-02T00:00:00Z",
      },
    ];
    mocks.engineSearch.mockResolvedValueOnce([
      sessionHit("s2", "Second vault note", "SECOND VAULT SNIPPET"),
    ]);
    view.rerender(<OpenNoteDialog open onOpenChange={mocks.onOpenChange} />);
    await typeQuery("zebra");
    await screen.findByText("SECOND VAULT SNIPPET");
    await act(async () =>
      finish([sessionHit("s2", "Weekly sync", "LATE PRIVATE FIRST VAULT")]),
    );
    expect(screen.queryByText("LATE PRIVATE FIRST VAULT")).toBeNull();
    expect(screen.getByText("SECOND VAULT SNIPPET")).toBeTruthy();
    first.queryClient.clear();
  });
});
