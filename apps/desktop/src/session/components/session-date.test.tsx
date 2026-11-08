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
import { StrictMode, useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { format } from "@hypr/utils";

const native = vi.hoisted(() => ({
  legacyRead: vi.fn(),
  legacyWrite: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionGet: native.legacyRead,
    sessionUpdateMeta: native.legacyWrite,
  },
  events: {},
}));

import { SessionDate } from "./session-date";

import { ApplicationRequestError } from "~/shared/application/errors";
import type {
  MetadataSnapshot,
  MetadataSaveOutcome,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import {
  VaultClientProvider,
  VaultHostContext,
  useVaultHost,
} from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";

const ORIGINAL = "2026-07-02T03:53:00.000Z";
const PICKED = "2026-08-01T10:30";
const pickedIso = () => new Date(PICKED).toISOString();
const dateLabel = (value: string) =>
  format(new Date(value), "MMM d, yyyy h:mm a");
const finishes: (() => void)[] = [];
function deferred<T>(fallback: T) {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  finishes.push(() => resolve(fallback));
  return { promise, resolve };
}
function snapshot(
  createdAt = ORIGINAL,
  revision = "r0",
  id = "same",
): MetadataSnapshot {
  return {
    meta: {
      id,
      title: "Keep this title",
      created_at: createdAt,
      started_at: null,
      ended_at: null,
      tags: ["keep"],
      author: "writer",
    },
    revision,
  };
}
function fixture(
  mode: "always" | "online" = "always",
  options: { canEdit?: boolean; features?: string[]; scope?: string } = {},
) {
  let current = snapshot();
  let sequence = 0;
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const read = vi.fn<VaultClient["readMetadata"]>(async () =>
    structuredClone(current),
  );
  const save = vi.fn<VaultClient["saveMetadata"]>(async (_id, input) => {
    if (input.expected_revision !== current.revision)
      return { kind: "conflict", snapshot: structuredClone(current) };
    current = {
      meta: {
        ...current.meta,
        created_at: input.patch.created_at ?? current.meta.created_at,
      },
      revision: `saved-${++sequence}`,
    };
    return { kind: "saved", snapshot: structuredClone(current) };
  });
  const client = {
    scope: options.scope ?? crypto.randomUUID(),
    networkMode: mode,
    canEdit: options.canEdit ?? true,
    features: options.features ?? ["metadata_edit"],
    readMetadata: read,
    saveMetadata: save,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      listeners.add(listener);
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(listener);
      };
    },
  } as unknown as VaultClient;
  return {
    client,
    read,
    save,
    scopes: [] as VaultScope[],
    set: (next: MetadataSnapshot) => {
      current = next;
    },
    get: () => structuredClone(current),
    emit: (event: VaultClientEvent) =>
      act(() => {
        for (const listener of listeners) listener(event);
      }),
  };
}
function ScopeCapture({
  backend,
  children,
}: {
  backend: ReturnType<typeof fixture>;
  children: ReactNode;
}) {
  const { scope } = useVaultHost();
  if (scope && !backend.scopes.includes(scope)) backend.scopes.push(scope);
  return children;
}
function Host({
  backend,
  id = "same",
}: {
  backend: ReturnType<typeof fixture>;
  id?: string;
}) {
  const [rootClient] = useState(() => new QueryClient());
  return (
    <StrictMode>
      <QueryClientProvider client={rootClient}>
        <VaultClientProvider client={backend.client}>
          <ScopeCapture backend={backend}>
            <SessionDate sessionId={id} />
          </ScopeCapture>
        </VaultClientProvider>
      </QueryClientProvider>
    </StrictMode>
  );
}
async function edit() {
  const button = await screen.findByRole("button", { name: "Edit date" });
  await waitFor(() => expect(button).toHaveProperty("disabled", false));
  fireEvent.click(button);
  return screen.getByLabelText<HTMLInputElement>("Note date and time");
}
function pick(input: HTMLInputElement, value = PICKED) {
  fireEvent.change(input, { target: { value } });
}
function clickSave() {
  fireEvent.click(screen.getByRole("button", { name: "Save date" }));
}
function failure(outcome: "unknown" | "not_committed" = "unknown") {
  return new ApplicationRequestError({
    code: "unavailable",
    message: "Reply lost",
    request_id: "request",
    retryable: true,
    current_revision: null,
    mutation_outcome: outcome,
  });
}
beforeEach(() => vi.clearAllMocks());
afterEach(async () => {
  cleanup();
  for (const finish of finishes.splice(0)) finish();
  onlineManager.setOnline(true);
  await Promise.resolve();
});

