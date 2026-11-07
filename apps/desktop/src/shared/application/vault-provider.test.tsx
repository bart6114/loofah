import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
  useQuery,
} from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode, useSyncExternalStore } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({ connect: vi.fn() }));
vi.mock("./vault-client", () => ({ connectLocalVault: native.connect }));
import type { VaultClient, VaultClientEvent } from "./vault-client";
import {
  LocalVaultProvider,
  VaultClientProvider,
  useVaultHost,
} from "./vault-provider";
import type { VaultScope } from "./vault-scope";

function Ready({ scope }: { scope: VaultScope }) {
  const value = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  return (
    <p>
      {scope.client.scope}:{value.state}
    </p>
  );
}
function Probe() {
  const host = useVaultHost();
  return host.scope ? (
    <Ready scope={host.scope} />
  ) : (
    <>
      <p>Waiting for local vault</p>
      {host.error && (
        <>
          <p>{host.error.message}</p>
          <button onClick={host.retry}>Retry connection</button>
        </>
      )}
    </>
  );
}
function start() {
  let active = 0;
  const unsubscribe = vi.fn(() => {
    active -= 1;
  });
  native.connect.mockResolvedValue({
    scope: "local:fixture",
    networkMode: "always",
    subscribe: async (listener: any) => {
      active += 1;
      listener({ kind: "connection", state: "connected", error: null });
      return unsubscribe;
    },
  });
  const mounted = render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <LocalVaultProvider>
          <Probe />
        </LocalVaultProvider>
      </QueryClientProvider>
    </StrictMode>,
  );
  return { ...mounted, count: () => active, unsubscribe };
}
beforeEach(() => {
  vi.resetAllMocks();
});
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});

it("starts the local IPC host while the browser is offline without any sign-in flow", async () => {
  onlineManager.setOnline(false);
  start();
  await screen.findByText("local:fixture:connected");
  expect(native.connect).toHaveBeenCalledOnce();
});

it("keeps one subscription through StrictMode and releases it on unmount", async () => {
  const mounted = start();
  await screen.findByText("local:fixture:connected");
  expect(mounted.count()).toBe(1);
  act(() => mounted.unmount());
  await waitFor(() => expect(mounted.count()).toBe(0));
});

function connection(scopeId = "remote:fixture", markdown = "Remote note") {
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const stop = vi.fn();
  const client = {
    scope: scopeId,
    networkMode: "online",
    canEdit: true,
    readNote: vi.fn(async () => ({ markdown, title: "", revision: "r0" })),
    subscribe: vi.fn(async (listener: (event: VaultClientEvent) => void) => {
      listeners.add(listener);
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(listener);
        stop();
      };
    }),
  } as unknown as VaultClient & {
    readNote: ReturnType<typeof vi.fn<VaultClient["readNote"]>>;
    subscribe: ReturnType<typeof vi.fn<VaultClient["subscribe"]>>;
  };
  return {
    client,
    stop,
    listeners,
    emit(event: VaultClientEvent) {
      for (const listener of listeners) listener(event);
    },
  };
}

function Note({ scope }: { scope: VaultScope }) {
  const result = useQuery(scope.noteOptions("same"), scope.queryClient);
  return <p>{result.data?.markdown ?? "Loading scoped note"}</p>;
}

function NoteProbe({ onScope }: { onScope?: (scope: VaultScope) => void }) {
  const { scope } = useVaultHost();
  if (!scope) return <p>Waiting for captured client</p>;
  onScope?.(scope);
  return <Note scope={scope} />;
}

it("uses an explicit remote client without querying a local connector or requiring a root QueryClient", async () => {
  const current = connection();
  const mounted = render(
    <StrictMode>
      <VaultClientProvider client={current.client}>
        <NoteProbe />
      </VaultClientProvider>
    </StrictMode>,
  );
  await screen.findByText("Remote note");
  expect(native.connect).not.toHaveBeenCalled();
  await waitFor(() => expect(current.listeners.size).toBe(1));
  mounted.unmount();
  await waitFor(() => expect(current.listeners.size).toBe(0));
});

it("retains the current scope while callback identities change and routes catalog changes to the newest callback", async () => {
  const current = connection();
  const original = vi.fn(),
    replacement = vi.fn();
  const scopes = new Set<VaultScope>();
  const mounted = render(
    <VaultClientProvider client={current.client} onChange={original}>
      <NoteProbe onScope={(scope) => scopes.add(scope)} />
    </VaultClientProvider>,
  );
  await screen.findByText("Remote note");
  mounted.rerender(
    <VaultClientProvider client={current.client} onChange={replacement}>
      <NoteProbe onScope={(scope) => scopes.add(scope)} />
    </VaultClientProvider>,
  );
  act(() =>
    current.emit({ kind: "change", change: { kind: "catalog_changed" } }),
  );
  expect(original).not.toHaveBeenCalled();
  expect(replacement).toHaveBeenCalledWith({ kind: "catalog_changed" });
  expect(scopes.size).toBe(1);
  expect(current.client.subscribe).toHaveBeenCalledOnce();
  expect(current.stop).not.toHaveBeenCalled();
});

