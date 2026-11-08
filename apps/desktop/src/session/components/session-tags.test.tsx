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
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("~/types/tauri.gen", () => ({ commands: {}, events: {} }));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: vi.fn() },
}));

import { SessionTags } from "./session-tags";

import { ApplicationRequestError } from "~/shared/application/errors";
import type {
  MetadataSnapshot,
  SaveMetadataRequest,
  TagSuggestionActionRequest,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import {
  VaultClientProvider,
  useVaultHost,
} from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

function snapshot(tags = ["existing"], revision = "r"): MetadataSnapshot {
  return {
    meta: {
      id: "same",
      title: "Title",
      created_at: "2026-11-08T00:00:00Z",
      started_at: null,
      ended_at: null,
      tags,
      tag_suggestions: { items: ["existing", "new/topic"], dismissed: [] },
    },
    revision,
  };
}
function fixture(networkMode: "always" | "online" = "always", scope = "scope") {
  let current = snapshot();
  let sequence = 0;
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const save = async (_id: string, input: SaveMetadataRequest) => {
    if (input.expected_revision !== current.revision)
      return { kind: "conflict" as const, snapshot: current };
    current = {
      ...current,
      meta: { ...current.meta, tags: input.patch.tags ?? current.meta.tags },
      revision: `r-${++sequence}`,
    };
    return { kind: "saved" as const, snapshot: current };
  };
  const action = async (
    _id: string,
    input: TagSuggestionActionRequest,
    accept: boolean,
  ) => {
    if (input.expected_revision !== current.revision)
      return { kind: "conflict" as const, snapshot: current };
    const state = current.meta.tag_suggestions!;
    current = {
      ...current,
      revision: `r-${++sequence}`,
      meta: {
        ...current.meta,
        tags: accept
          ? [...current.meta.tags, input.name].sort()
          : current.meta.tags,
        tag_suggestions: {
          items: state.items.filter((name) => name !== input.name),
          dismissed: accept
            ? state.dismissed
            : [...state.dismissed, input.name],
        },
      },
    };
    return { kind: "saved" as const, snapshot: current };
  };
  const client = {
    scope,
    networkMode,
    canEdit: true,
    features: ["tag_editing"],
    readMetadata: vi.fn(async () => current),
    saveMetadata: vi.fn(save),
    ensureTag: vi.fn(async ({ name }: { name: string }) => ({
      id: name,
      name,
    })),
    acceptTagSuggestion: vi.fn(
      (id: string, input: TagSuggestionActionRequest) =>
        action(id, input, true),
    ),
    dismissTagSuggestion: vi.fn(
      (id: string, input: TagSuggestionActionRequest) =>
        action(id, input, false),
    ),
    tagSuggestions: vi.fn(
      async ({
        query,
        excluded,
        limit,
      }: {
        query: string;
        excluded: string[];
        limit: number;
      }) =>
        ["new/topic", "registry"]
          .filter(
            (name) =>
              !excluded.includes(name) &&
              name.includes(query.replace(/^#/, "").trim().toLowerCase()),
          )
          .slice(0, limit),
    ),
    subscribe: vi.fn(async (listener: (event: VaultClientEvent) => void) => {
      listeners.add(listener);
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(listener);
      };
    }),
  };
  return {
    client,
    save,
    get: () => current,
    set: (value: MetadataSnapshot) => {
      current = value;
    },
    emit: (event: VaultClientEvent) => {
      for (const listener of listeners) listener(event);
    },
  };
}
function host(client: VaultClient, onScope?: (scope: VaultScope) => void) {
  function Probe() {
    const { scope } = useVaultHost();
    if (scope) onScope?.(scope);
    return null;
  }
  return (
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <VaultClientProvider client={client}>
          <Probe />
          <SessionTags sessionId="same" />
        </VaultClientProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}
function open(
  backend: ReturnType<typeof fixture>,
  onScope?: (scope: VaultScope) => void,
) {
  return render(host(backend.client as unknown as VaultClient, onScope));
}
async function ready() {
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Remove tag existing" }),
    ).toHaveProperty("disabled", false),
  );
}
async function add(name: string) {
  fireEvent.click(screen.getByRole("button", { name: "+" }));
  fireEvent.change(screen.getByPlaceholderText("Tag name"), {
    target: { value: name },
  });
  fireEvent.keyDown(screen.getByPlaceholderText("Tag name"), { key: "Enter" });
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  onlineManager.setOnline(true);
  vi.restoreAllMocks();
});

