import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SettingsSync } from "./sync";

import { commands, type SyncStatus } from "~/types/tauri.gen";

vi.mock("~/types/tauri.gen", () => ({
  commands: { syncStatus: vi.fn(), syncAction: vi.fn() },
  events: { syncStatusChanged: { listen: vi.fn(async () => vi.fn()) } },
}));

const disconnected: SyncStatus = {
  enabled: true,
  phase: "disconnected",
  vaultPath: "/copied-test-vault",
  browserUrl: null,
  userCode: null,
  pairingCode: null,
  error: null,
  lastSuccess: null,
  pendingWork: 0,
  activeTransfers: 0,
  transferredBytes: 0,
  transferBytes: 0,
  quotaBytes: 0,
  usedBytes: 0,
  conflicts: [],
  devices: [],
};

function show(status: SyncStatus) {
  vi.mocked(commands.syncStatus).mockResolvedValue(status);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <SettingsSync />
    </QueryClientProvider>,
  );
  return client;
}

describe("sync consent and recovery controls", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.syncAction).mockResolvedValue({
      status: "ok",
      data: { versions: [], preview: null },
    });
  });
  afterEach(cleanup);

  it("requires an explicit connection and hides actions in stable builds", async () => {
    show({ ...disconnected, enabled: false });
    await screen.findByText("Sync is available in Loofah Staging.");
    expect(screen.queryByRole("button", { name: "Connect" })).toBeNull();
    expect(commands.syncAction).not.toHaveBeenCalled();
    cleanup();
    show(disconnected);
    const connect = await screen.findByRole("button", { name: "Connect" });
    expect(commands.syncAction).not.toHaveBeenCalled();
    fireEvent.click(connect);
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("connect"),
    );
  });

  it("keeps cancellation available while browser authorization is pending", async () => {
    vi.mocked(commands.syncAction).mockImplementation(async (action) => {
      if (action === "connect") return new Promise(() => {});
      return { status: "ok", data: { versions: [], preview: null } };
    });
    const client = show(disconnected);
    fireEvent.click(await screen.findByRole("button", { name: "Connect" }));
    client.setQueryData(["sync-status"], {
      ...disconnected,
      phase: "authorizing",
      userCode: "ABCD-EFGH",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("cancel"),
    );
  });

  it("offers recovery import and pairing when an existing vault has no local keys", async () => {
    show({ ...disconnected, phase: "import_recovery_kit" });
    await screen.findByRole("button", { name: "Import recovery kit…" });
    expect(
      screen.getByRole("button", { name: "Pair with a trusted Mac" }),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Save recovery kit…" }),
    ).toBeNull();
    expect(commands.syncAction).not.toHaveBeenCalled();
  });

  it("requires an explicit choice for a global registry conflict", async () => {
    show({
      ...disconnected,
      phase: "paused",
      conflicts: [
        {
          entity: { kind: "people" },
          local: "local-revision",
          cloud: "cloud-revision",
        },
      ],
    });
    await screen.findByText(
      /This choice replaces the complete global registry/,
    );
    expect(commands.syncAction).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Use cloud version" }));
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith({
        restore: { entity: { kind: "people" }, revision: "cloud-revision" },
      }),
    );
  });
});
