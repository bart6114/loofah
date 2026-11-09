import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { expect, it, vi } from "vitest";
const native = vi.hoisted(() => ({
  claim: vi.fn(),
  disconnect: vi.fn(),
  connect: vi.fn(),
  layout: vi.fn(),
  local: vi.fn(),
  current: true,
  scope: { client: { scope: "original shared vault" } },
}));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () => "remote-note-original-lease",
}));
vi.mock("./remote", () => ({
  NativeRemoteConnection: { claimNoteWindow: native.claim },
}));
vi.mock("./vault-client", () => ({ connectRemoteVault: native.connect }));
vi.mock("./vault-provider", () => ({
  VaultClientProvider: ({ children }: any) => children,
  useVaultHost: () => ({ scope: native.scope, error: null }),
}));
vi.mock("./use-vault-read-scope", () => ({
  useVaultReadScope: () => ({
    scope: native.scope,
    current: () => native.current,
    readable: native.current,
  }),
}));
vi.mock("~/main/layout", () => ({
  ClassicMainLayout: native.local,
  CapturedMainLayout: ({ children, ...props }: any) => {
    native.layout(props);
    return children;
  },
}));
vi.mock("~/routes/app/note.$sessionId", () => ({
  StandaloneNoteContent: ({ sessionId }: any) => <output>{sessionId}</output>,
}));
vi.mock("~/store/zustand/ai-task", () => ({ createAITaskStore: () => ({}) }));
vi.mock("~/store/zustand/listener", () => ({
  createListenerStore: () => ({}),
}));
import { RemoteNoteWindow } from "./remote-note-window";
it("claims once in StrictMode, renders only the original note and clears it on retirement", async () => {
  const connection = { disconnect: native.disconnect };
  const vault = { id: "shared", role: "reader" };
  native.claim.mockResolvedValue({
    connection,
    info: { session_id: "original-note", member_id: "original-member", vault },
  });
  native.connect.mockReturnValue(native.scope.client);
  native.disconnect.mockResolvedValue(undefined);
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const tree = () => (
    <StrictMode>
      <QueryClientProvider client={cache}>
        <RemoteNoteWindow />
      </QueryClientProvider>
    </StrictMode>
  );
  const view = render(tree());
  await waitFor(() => expect(screen.getByText("original-note")).toBeTruthy());
  expect(native.claim).toHaveBeenCalledOnce();
  expect(native.connect).toHaveBeenCalledWith(
    connection,
    "original-member",
    vault,
  );
  expect(native.local).not.toHaveBeenCalled();
  expect(native.disconnect).not.toHaveBeenCalled();
  expect(native.layout).toHaveBeenCalledWith(
    expect.objectContaining({ scope: native.scope, managePins: false }),
  );
  native.current = false;
  view.rerender(tree());
  await waitFor(() => expect(screen.queryByText("original-note")).toBeNull());
  expect(screen.getByRole("alert")).toBeTruthy();
  view.unmount();
  await act(async () => {});
  expect(native.disconnect).toHaveBeenCalledOnce();
  cache.clear();
});
