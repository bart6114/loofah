import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), flush: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
  isTauri: () => false,
}));
vi.mock("~/shared/write-queue", () => ({ flushDatabaseWrites: mocks.flush }));

import { CompanyStorageSettings } from "./company";

beforeEach(() => {
  vi.clearAllMocks();
  mocks.flush.mockResolvedValue(undefined);
  mocks.invoke.mockImplementation(
    async (
      _command: string,
      { operation, args }: { operation: string; args: Record<string, unknown> },
    ) => {
      if (operation === "status")
        return {
          active: true,
          recording: false,
          profile: {
            base_url: "https://company.example",
            server_id: "server",
            user_id: "me",
            vault_id: "team",
          },
          pending: [],
          transfers: [],
          error: null,
        };
      if (operation === "request" && args.path === "/v1/vaults")
        return [
          {
            id: "team",
            name: "Team notes",
            personal: false,
            members: { me: "owner", other: "viewer" },
          },
        ];
      if (operation === "request" && args.path === "/v1/users")
        return [
          { id: "me", name: "Me", email: "me@example.com" },
          { id: "other", name: "Other person", email: "other@example.com" },
        ];
      return {};
    },
  );
});
afterEach(cleanup);

function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <CompanyStorageSettings />
    </QueryClientProvider>,
  );
}

it("switches destinations only after pending frontend writes are flushed", async () => {
  show();
  await screen.findByText("Manage shared vault");
  fireEvent.change(screen.getByLabelText("Vault location"), {
    target: { value: "" },
  });
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "select",
      args: { vault_id: null },
    }),
  );
  expect(mocks.flush.mock.invocationCallOrder[0]).toBeLessThan(
    mocks.invoke.mock.invocationCallOrder[
      mocks.invoke.mock.calls.findIndex(([, arg]) => arg.operation === "select")
    ],
  );
});

it("lets owners edit membership and displays server rejection", async () => {
  show();
  await screen.findByText("Other person");
  mocks.invoke.mockRejectedValueOnce(
    new Error("A shared vault must retain an owner"),
  );
  fireEvent.change(screen.getAllByLabelText("Member role")[0], {
    target: { value: "viewer" },
  });
  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain("retain an owner");
});

it("leaves unsaved work visible when saving is unavailable", async () => {
  const fallback = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation(async (command, args) =>
    args.operation === "status"
      ? {
          active: true,
          recording: false,
          profile: {
            base_url: "https://company.example",
            server_id: "server",
            user_id: "me",
            vault_id: "team",
          },
          pending: [
            {
              call: {
                mutation_id: "draft",
                operation: "session_save_note",
                args: { session_id: "s1", markdown: "Keep this draft" },
              },
              error: { code: "unavailable", message: "Server offline" },
            },
          ],
          transfers: [],
          error: { code: "unavailable", message: "Server offline" },
        }
      : fallback(command, args),
  );
  show();
  await screen.findByText("Work waiting to be saved");
  fireEvent.click(screen.getByText("Server offline", { selector: "summary" }));
  expect(screen.getByDisplayValue("Keep this draft")).toBeTruthy();
  expect(screen.queryByText("Replace the displayed server version")).toBeNull();
});