it("remounts note observers on client replacement with the same scope string and identical note IDs", async () => {
  const original = connection("same-scope", "Old vault note"),
    replacement = connection("same-scope", "Replacement note");
  const scopes: VaultScope[] = [];
  const mounted = render(
    <VaultClientProvider client={original.client}>
      <NoteProbe onScope={(scope) => scopes.push(scope)} />
    </VaultClientProvider>,
  );
  await screen.findByText("Old vault note");
  const previous = scopes[scopes.length - 1];
  mounted.rerender(
    <VaultClientProvider client={replacement.client}>
      <NoteProbe onScope={(scope) => scopes.push(scope)} />
    </VaultClientProvider>,
  );
  await screen.findByText("Replacement note");
  expect(screen.queryByText("Old vault note")).toBeNull();
  const current = scopes[scopes.length - 1];
  expect(current).not.toBe(previous);
  expect(current.queryClient).not.toBe(previous.queryClient);
  expect(previous.getSnapshot().state).toBe("closed");
  expect(previous.queryClient.getQueryCache().getAll()).toEqual([]);
  expect(original.stop).toHaveBeenCalledOnce();
  expect(native.connect).not.toHaveBeenCalled();
});

it("rejects retired pending completions without admitting data or events into a replacement host", async () => {
  const original = connection("same-scope"),
    replacement = connection("same-scope", "Replacement note");
  let resolveRead!: (
    value: Awaited<ReturnType<VaultClient["readNote"]>>,
  ) => void;
  original.client.readNote.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        resolveRead = resolve;
      }),
  );
  const oldChanged = vi.fn(),
    newChanged = vi.fn();
  const scopes: VaultScope[] = [];
  const mounted = render(
    <VaultClientProvider client={original.client} onChange={oldChanged}>
      <NoteProbe onScope={(scope) => scopes.push(scope)} />
    </VaultClientProvider>,
  );
  await waitFor(() => expect(original.client.readNote).toHaveBeenCalledOnce());
  const previous = scopes[scopes.length - 1];
  const lateEvent = [...original.listeners][0];
  mounted.rerender(
    <VaultClientProvider client={replacement.client} onChange={newChanged}>
      <NoteProbe onScope={(scope) => scopes.push(scope)} />
    </VaultClientProvider>,
  );
  await screen.findByText("Replacement note");
  await act(async () => {
    lateEvent({ kind: "change", change: { kind: "catalog_changed" } });
    resolveRead({
      markdown: "Retired private text",
      title: "",
      revision: "r1",
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  expect(oldChanged).not.toHaveBeenCalled();
  expect(newChanged).not.toHaveBeenCalled();
  expect(screen.queryByText("Retired private text")).toBeNull();
  expect(previous.queryClient.getQueryCache().getAll()).toEqual([]);
  expect(
    scopes[scopes.length - 1].queryClient.getQueryData(
      previous.noteOptions("same").queryKey,
    ),
  ).toMatchObject({ markdown: "Replacement note" });
});

it("releases a late subscription acknowledgement after retirement without reconnecting the old host", async () => {
  const original = connection(),
    replacement = connection("replacement");
  let acknowledge!: (stop: () => void) => void;
  let lateEvent!: (value: VaultClientEvent) => void;
  original.client.subscribe.mockImplementationOnce((listener) => {
    lateEvent = listener;
    return new Promise((resolve) => {
      acknowledge = resolve;
    });
  });
  const stop = vi.fn(),
    changed = vi.fn();
  const mounted = render(
    <VaultClientProvider client={original.client} onChange={changed}>
      <Probe />
    </VaultClientProvider>,
  );
  await screen.findByText("remote:fixture:connecting");
  mounted.rerender(
    <VaultClientProvider client={replacement.client} onChange={changed}>
      <Probe />
    </VaultClientProvider>,
  );
  await screen.findByText("replacement:connected");
  await act(async () => {
    lateEvent({ kind: "connection", state: "connected", error: null });
    lateEvent({ kind: "change", change: { kind: "catalog_changed" } });
    acknowledge(stop);
  });
  expect(stop).toHaveBeenCalledOnce();
  expect(changed).not.toHaveBeenCalled();
  expect(screen.queryByText("remote:fixture:connected")).toBeNull();
  expect(replacement.client.subscribe).toHaveBeenCalledOnce();
});

it("reports an unavailable explicit client without any local fallback", async () => {
  const retry = vi.fn();
  render(
    <VaultClientProvider
      client={undefined}
      error={new Error("Remote profile unavailable")}
      retry={retry}
    >
      <Probe />
    </VaultClientProvider>,
  );
  await screen.findByText("Remote profile unavailable");
  fireEvent.click(screen.getByRole("button", { name: "Retry connection" }));
  expect(retry).toHaveBeenCalledOnce();
  expect(native.connect).not.toHaveBeenCalled();
});

it("preserves the local connector's offline mode, infinite freshness and four retries", async () => {
  native.connect.mockRejectedValue(new Error("Local vault unavailable"));
  const cache = new QueryClient({
    defaultOptions: { queries: { retryDelay: 0 } },
  });
  render(
    <QueryClientProvider client={cache}>
      <LocalVaultProvider>
        <Probe />
      </LocalVaultProvider>
    </QueryClientProvider>,
  );
  await screen.findByText("Local vault unavailable");
  expect(native.connect).toHaveBeenCalledTimes(5);
  expect(
    cache.getQueryCache().find({ queryKey: ["application", "local-vault"] })
      ?.options,
  ).toMatchObject({ networkMode: "always", staleTime: Infinity, retry: 4 });
});