beforeEach(() =>
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  ),
);

it.each(["always", "online"] as const)(
  "accepts a suggestion with its captured revision in %s mode",
  async (mode) => {
    const backend = fixture(mode);
    open(backend);
    await ready();
    expect(
      screen.queryByRole("button", { name: "Accept suggested tag existing" }),
    ).toBeNull();
    expect(backend.client.acceptTagSuggestion).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Accept suggested tag new/topic" }),
    );
    await waitFor(() =>
      expect(backend.client.acceptTagSuggestion).toHaveBeenCalledWith("same", {
        name: "new/topic",
        expected_revision: "r",
      }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Remove tag new/topic" }),
      ).toHaveProperty("disabled", false),
    );
    expect(backend.client.saveMetadata).not.toHaveBeenCalled();
  },
);

it("dismisses through the same checked scope without applying the suggestion", async () => {
  const backend = fixture();
  open(backend);
  await ready();
  fireEvent.click(
    screen.getByRole("button", { name: "Dismiss suggested tag new/topic" }),
  );
  await waitFor(() =>
    expect(backend.client.dismissTagSuggestion).toHaveBeenCalledWith("same", {
      name: "new/topic",
      expected_revision: "r",
    }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Dismiss suggested tag new/topic" }),
    ).toBeNull(),
  );
  expect(backend.get().meta.tags).toEqual(["existing"]);
  expect(backend.client.acceptTagSuggestion).not.toHaveBeenCalled();
});

it.each(["always", "online"] as const)(
  "queries bounded suggestions, ensures and removes tags in %s mode",
  async (mode) => {
    const backend = fixture(mode);
    open(backend);
    await ready();
    fireEvent.click(screen.getByRole("button", { name: "+" }));
    fireEvent.change(screen.getByPlaceholderText("Tag name"), {
      target: { value: "# REGISTRY " },
    });
    fireEvent.click(await screen.findByRole("option", { name: "#registry" }));
    await waitFor(() =>
      expect(backend.client.ensureTag).toHaveBeenCalledWith({
        name: "registry",
      }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Remove tag registry" }),
      ).toHaveProperty("disabled", false),
    );
    expect(backend.client.tagSuggestions).toHaveBeenCalledWith(
      { query: "# REGISTRY ", excluded: ["existing"], limit: 8 },
      { signal: expect.any(AbortSignal) },
    );
    expect(backend.client.saveMetadata).toHaveBeenNthCalledWith(1, "same", {
      patch: { tags: ["existing", "registry"] },
      expected_revision: "r",
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Remove tag existing" }),
    );
    await waitFor(() => expect(backend.get().meta.tags).toEqual(["registry"]));
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Remove tag existing" }),
      ).toBeNull(),
    );
    expect(backend.client.ensureTag).toHaveBeenCalledTimes(1);
  },
);

it("shows an edit immediately while the captured save is pending and does not double-submit on blur", async () => {
  const backend = fixture();
  let release!: () => void;
  backend.client.saveMetadata.mockImplementationOnce(async (id, input) => {
    await new Promise<void>((done) => {
      release = done;
    });
    return backend.save(id, input);
  });
  open(backend);
  await ready();
  fireEvent.click(screen.getByRole("button", { name: "+" }));
  const input = screen.getByPlaceholderText("Tag name");
  fireEvent.change(input, { target: { value: "Fresh" } });
  fireEvent.keyDown(input, { key: "Enter" });
  fireEvent.blur(input);
  await waitFor(() =>
    expect(backend.client.saveMetadata).toHaveBeenCalledTimes(1),
  );
  expect(
    screen.getByRole("button", { name: "Remove tag fresh" }),
  ).toHaveProperty("disabled", true);
  expect(backend.client.ensureTag).not.toHaveBeenCalled();
  await act(async () => release());
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Remove tag fresh" }),
    ).toHaveProperty("disabled", false),
  );
  expect(backend.client.saveMetadata).toHaveBeenCalledTimes(1);
});

