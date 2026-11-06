import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useSyncExternalStore } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({ connect: vi.fn() }));
vi.mock("./vault-client", () => ({ connectLocalVault: native.connect }));
import { LocalVaultProvider, useVaultHost } from "./vault-provider";
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
    <p>Waiting for local vault</p>
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
