import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { StrictMode, useEffect, useState } from "react";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  applicationScope: vi.fn(),
  applicationProcessingAvailable: vi.fn(),
  applicationSearch: vi.fn(),
  listen: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: native,
  events: { applicationChanged: { listen: native.listen } },
}));
vi.mock("~/sidebar/timeline/queries", () => ({
  useTimelineSessionsTable: () => ({
    "same-session": { title: "Recent local note" },
  }),
}));

import { SearchEngineProvider, useSearchEngine } from "./index";

import { useMentionConfig } from "~/editor-bridge/mention-config";
import type { NativeRemoteConnection } from "~/shared/application/remote";
import {
  connectLocalVault,
  connectRemoteVault,
} from "~/shared/application/vault-client";
import type { VaultClientEvent } from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";
import type { SearchResult } from "~/types/tauri.gen";

const scopes: VaultScope[] = [];
const empty: SearchResult = { hits: [], count: 0 };
const data: SearchResult = {
  count: 1,
  hits: [
    {
      score: 2.5,
      document: {
        id: "same-session",
        doc_type: "session",
        language: null,
        title: "Planning",
        content: "zebra sighting",
        created_at: 1234,
      },
      title_snippet: null,
      content_snippet: {
        fragment: "zebra sighting",
        highlights: [{ start: 0, end: 5 }],
      },
    },
  ],
};

async function fixture(mode: "local" | "remote" = "remote", id = "vault") {
  const search = vi.fn(async (..._args: unknown[]) => empty);
  const stopped = vi.fn();
  let receive!: (event: VaultClientEvent) => void;
  const connection = {
    info: {
      server: "https://notes.company.example",
      capabilities: { features: ["search"] },
    },
    request: vi.fn(
      async (operation: { input: unknown }, _reply: string, options: unknown) =>
        search(operation.input, options),
    ),
    subscribe: vi.fn(
      async (_vault: string, listener: (event: VaultClientEvent) => void) => {
        receive = listener;
        listener({ kind: "connection", state: "connected", error: null });
        return stopped;
      },
    ),
  };
  const client =
    mode === "local"
      ? await connectLocalVault()
      : connectRemoteVault(
          connection as unknown as NativeRemoteConnection,
          "member",
          { id, role: "editor" },
        );
  if (mode === "local")
    native.applicationSearch.mockImplementation(async (input) => ({
      status: "ok",
      data: await search(input),
    }));
  const scope = new VaultScope(client);
  scopes.push(scope);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: client.networkMode },
  });
  await scope.start();
  return {
    scope,
    client,
    search,
    stopped,
    connection,
    emit: (event: VaultClientEvent) => receive(event),
  };
}

function renderEngine(initial: VaultScope | null) {
  let scope = initial;
  const mounted = renderHook(() => useSearchEngine(), {
    wrapper: ({ children }) => (
      <VaultHostContext.Provider
        value={{ scope, error: null, retry: () => {} }}
      >
        <SearchEngineProvider>{children}</SearchEngineProvider>
      </VaultHostContext.Provider>
    ),
  });
  return {
    ...mounted,
    switchScope: (next: VaultScope) => {
      scope = next;
      mounted.rerender();
    },
  };
}

beforeEach(() => {
  vi.resetAllMocks();
  native.applicationScope.mockResolvedValue({
    status: "ok",
    data: "local:fixture",
  });
  native.applicationProcessingAvailable.mockResolvedValue(false);
  native.listen.mockResolvedValue(vi.fn());
});
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  onlineManager.setOnline(true);
});