it("keeps the local client login-free and writable when browser networking is offline", async () => {
  onlineManager.setOnline(false);
  const backend = fixture("always");
  open(backend);
  await ready();
  await add("offline");
  await waitFor(() => expect(backend.get().meta.tags).toContain("offline"));
  expect(backend.client.ensureTag).toHaveBeenCalledWith({ name: "offline" });
});

it("retains confirmed chips through a cancelled refresh and accepts a canonical return to the original revision", async () => {
  const backend = fixture();
  open(backend);
  await ready();
  const original = backend.get();
  const reads = backend.client.readMetadata.mock.calls.length;
  let first!: (value: MetadataSnapshot) => void,
    second!: (value: MetadataSnapshot) => void;
  backend.client.readMetadata
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          first = done;
        }),
    )
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          second = done;
        }),
    );
  await add("confirmed");
  await waitFor(() =>
    expect(backend.client.readMetadata).toHaveBeenCalledTimes(reads + 1),
  );
  const saved = backend.get();
  await act(async () =>
    backend.emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "session_headers",
        ids: ["same"],
      },
    }),
  );
  await waitFor(() =>
    expect(backend.client.readMetadata).toHaveBeenCalledTimes(reads + 2),
  );
  await act(async () => {
    await new Promise((done) => setTimeout(done, 20));
  });
  expect(
    screen.getByRole("button", { name: "Remove tag confirmed" }),
  ).toHaveProperty("disabled", true);
  backend.set(original);
  await act(async () => {
    second(original);
    first(saved);
  });
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Remove tag existing" }),
    ).toHaveProperty("disabled", false),
  );
  expect(
    screen.queryByRole("button", { name: "Remove tag confirmed" }),
  ).toBeNull();
  expect(backend.client.saveMetadata).toHaveBeenCalledTimes(1);
});

it("keeps typing responsive and preserves the field during a background metadata refresh", async () => {
  const backend = fixture();
  open(backend);
  await ready();
  fireEvent.click(screen.getByRole("button", { name: "+" }));
  const input = screen.getByPlaceholderText("Tag name");
  fireEvent.change(input, { target: { value: "draft" } });
  let resolve!: (value: MetadataSnapshot) => void;
  const readsBeforeRefresh = backend.client.readMetadata.mock.calls.length;
  backend.client.readMetadata.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  await act(async () =>
    backend.emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "session_headers",
        ids: ["same"],
      },
    }),
  );
  await waitFor(() =>
    expect(backend.client.readMetadata).toHaveBeenCalledTimes(
      readsBeforeRefresh + 1,
    ),
  );
  expect(input).toHaveProperty("disabled", false);
  fireEvent.change(input, { target: { value: "draft-still-here" } });
  backend.set(snapshot(["existing"], "newer"));
  await act(async () => resolve(backend.get()));
  expect(input).toHaveProperty("value", "draft-still-here");
  fireEvent.keyDown(input, { key: "Escape" });
  expect(backend.client.saveMetadata).not.toHaveBeenCalled();
});

it.each(["reader", "unsupported", "disconnected"])(
  "keeps %s tags read-only without falling back to native commands",
  async (kind) => {
    const backend = fixture();
    if (kind === "reader") backend.client.canEdit = false;
    if (kind === "unsupported") backend.client.features = [];
    open(backend);
    await screen.findByRole("button", { name: "Remove tag existing" });
    if (kind === "disconnected")
      await act(async () =>
        backend.emit({
          kind: "connection",
          state: "reconnecting",
          error: null,
        }),
      );
    expect(
      screen.getByRole("button", { name: "Remove tag existing" }),
    ).toHaveProperty("disabled", true);
    fireEvent.click(
      screen.getByRole("button", { name: "Accept suggested tag new/topic" }),
    );
    expect(backend.client.saveMetadata).not.toHaveBeenCalled();
    expect(backend.client.acceptTagSuggestion).not.toHaveBeenCalled();
    expect(backend.client.ensureTag).not.toHaveBeenCalled();
  },
);