describe("scoped SessionDate", () => {
  it("opens from already-cached metadata without requiring another read to enable editing", async () => {
    const backend = fixture();
    const scope = new VaultScope(backend.client);
    await scope.start();
    scope.queryClient.setQueryData(
      scope.metadataOptions("same").queryKey,
      backend.get(),
    );
    const mounted = render(
      <StrictMode>
        <VaultHostContext.Provider value={{ scope, error: null, retry() {} }}>
          <SessionDate sessionId="same" />
        </VaultHostContext.Provider>
      </StrictMode>,
    );
    try {
      pick(await edit());
      expect(backend.read).not.toHaveBeenCalled();
      clickSave();
      await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "Edit date" }).textContent,
        ).toBe(dateLabel(pickedIso())),
      );
    } finally {
      mounted.unmount();
      scope.dispose();
    }
  });

  it("opens the inline picker with the captured canonical date", async () => {
    const backend = fixture();
    render(<Host backend={backend} />);
    const input = await edit();
    expect(input.value).toBe(format(new Date(ORIGINAL), "yyyy-MM-dd'T'HH:mm"));
    expect(
      screen.getByRole("button", { name: "Cancel date edit" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Save date" })).toBeTruthy();
    expect(native.legacyRead).not.toHaveBeenCalled();
  });

  it.each(["always", "online"] as const)(
    "saves only the picked date through the captured %s client and revision",
    async (mode) => {
      const backend = fixture(mode);
      render(<Host backend={backend} />);
      pick(await edit());
      clickSave();
      await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
      expect(backend.save).toHaveBeenCalledWith("same", {
        patch: { created_at: pickedIso() },
        expected_revision: "r0",
      });
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "Edit date" }).textContent,
        ).toBe(dateLabel(pickedIso())),
      );
      await waitFor(() =>
        expect(screen.queryByLabelText("Note date and time")).toBeNull(),
      );
      expect(backend.get().meta).toMatchObject({
        title: "Keep this title",
        tags: ["keep"],
        author: "writer",
      });
      expect(native.legacyWrite).not.toHaveBeenCalled();
    },
  );

  it.each([50, 150, 500])(
    "shows the picked label immediately and deduplicates Enter/blur during %sms latency",
    async (delay) => {
      const backend = fixture();
      const save = backend.save.getMockImplementation()!;
      backend.save.mockImplementation(async (...args) => {
        await new Promise((resolve) => setTimeout(resolve, delay));
        return save(...args);
      });
      render(<Host backend={backend} />);
      const input = await edit();
      pick(input);
      fireEvent.keyDown(input, { key: "Enter" });
      fireEvent.blur(input, { relatedTarget: null });
      await waitFor(() =>
        expect(screen.queryByLabelText("Note date and time")).toBeNull(),
      );
      expect(
        screen.getByRole("button", { name: "Edit date" }).textContent,
      ).toBe(dateLabel(pickedIso()));
      await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
      await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
      expect(backend.save).toHaveBeenCalledOnce();
    },
  );

  it("saves an edited value when focus leaves but not when focus moves to an editor button", async () => {
    const backend = fixture();
    render(<Host backend={backend} />);
    const input = await edit();
    pick(input);
    fireEvent.blur(input, {
      relatedTarget: screen.getByRole("button", { name: "Save date" }),
    });
    expect(backend.save).not.toHaveBeenCalled();
    expect(input.isConnected).toBe(true);
    fireEvent.blur(input, { relatedTarget: null });
    await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
    await waitFor(() =>
      expect(screen.queryByLabelText("Note date and time")).toBeNull(),
    );
  });

  it("cancels an unchanged blur without rewriting the timestamp's precision", async () => {
    const backend = fixture();
    backend.set(snapshot("2026-07-02T03:53:42.500Z"));
    render(<Host backend={backend} />);
    fireEvent.blur(await edit(), { relatedTarget: null });
    expect(backend.save).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Note date and time")).toBeNull();
    expect(backend.get().meta.created_at).toBe("2026-07-02T03:53:42.500Z");
  });

  it.each(["Escape", "Cancel"])(
    "cancels through %s without a WebKit blur commit",
    async (action) => {
      const backend = fixture();
      render(<Host backend={backend} />);
      const input = await edit();
      pick(input);
      if (action === "Escape") fireEvent.keyDown(input, { key: "Escape" });
      else {
        const button = screen.getByRole("button", { name: "Cancel date edit" });
        expect(fireEvent.mouseDown(button)).toBe(false);
        fireEvent.click(button);
      }
      expect(backend.save).not.toHaveBeenCalled();
      expect(screen.queryByLabelText("Note date and time")).toBeNull();
      expect(
        screen.getByRole("button", { name: "Edit date" }).textContent,
      ).toBe(dateLabel(ORIGINAL));
    },
  );

  it("validates an empty pick without issuing a write", async () => {
    const backend = fixture();
    render(<Host backend={backend} />);
    pick(await edit(), "");
    await screen.findByText("Date and time are required");
    expect(screen.getByRole("button", { name: "Save date" })).toHaveProperty(
      "disabled",
      true,
    );
    expect(backend.save).not.toHaveBeenCalled();
  });

  it.each([
    ["date", "2026-07-10T12:00:00Z", ["keep"]],
    ["tags", ORIGINAL, ["new tag"]],
  ])(
    "retains the picked date through a background %s change until explicit review",
    async (_field, createdAt, tags) => {
      const backend = fixture();
      render(<Host backend={backend} />);
      const input = await edit();
      pick(input);
      const external = snapshot(createdAt as string, "external");
      external.meta.tags = tags as string[];
      backend.set(external);
      backend.emit({
        kind: "change",
        change: {
          kind: "index_changed",
          entity: "session_headers",
          ids: ["same"],
        },
      });
      await screen.findByText(
        "The note changed. Review its saved date before saving your picked date.",
      );
      expect(input.value).toBe(PICKED);
      expect(screen.getByRole("button", { name: "Save date" })).toHaveProperty(
        "disabled",
        true,
      );
      expect(backend.save).not.toHaveBeenCalled();
      fireEvent.click(
        screen.getByRole("button", { name: "Review saved date" }),
      );
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: "Save date" }),
        ).toHaveProperty("disabled", false),
      );
      expect(input.value).toBe(PICKED);
      clickSave();
      await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
      expect(backend.save.mock.calls[0][1].expected_revision).toBe("external");
      expect(backend.get().meta.tags).toEqual(tags);
    },
  );

  it("shows a typed conflict and requires fresh review before an explicit CAS retry", async () => {
    const backend = fixture();
    render(<Host backend={backend} />);
    const input = await edit();
    pick(input);
    backend.set(snapshot("2026-07-10T12:00:00Z", "external"));
    clickSave();
    await screen.findByText(
      "The note changed. Review its saved date before saving your picked date.",
    );
    expect(
      screen.getByLabelText<HTMLInputElement>("Note date and time").value,
    ).toBe(PICKED);
    expect(screen.getByText(/Current saved date:/).textContent).toContain(
      dateLabel("2026-07-10T12:00:00Z"),
    );
    expect(backend.save).toHaveBeenCalledOnce();
    expect(backend.save.mock.calls[0][1].expected_revision).toBe("r0");
    clickSave();
    expect(backend.save).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Review saved date" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save date" })).toHaveProperty(
        "disabled",
        false,
      ),
    );
    clickSave();
    await waitFor(() => expect(backend.save).toHaveBeenCalledTimes(2));
    expect(backend.save.mock.calls[1][1]).toEqual({
      patch: { created_at: pickedIso() },
      expected_revision: "external",
    });
  });

  it("resolves a committed lost reply by reading the picked date without a second write", async () => {
    const backend = fixture();
    const save = backend.save.getMockImplementation()!;
    backend.save.mockImplementationOnce(async (...args) => {
      await save(...args);
      throw failure();
    });
    render(<Host backend={backend} />);
    pick(await edit());
    clickSave();
    await screen.findByText(
      "Could not confirm the date change. Review the saved date before trying again.",
    );
    expect(
      screen.getByLabelText<HTMLInputElement>("Note date and time").value,
    ).toBe(PICKED);
    clickSave();
    expect(backend.save).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Review saved date" }));
    await waitFor(() =>
      expect(screen.queryByLabelText("Note date and time")).toBeNull(),
    );
    expect(screen.getByRole("button", { name: "Edit date" }).textContent).toBe(
      dateLabel(pickedIso()),
    );
    expect(backend.save).toHaveBeenCalledOnce();
  });

  it("keeps an uncommitted unknown edit through typing and never retries automatically", async () => {
    const backend = fixture("online");
    backend.save.mockRejectedValueOnce(failure());
    render(<Host backend={backend} />);
    pick(await edit());
    clickSave();
    await screen.findByRole("button", { name: "Review saved date" });
    const input = screen.getByLabelText<HTMLInputElement>("Note date and time");
    pick(input, "2026-09-02T11:40");
    backend.emit({ kind: "connection", state: "reconnecting", error: null });
    backend.emit({ kind: "connection", state: "connected", error: null });
    clickSave();
    expect(backend.save).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Review saved date" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save date" })).toHaveProperty(
        "disabled",
        false,
      ),
    );
    clickSave();
    await waitFor(() => expect(backend.save).toHaveBeenCalledTimes(2));
    expect(backend.save.mock.calls[1][1]).toEqual({
      patch: { created_at: new Date("2026-09-02T11:40").toISOString() },
      expected_revision: "r0",
    });
  });

  it("does not mistake a failed confirmation read for a failed durable write", async () => {
    const backend = fixture();
    const save = backend.save.getMockImplementation()!;
    backend.save.mockImplementationOnce(async (...args) => {
      const outcome = await save(...args);
      backend.read.mockRejectedValueOnce(new Error("Confirmation unavailable"));
      return outcome;
    });
    render(<Host backend={backend} />);
    pick(await edit());
    clickSave();
    await screen.findByText(
      "The date was saved, but its current value could not be confirmed.",
    );
    expect(
      screen.getByLabelText<HTMLInputElement>("Note date and time").value,
    ).toBe(PICKED);
    expect(backend.save).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Review saved date" }));
    await waitFor(() =>
      expect(screen.queryByLabelText("Note date and time")).toBeNull(),
    );
    expect(backend.save).toHaveBeenCalledOnce();
  });

  it("never replaces a newer canonical date with a delayed historical save reply", async () => {
    const backend = fixture();
    const save = backend.save.getMockImplementation()!;
    const release = deferred<void>(undefined);
    backend.save.mockImplementationOnce(async (...args) => {
      const outcome = await save(...args);
      await release.promise;
      return outcome;
    });
    render(<Host backend={backend} />);
    pick(await edit());
    clickSave();
    await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
    backend.set(snapshot("2026-10-03T12:00:00Z", "newer"));
    backend.emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "session_headers",
        ids: ["same"],
      },
    });
    const scope = backend.scopes[backend.scopes.length - 1];
    await waitFor(() =>
      expect(
        scope.queryClient.getQueryData(scope.metadataOptions("same").queryKey)
          ?.revision,
      ).toBe("newer"),
    );
    await act(async () => release.resolve());
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Edit date" }).textContent,
      ).toBe(dateLabel("2026-10-03T12:00:00Z")),
    );
    expect(
      scope.queryClient.getQueryData(scope.metadataOptions("same").queryKey)
        ?.revision,
    ).toBe("newer");
  });

  it.each(["reader", "unsupported", "remote-offline"])(
    "refuses %s writes before dispatch",
    async (kind) => {
      const backend = fixture(kind === "remote-offline" ? "online" : "always", {
        canEdit: kind !== "reader",
        features: kind === "unsupported" ? [] : ["metadata_edit"],
      });
      render(<Host backend={backend} />);
      await waitFor(() => expect(backend.read).toHaveBeenCalled());
      if (kind === "remote-offline") act(() => onlineManager.setOnline(false));
      const button = screen.getByRole("button", { name: "Edit date" });
      expect(button).toHaveProperty("disabled", true);
      fireEvent.click(button);
      expect(screen.queryByLabelText("Note date and time")).toBeNull();
      expect(backend.save).not.toHaveBeenCalled();
    },
  );

  it("keeps local reads and date saves available without browser networking or login", async () => {
    onlineManager.setOnline(false);
    const backend = fixture("always");
    render(<Host backend={backend} />);
    pick(await edit());
    clickSave();
    await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Edit date" }).textContent,
      ).toBe(dateLabel(pickedIso())),
    );
    expect(native.legacyWrite).not.toHaveBeenCalled();
  });

  it("retains a disconnected form but never queues its date mutation", async () => {
    const backend = fixture();
    render(<Host backend={backend} />);
    const input = await edit();
    pick(input);
    backend.emit({ kind: "connection", state: "reconnecting", error: null });
    clickSave();
    expect(input.value).toBe(PICKED);
    expect(backend.save).not.toHaveBeenCalled();
    backend.emit({ kind: "connection", state: "connected", error: null });
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save date" })).toHaveProperty(
        "disabled",
        false,
      ),
    );
    expect(backend.save).not.toHaveBeenCalled();
    clickSave();
    await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
  });

  it("hides private form state after access loss and never revives it from a late save reply", async () => {
    const backend = fixture();
    const pending = deferred<MetadataSaveOutcome>({
      kind: "saved",
      snapshot: snapshot(pickedIso(), "late"),
    });
    backend.save.mockReturnValueOnce(pending.promise);
    render(<Host backend={backend} />);
    pick(await edit());
    clickSave();
    await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
    backend.emit({ kind: "connection", state: "access_denied", error: null });
    backend.emit({ kind: "connection", state: "connected", error: null });
    await act(async () =>
      pending.resolve({
        kind: "saved",
        snapshot: snapshot(pickedIso(), "late"),
      }),
    );
    expect(screen.queryByRole("button", { name: "Edit date" })).toBeNull();
    expect(screen.queryByLabelText("Note date and time")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(backend.save).toHaveBeenCalledOnce();
  });

  it("isolates a replacement client with the same scope and note IDs from an old in-flight date", async () => {
    const first = fixture("online", { scope: "same-scope" });
    const second = fixture("online", { scope: "same-scope" });
    second.set(snapshot("2026-10-03T12:00:00Z", "second"));
    const pending = deferred<MetadataSaveOutcome>({
      kind: "saved",
      snapshot: snapshot(pickedIso(), "old"),
    });
    first.save.mockReturnValueOnce(pending.promise);
    const mounted = render(<Host backend={first} />);
    pick(await edit());
    clickSave();
    await waitFor(() => expect(first.save).toHaveBeenCalledOnce());
    const retired = first.scopes[first.scopes.length - 1];
    mounted.rerender(<Host backend={second} />);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Edit date" }).textContent,
      ).toBe(dateLabel("2026-10-03T12:00:00Z")),
    );
    await act(async () =>
      pending.resolve({
        kind: "saved",
        snapshot: snapshot(pickedIso(), "old"),
      }),
    );
    expect(screen.queryByLabelText("Note date and time")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit date" }).textContent).toBe(
      dateLabel("2026-10-03T12:00:00Z"),
    );
    expect(second.save).not.toHaveBeenCalled();
    expect(retired.queryClient.getQueryCache().getAll()).toHaveLength(0);
  });
});
