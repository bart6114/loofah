import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: new Map<string, () => void>(),
  unlisten: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, listener: () => void) => {
    mocks.listeners.set(name, listener);
    return mocks.unlisten;
  }),
}));
vi.mock("~/settings/general/storage/company", () => ({
  CompanyStorageSettings: () => <div>Storage recovery settings</div>,
}));

import { CompanyBoundary } from "./company-boundary";
import { useCompanyActive, useCompanyReadOnly } from "./company-permissions";

let status: Record<string, unknown>;
let vaults: unknown[];
const clients: QueryClient[] = [];

beforeEach(() => {
  vi.clearAllMocks();
  mocks.listeners.clear();
  status = {
    active: true,
    profile: { server_id: "server", user_id: "me", vault_id: "team" },
    pending: [],
    transfers: [],
    error: null,
  };
  vaults = [{ id: "team", members: { me: "editor" } }];
  mocks.invoke.mockImplementation(async (_, { operation, args }) => {
    if (operation === "status") return status;
    if (operation === "request" && args.path === "/v1/vaults") return vaults;
    throw new Error("Unexpected company request");
  });
});
afterEach(() => {
  cleanup();
  for (const client of clients.splice(0)) client.clear();
});

function Consumer() {
  return (
    <div>
      Private meeting contents · active={String(useCompanyActive())} · readOnly=
      {String(useCompanyReadOnly())}
    </div>
  );
}

function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  clients.push(client);
  const view = render(
    <QueryClientProvider client={client}>
      <CompanyBoundary>
        <Consumer />
      </CompanyBoundary>
    </QueryClientProvider>,
  );
  return { client, ...view };
}

async function emit(name: string) {
  await act(async () => mocks.listeners.get(name)!());
}

it.each(["owner", "editor", "viewer", undefined])(
  "allows editing only for a confirmed owner or editor (%s)",
  async (role) => {
    vaults = [{ id: "team", members: role ? { me: role } : {} }];
    show();
    const readOnly = role !== "owner" && role !== "editor";
    await screen.findByText(
      `Private meeting contents · active=true · readOnly=${readOnly}`,
    );
    expect(screen.getByRole("status").textContent).toBe(
      readOnly ? "Read-only company vault" : "Saved to company vault",
    );
  },
);

it("keeps a company vault read-only when membership cannot be loaded", async () => {
  const fallback = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation(async (...args) => {
    if (args[1].operation === "request") throw new Error("Offline");
    return fallback(...args);
  });
  show();
  await screen.findByText(
    "Private meeting contents · active=true · readOnly=true",
  );
});

it("keeps local editing available without requesting remote memberships", async () => {
  status.active = false;
  status.profile = null;
  show();
  await screen.findByText(
    "Private meeting contents · active=false · readOnly=false",
  );
  expect(
    mocks.invoke.mock.calls.every(([, arg]) => arg.operation === "status"),
  ).toBe(true);
  expect(screen.queryByRole("status")).toBeNull();
});

it("shows pending work and connection errors from native status events", async () => {
  const { client } = show();
  await screen.findByText("Saved to company vault");
  status = { ...status, pending: [{}], transfers: [{}] };
  await emit("company-status-changed");
  await screen.findByText("Saving to company vault · 2 pending");
  status = { ...status, error: { message: "Server unavailable" } };
  await emit("company-status-changed");
  await screen.findByText("Server unavailable");
  expect(client.getQueryData(["company", "status"])).toEqual(status);
});

it("refreshes membership on events and immediately applies a demotion", async () => {
  const { client } = show();
  await screen.findByText("Saved to company vault");
  client.setQueryData(["unrelated"], "keep");
  vaults = [{ id: "team", members: { me: "viewer" } }];
  await emit("company-vault-list-changed");
  await screen.findByText(
    "Private meeting contents · active=true · readOnly=true",
  );
  expect(client.getQueryData(["unrelated"])).toBe("keep");
});

it("removes sensitive UI and cached content on revocation until authentication succeeds", async () => {
  const { client } = show();
  await screen.findByText("Saved to company vault");
  client.setQueryData(["session", "private"], { note: "Private draft" });
  await emit("company-access-revoked");
  await screen.findByText("Company vault access is unavailable");
  expect(screen.queryByText(/Private meeting contents/)).toBeNull();
  expect(screen.getByText("Storage recovery settings")).toBeTruthy();
  expect(client.getQueryData(["session", "private"])).toBeUndefined();
  await emit("company-status-changed");
  expect(screen.queryByText(/Private meeting contents/)).toBeNull();
  await emit("company-authenticated");
  await screen.findByText("Saved to company vault");
  await screen.findByText(
    "Private meeting contents · active=true · readOnly=false",
  );
});

it("unsubscribes all native events on unmount", async () => {
  const { unmount } = show();
  await waitFor(() => expect(mocks.listeners.size).toBe(5));
  unmount();
  await waitFor(() => expect(mocks.unlisten).toHaveBeenCalledTimes(5));
});