it.each(["conflict", "unknown"])(
  "requires a canonical read after %s without replaying the mutation",
  async (kind) => {
    const backend = fixture();
    backend.client.saveMetadata.mockImplementationOnce(async () => {
      const latest = snapshot(["newer"], "newer");
      backend.set(latest);
      if (kind === "unknown")
        throw new ApplicationRequestError({
          code: "unavailable",
          message: "Lost reply",
          request_id: null,
          retryable: true,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      return { kind: "conflict", snapshot: latest };
    });
    open(backend);
    await ready();
    await add("requested");
    await screen.findByRole("alert");
    expect(backend.client.saveMetadata).toHaveBeenCalledTimes(1);
    expect(backend.client.ensureTag).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Reload tags" }),
      ).toHaveProperty("disabled", false),
    );
    fireEvent.click(screen.getByRole("button", { name: "Reload tags" }));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Remove tag newer" }),
      ).toHaveProperty("disabled", false),
    );
    expect(backend.client.saveMetadata).toHaveBeenCalledTimes(1);
    expect(
      screen.queryByRole("button", { name: "Remove tag requested" }),
    ).toBeNull();
  },
);

it("uses canonical metadata when a write reply arrives after a newer header event", async () => {
  const backend = fixture();
  let resolve!: (
    value: Awaited<ReturnType<typeof backend.client.acceptTagSuggestion>>,
  ) => void;
  backend.client.acceptTagSuggestion.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  open(backend);
  await ready();
  fireEvent.click(
    screen.getByRole("button", { name: "Accept suggested tag new/topic" }),
  );
  await waitFor(() =>
    expect(backend.client.acceptTagSuggestion).toHaveBeenCalled(),
  );
  backend.set(snapshot(["newer"], "newer"));
  await act(async () =>
    backend.emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "session_headers",
        ids: ["same"],
      },
    }),
  );
  await screen.findByRole("button", { name: "Remove tag newer" });
  await act(async () =>
    resolve({ kind: "saved", snapshot: snapshot(["old"], "old") }),
  );
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Remove tag newer" }),
    ).toHaveProperty("disabled", false),
  );
  expect(screen.queryByRole("button", { name: "Remove tag old" })).toBeNull();
});

it("isolates identical IDs and scope strings during a replacement with an old pending write", async () => {
  const first = fixture("always", "identical"),
    second = fixture("online", "identical");
  second.set(snapshot(["replacement"], "replacement"));
  let release!: (
    value: Awaited<ReturnType<typeof first.client.saveMetadata>>,
  ) => void;
  first.client.saveMetadata.mockImplementationOnce(
    () =>
      new Promise((done) => {
        release = done;
      }),
  );
  let original: VaultScope | undefined;
  const rendered = open(first, (scope) => {
    original = scope;
  });
  await ready();
  await add("old-private");
  await waitFor(() => expect(first.client.saveMetadata).toHaveBeenCalled());
  const retired = original!;
  rendered.rerender(host(second.client as unknown as VaultClient));
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Remove tag replacement" }),
    ).toHaveProperty("disabled", false),
  );
  await act(async () =>
    release({ kind: "saved", snapshot: snapshot(["old-private"], "old") }),
  );
  expect(
    screen.queryByRole("button", { name: "Remove tag old-private" }),
  ).toBeNull();
  expect(first.client.ensureTag).not.toHaveBeenCalled();
  expect(second.client.saveMetadata).not.toHaveBeenCalled();
  expect(retired.queryClient.getQueryCache().getAll()).toEqual([]);
});

it("removes private tags and late suggestion replies on access loss", async () => {
  const backend = fixture();
  let resolve!: (value: string[]) => void;
  backend.client.tagSuggestions.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  let current: VaultScope | undefined;
  open(backend, (scope) => {
    current = scope;
  });
  await ready();
  fireEvent.click(screen.getByRole("button", { name: "+" }));
  await waitFor(() => expect(backend.client.tagSuggestions).toHaveBeenCalled());
  await act(async () =>
    backend.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await act(async () => resolve(["private-name"]));
  expect(
    screen.queryByRole("button", { name: "Remove tag existing" }),
  ).toBeNull();
  expect(screen.queryByText("#private-name")).toBeNull();
  expect(current!.queryClient.getQueryCache().getAll()).toEqual([]);
});
