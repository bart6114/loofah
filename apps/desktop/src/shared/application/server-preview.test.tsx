import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ connect: vi.fn(), workspace: vi.fn() }));
vi.mock("./remote", () => ({
  NativeRemoteConnection: { connect: mocks.connect },
}));
vi.mock("./notes-workspace", () => ({
  VaultNotesWorkspace: ({
    client,
  }: {
    client: { scope: string; canEdit: boolean };
  }) => {
    mocks.workspace(client);
    return (
      <div
        data-testid="workspace"
        data-scope={client.scope}
        data-editable={client.canEdit}
      />
    );
  },
}));

import { ApplicationRequestError } from "./errors";
import ServerPreview from "./server-preview";

const me = {
  member: { id: "alice", name: "Alice" },
  personal_vault_id: "private",
};
const vault = {
  id: "private",
  name: "My notes",
  kind: "personal",
  role: "owner",
  revision: "a",
};
function connection() {
  return {
    info: {
      connection_id: "native",
      server: "https://company.example/",
      capabilities: {
        api_major: 1,
        application_version: "test",
        features: ["notes", "change_stream"],
      },
    },
    restore: vi.fn().mockResolvedValue(me),
    signIn: vi.fn().mockResolvedValue(me),
    disconnect: vi.fn().mockResolvedValue(undefined),
    signOut: vi.fn().mockResolvedValue(undefined),
    cancelSignIn: vi.fn().mockResolvedValue(undefined),
    request: vi.fn().mockResolvedValue([vault]),
    subscribe: vi.fn(),
  };
}
function renderPreview() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <ServerPreview />
      </QueryClientProvider>
    </StrictMode>,
  );
  return queryClient;
}
async function enterServer() {
  fireEvent.click(screen.getByRole("button", { name: "Try server notes" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Server address" }), {
    target: { value: "https://company.example" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Connect" }));
}

beforeEach(() => {
  vi.resetAllMocks();
});
afterEach(() => cleanup());

describe("opt-in server connection preview", () => {
  it("starts no authentication work until the user explicitly connects", () => {
    renderPreview();
    expect(mocks.connect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Try server notes" }));
    expect(mocks.connect).not.toHaveBeenCalled();
  });

  it("restores native credentials and opens the personal vault with an account-scoped client", async () => {
    const native = connection();
    mocks.connect.mockResolvedValue(native);
    renderPreview();
    await enterServer();
    const workspace = await screen.findByTestId("workspace");
    expect(workspace.dataset.scope).toBe(
      JSON.stringify(["remote", native.info.server, "alice", "private"]),
    );
    expect(native.signIn).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(native.signOut).toHaveBeenCalledOnce());
    await screen.findByRole("textbox", { name: "Server address" });
    expect(screen.queryByTestId("workspace")).toBeNull();
  });

  it("opens company sign-in only when no native session can be restored", async () => {
    const native = connection();
    native.restore.mockRejectedValueOnce(
      new ApplicationRequestError({
        code: "unauthenticated",
        message: "Sign in",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: null,
      }),
    );
    mocks.connect.mockResolvedValue(native);
    renderPreview();
    await enterServer();
    await screen.findByRole("button", { name: "Company sign-in" });
    expect(native.signIn).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Company sign-in" }));
    await screen.findByTestId("workspace");
    expect(native.signIn).toHaveBeenCalledOnce();
  });

  it("retains the active client when only the vault catalog name or revision changes", async () => {
    const native = connection();
    mocks.connect.mockResolvedValue(native);
    const queryClient = renderPreview();
    await enterServer();
    const workspace = await screen.findByTestId("workspace");
    const scope = workspace.dataset.scope;
    const originalClient =
      mocks.workspace.mock.calls[mocks.workspace.mock.calls.length - 1]?.[0];
    queryClient.setQueryData(
      ["server-preview", native.info.server, me.member.id, "vaults"],
      [{ ...vault, name: "Renamed", revision: "b" }],
    );
    await screen.findByRole("option", { name: "Renamed" });
    expect(screen.getByTestId("workspace")).toBe(workspace);
    expect(workspace.dataset.scope).toBe(scope);
    expect(
      mocks.workspace.mock.calls[mocks.workspace.mock.calls.length - 1]?.[0],
    ).toBe(originalClient);
  });

  it("releases a connection whose discovery finishes after the dialog was closed", async () => {
    const native = connection();
    let complete!: (value: unknown) => void;
    mocks.connect.mockReturnValueOnce(
      new Promise((resolve) => {
        complete = resolve;
      }),
    );
    renderPreview();
    await enterServer();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    complete(native);
    await waitFor(() => expect(native.disconnect).toHaveBeenCalledOnce());
    expect(screen.queryByTestId("workspace")).toBeNull();
  });
});
