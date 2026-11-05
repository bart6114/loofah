import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ApplicationRequestError } from "./errors";
import type { VaultInfo } from "./protocol.gen";
import type { NativeRemoteConnection } from "./remote";
import { CreateSharedVault, VaultSharing } from "./vault-sharing";

import type { RemoteOperation } from "~/types/tauri.gen";

const shared: VaultInfo = {
  id: "shared",
  name: "Team notes",
  kind: "shared",
  role: "owner",
  revision: "a",
};
function fixture() {
  const memberships = [
    { member: { id: "alice", name: "Alice" }, role: "owner" },
    { member: { id: "bob", name: "Bob" }, role: "editor" },
  ];
  const request = vi.fn(
    async (
      operation: RemoteOperation,
      _reply?: string,
      _options?: { requestId?: string; signal?: AbortSignal },
    ): Promise<unknown> => {
      if (operation.kind === "memberships") return memberships;
      if (operation.kind === "directory")
        return [
          ...memberships.map((entry) => entry.member),
          { id: "chris", name: "Chris" },
        ];
      return { vault_id: "shared", revision: "b" };
    },
  );
  return {
    request,
    connection: {
      info: { server: "https://company.example/" },
      request,
    } as unknown as NativeRemoteConnection,
  };
}
function mount(element: React.ReactNode) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>{element}</QueryClientProvider>
    </StrictMode>,
  );
}
afterEach(() => cleanup());

describe("shared vault controls", () => {
  it("protects the last owner and grants only an existing company member with the reviewed revision", async () => {
    const { connection, request } = fixture();
    mount(
      <VaultSharing connection={connection} memberId="alice" vault={shared} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Vault members" }));
    const owner = await screen.findByRole("combobox", {
      name: "Role for Alice",
    });
    expect((owner as HTMLSelectElement).disabled).toBe(true);
    const members = await screen.findByRole("combobox", {
      name: "Company member",
    });
    await waitFor(() =>
      expect(screen.getByRole("option", { name: "Chris" })).toBeTruthy(),
    );
    expect(
      [...(members as HTMLSelectElement).options].map((option) => option.value),
    ).toEqual(["", "chris"]);
    fireEvent.change(members, { target: { value: "chris" } });
    fireEvent.click(screen.getByRole("button", { name: "Add member" }));
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith(
        {
          kind: "set_membership",
          vault_id: "shared",
          member_id: "chris",
          input: { role: "reader", expected_revision: "a" },
        },
        "vault_mutation",
      ),
    );
  });

  it("shows members to readers without loading the directory or offering mutations", async () => {
    const { connection, request } = fixture();
    mount(
      <VaultSharing
        connection={connection}
        memberId="bob"
        vault={{ ...shared, role: "reader" }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Vault members" }));
    await screen.findByText("Alice");
    expect(screen.queryByRole("button", { name: "Add member" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Rename vault" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete vault" })).toBeNull();
    expect(
      request.mock.calls.some(([operation]) => operation.kind === "directory"),
    ).toBe(false);
  });

  it("offers no sharing controls for a personal vault", () => {
    const { connection, request } = fixture();
    mount(
      <VaultSharing
        connection={connection}
        memberId="alice"
        vault={{ ...shared, kind: "personal" }}
      />,
    );
    expect(screen.queryByRole("button", { name: "Vault members" })).toBeNull();
    expect(request).not.toHaveBeenCalled();
  });

  it("requires explicit deletion confirmation and checks the catalog revision", async () => {
    const { connection, request } = fixture();
    mount(
      <VaultSharing connection={connection} memberId="alice" vault={shared} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Vault members" }));
    const remove = await screen.findByRole("button", { name: "Delete vault" });
    expect((remove as HTMLButtonElement).disabled).toBe(true);
    expect(
      request.mock.calls.some(
        ([operation]) => operation.kind === "delete_vault",
      ),
    ).toBe(false);
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Delete this shared vault" }),
    );
    fireEvent.click(remove);
    await waitFor(() =>
      expect(request).toHaveBeenCalledWith(
        { kind: "delete_vault", vault_id: "shared", expected_revision: "a" },
        "vault_mutation",
      ),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("clears the member view after access loss", async () => {
    const { connection, request } = fixture();
    request.mockImplementation(async (operation) => {
      if (operation.kind === "memberships")
        return [
          { member: { id: "alice", name: "Private name" }, role: "owner" },
        ];
      if (operation.kind === "directory") return [];
      throw new ApplicationRequestError({
        code: "forbidden",
        message: "Access removed",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      });
    });
    mount(
      <VaultSharing connection={connection} memberId="alice" vault={shared} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Vault members" }));
    await screen.findByText("Private name");
    fireEvent.change(screen.getByRole("textbox", { name: "Vault name" }), {
      target: { value: "New name" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Rename vault" }));
    await screen.findByText(
      "Access to this vault changed. Close this panel and reload your vaults.",
    );
    expect(screen.queryByText("Private name")).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete vault" })).toBeNull();
  });

  it("requires a reload after a concurrent catalog change before another mutation", async () => {
    const { connection, request } = fixture();
    const original = request.getMockImplementation()!;
    request.mockImplementation(async (operation, ...args) => {
      if (operation.kind === "rename_vault")
        throw new ApplicationRequestError({
          code: "conflict",
          message: "Access changed since it was read",
          retryable: false,
          request_id: null,
          current_revision: "b",
          mutation_outcome: "not_committed",
        });
      return original(operation, ...args);
    });
    mount(
      <VaultSharing connection={connection} memberId="alice" vault={shared} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Vault members" }));
    await screen.findByText("Bob");
    fireEvent.click(screen.getByRole("button", { name: "Rename vault" }));
    await screen.findByText("Access changed since it was read");
    expect(
      (screen.getByRole("button", { name: "Add member" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(
      request.mock.calls.filter(
        ([operation]) => operation.kind === "rename_vault",
      ),
    ).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Reload access" }));
    await waitFor(() =>
      expect(screen.queryByText("Access changed since it was read")).toBeNull(),
    );
    expect(
      (
        screen.getByRole("button", {
          name: "Rename vault",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false);
  });

  it("retries uncertain creation with the original UUID and payload after closing the dialog", async () => {
    const { connection, request } = fixture();
    const created = vi.fn();
    request
      .mockRejectedValueOnce(
        new ApplicationRequestError({
          code: "unavailable",
          message: "Response lost",
          retryable: true,
          request_id: null,
          current_revision: null,
          mutation_outcome: "unknown",
        }),
      )
      .mockResolvedValueOnce(shared);
    mount(
      <CreateSharedVault
        connection={connection}
        memberId="alice"
        onCreated={created}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Create shared vault" }),
    );
    fireEvent.change(screen.getByRole("textbox", { name: "Vault name" }), {
      target: { value: "Team notes" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create vault" }));
    await screen.findByText("Response lost");
    const first = request.mock.calls[0];
    expect(first).toEqual([
      { kind: "create_vault", input: { name: "Team notes" } },
      "vault",
      { requestId: expect.any(String) },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Create shared vault" }),
    );
    expect(
      (screen.getByRole("textbox", { name: "Vault name" }) as HTMLInputElement)
        .readOnly,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Retry creation" }));
    await waitFor(() => expect(created).toHaveBeenCalledWith(shared));
    expect(request.mock.calls[1]).toEqual(first);
  });
});