describe("scoped main search", () => {
  it.each(["local", "remote"] as const)(
    "normalizes queries and preserves date filters, limits and snippets through the %s adapter",
    async (mode) => {
      const { scope, search, connection } = await fixture(mode);
      search.mockResolvedValue(data);
      const { result } = renderEngine(scope);
      expect(result.current.ready).toBe(true);
      const hits = await result.current.search(
        "  hello   world  ",
        { created_at: { gte: 123, lt: 456 } },
        { limit: 20, snippets: true, snippetMaxChars: 120 },
      );
      const input = {
        query: "hello world",
        filters: {
          created_at: { gte: 123, lte: null, gt: null, lt: 456, eq: null },
          doc_type: null,
          facet: null,
        },
        limit: 20,
        options: {
          fuzzy: null,
          distance: null,
          phrase_slop: null,
          snippets: true,
          snippet_max_chars: 120,
        },
      };
      expect(hits).toEqual([
        {
          score: 2.5,
          document: {
            id: "same-session",
            type: "session",
            title: "Planning",
            content: "zebra sighting",
            created_at: 1234,
          },
          titleSnippet: null,
          contentSnippet: data.hits[0].content_snippet,
        },
      ]);
      if (mode === "local")
        expect(native.applicationSearch).toHaveBeenCalledWith(input);
      else
        expect(connection.request).toHaveBeenCalledWith(
          { kind: "search", vault_id: "vault", input },
          "search",
          { signal: expect.any(AbortSignal) },
        );
    },
  );

  it("uses local search while browser networking is offline without login", async () => {
    const { scope, search } = await fixture("local");
    onlineManager.setOnline(false);
    const { result } = renderEngine(scope);
    await expect(result.current.search("zebra")).resolves.toEqual([]);
    expect(search).toHaveBeenCalledOnce();
    expect(native.applicationSearch).toHaveBeenCalledWith({
      query: "zebra",
      filters: undefined,
    });
  });

  it("does not dispatch before a vault host is ready", async () => {
    const { result } = renderEngine(null);
    expect(result.current.ready).toBe(false);
    await expect(result.current.search("zebra")).resolves.toEqual([]);
    expect(native.applicationSearch).not.toHaveBeenCalled();
  });

  it("keeps mount-driven searches usable through StrictMode effect replay", async () => {
    const { scope, search } = await fixture();
    search.mockResolvedValue(data);
    const { result } = renderHook(
      () => {
        const { search: runSearch } = useSearchEngine();
        const [title, setTitle] = useState("");
        useEffect(() => {
          void runSearch("zebra").then((hits) =>
            setTitle(hits[0]?.document.title ?? ""),
          );
        }, [runSearch]);
        return title;
      },
      {
        wrapper: ({ children }) => (
          <StrictMode>
            <VaultHostContext.Provider
              value={{ scope, error: null, retry: () => {} }}
            >
              <SearchEngineProvider>{children}</SearchEngineProvider>
            </VaultHostContext.Provider>
          </StrictMode>
        ),
      },
    );
    await waitFor(() => expect(result.current).toBe("Planning"));
  });

  it("never redirects a delayed search or its old callback to a replacement vault", async () => {
    const first = await fixture("remote", "first");
    const second = await fixture("remote", "second");
    let finish!: (value: SearchResult) => void;
    first.search.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const view = renderEngine(first.scope);
    const retiredSearch = view.result.current.search;
    const pending = retiredSearch("zebra");
    view.switchScope(second.scope);
    finish(data);
    await expect(pending).resolves.toEqual([]);
    await expect(retiredSearch("zebra")).resolves.toEqual([]);
    second.search.mockResolvedValue({
      ...data,
      hits: [
        {
          ...data.hits[0],
          document: { ...data.hits[0].document, title: "Second vault" },
        },
      ],
    });
    const current = await view.result.current.search("zebra");
    expect(current[0].document.title).toBe("Second vault");
    expect(first.search).toHaveBeenCalledOnce();
    expect(second.search).toHaveBeenCalledOnce();
  });

  it.each([
    "reconnecting",
    "access_denied",
    "login_required",
    "closed",
  ] as const)(
    "cancels and discards late private replies when the connection becomes %s",
    async (state) => {
      const { scope, search, emit } = await fixture();
      let finish!: (value: SearchResult) => void;
      search.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      );
      const { result } = renderEngine(scope);
      const pending = result.current.search("zebra");
      const signal = (search.mock.calls[0][1] as { signal: AbortSignal })
        .signal;
      act(() => emit({ kind: "connection", state, error: null }));
      expect(result.current.ready).toBe(false);
      expect(signal.aborted).toBe(true);
      finish(data);
      await expect(pending).resolves.toEqual([]);
      await result.current.search("zebra");
      expect(search).toHaveBeenCalledOnce();
    },
  );

  it("cancels only its owned requests on unmount and honors caller cancellation", async () => {
    const { scope, search } = await fixture();
    let finish!: (value: SearchResult) => void;
    search.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const { result, unmount } = renderEngine(scope);
    const active = result.current.search;
    const caller = new AbortController();
    const pending = active("zebra", null, {}, caller.signal);
    const signal = (search.mock.calls[0][1] as { signal: AbortSignal }).signal;
    caller.abort();
    expect(signal.aborted).toBe(true);
    unmount();
    finish(data);
    await expect(pending).resolves.toEqual([]);
    await active("zebra");
    expect(search).toHaveBeenCalledOnce();
    expect(scope.getSnapshot().state).toBe("connected");
  });

  it("preserves the empty-result behavior for a failed request", async () => {
    const { scope, search } = await fixture();
    search.mockRejectedValueOnce(new Error("Search unavailable"));
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const { result } = renderEngine(scope);
    await expect(result.current.search("zebra")).resolves.toEqual([]);
    expect(log).toHaveBeenCalledOnce();
    log.mockRestore();
  });

  it("routes nonempty editor mentions through the captured client and discards replies after access loss", async () => {
    const { scope, search, emit } = await fixture();
    search.mockResolvedValueOnce(data);
    const { result } = renderHook(() => useMentionConfig(), {
      wrapper: ({ children }) => (
        <VaultHostContext.Provider
          value={{ scope, error: null, retry: () => {} }}
        >
          <SearchEngineProvider>{children}</SearchEngineProvider>
        </VaultHostContext.Provider>
      ),
    });
    await expect(result.current.handleSearch("zebra")).resolves.toEqual([
      { id: "same-session", type: "session", label: "Planning" },
    ]);
    let finish!: (value: SearchResult) => void;
    search.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const pending = result.current.handleSearch("zebra");
    act(() =>
      emit({ kind: "connection", state: "access_denied", error: null }),
    );
    finish(data);
    await expect(pending).resolves.toEqual([]);
  });
});
