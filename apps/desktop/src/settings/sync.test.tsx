import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
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
  accountEmail: null,
  currentDeviceId: null,
  hasStarted: false,
  upToDate: false,
  pairingRole: null,
  notice: null,
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
      data: { versions: [], preview: null, cancelled: false },
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
      return {
        status: "ok",
        data: { versions: [], preview: null, cancelled: false },
      };
    });
    const client = show(disconnected);
    fireEvent.click(await screen.findByRole("button", { name: "Connect" }));
    client.setQueryData(["sync-status"], {
      ...disconnected,
      phase: "authorizing",
    });
    await screen.findByText("Requesting a browser approval code…");
    expect(screen.queryByText(/Approve code/)).toBeNull();
    client.setQueryData(["sync-status"], {
      ...disconnected,
      phase: "authorizing",
      userCode: "ABCD-EFGH",
    });
    await screen.findByText("ABCD-EFGH");
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

  it("requires explicit first-sync consent after securing this Mac", async () => {
    show({
      ...disconnected,
      phase: "paused",
      accountEmail: "bart@example.com",
      currentDeviceId: "this-mac",
    });
    const start = await screen.findByRole("button", { name: "Start syncing" });
    expect(screen.queryByRole("button", { name: "Resume syncing" })).toBeNull();
    expect(screen.getByText(/bart@example.com/)).toBeTruthy();
    expect(commands.syncAction).not.toHaveBeenCalled();
    fireEvent.click(start);
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("resume"),
    );
  });

  it("offers Resume only for a vault that has already started syncing", async () => {
    show({ ...disconnected, phase: "paused", hasStarted: true });
    fireEvent.click(
      await screen.findByRole("button", { name: "Resume syncing" }),
    );
    expect(screen.queryByRole("button", { name: "Start syncing" })).toBeNull();
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("resume"),
    );
  });

  it("can reopen browser approval without creating another connection", async () => {
    show({
      ...disconnected,
      phase: "authorizing",
      browserUrl: "https://staging-app.loofah.io/device",
      userCode: "ABCD-EFGH",
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Open browser again" }),
    );
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("reopen_browser"),
    );
    const copy = screen.getByRole("button", { name: "Copy code" });
    await waitFor(() => expect(copy.hasAttribute("disabled")).toBe(false));
    fireEvent.click(copy);
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("copy_code"),
    );
    expect(commands.syncAction).not.toHaveBeenCalledWith("connect");
  });

  it("copies the displayed pairing code on the new Mac", async () => {
    show({
      ...disconnected,
      phase: "pairing",
      pairingRole: "new",
      pairingCode: "7-correct-horse",
    });
    await screen.findByText("7-correct-horse");
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("copy_code"),
    );
  });

  it("shows approval progress on the trusted Mac instead of asking it to enter a nonexistent code", async () => {
    show({
      ...disconnected,
      phase: "pairing",
      pairingRole: "approver",
      hasStarted: true,
    });
    await screen.findByText(/Approving your new Mac/);
    expect(screen.queryByText(/On your trusted Mac/)).toBeNull();
    expect(screen.queryByText("Connecting…")).toBeNull();
    expect(screen.queryByRole("button", { name: "Copy code" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cancel pairing" }));
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith("cancel"),
    );
  });

  it("does not call an old successful connection up to date while reconciliation is pending", async () => {
    const client = show({
      ...disconnected,
      phase: "connected",
      hasStarted: true,
      lastSuccess: 1,
      upToDate: false,
    });
    await screen.findByText("Checking for changes…");
    expect(screen.queryByText("Up to date")).toBeNull();
    client.setQueryData(["sync-status"], {
      ...disconnected,
      phase: "connected",
      hasStarted: true,
      lastSuccess: 1,
      upToDate: true,
    });
    await screen.findByText("Up to date");
  });

  it("identifies this Mac and requires confirmation before revoking access", async () => {
    show({
      ...disconnected,
      phase: "connected",
      currentDeviceId: "device-1",
      devices: [
        {
          id: "device-1",
          name: "Office Mac",
          public_key: "key",
          enrolled_at: 1,
          revoked_at: null,
        },
      ],
    });
    const revoke = await screen.findByRole("button", { name: "Revoke access" });
    expect(screen.getByText(/This Mac/)).toBeTruthy();
    fireEvent.click(revoke);
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByRole("heading", { name: /Office Mac/ }),
    ).toBeTruthy();
    expect(commands.syncAction).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(commands.syncAction).not.toHaveBeenCalled();
    fireEvent.click(revoke);
    fireEvent.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "Revoke access",
      }),
    );
    await waitFor(() =>
      expect(commands.syncAction).toHaveBeenCalledWith({
        revoke_device: { id: "device-1" },
      }),
    );
  });

  it.each(["local", "cloud"] as const)(
    "requires reviewing both versions again when the %s conflict revision changes",
    async (changedSide) => {
      vi.mocked(commands.syncAction).mockImplementation(async (action) => ({
        status: "ok",
        data: {
          versions: [],
          cancelled: false,
          preview:
            typeof action === "object" && "preview" in action
              ? {
                  title: "Planning",
                  text: action.preview.revision,
                  device_id: "device-1",
                  created_at: 1,
                  captured_at: 1,
                  files: ["notes.md"],
                  changed: ["notes.md"],
                  deleted: false,
                  missing_references: [],
                }
              : null,
        },
      }));
      const entity = { kind: "session" as const, id: "session-1" };
      const conflict = { entity, local: "saved-v1", cloud: "cloud-v1" };
      const status: SyncStatus = {
        ...disconnected,
        phase: "connected",
        hasStarted: true,
        conflicts: [conflict],
      };
      const client = show(status);
      fireEvent.click(
        await screen.findByRole("button", { name: "Review versions" }),
      );
      await screen.findByRole("button", { name: "Use cloud version" });
      expect(screen.getByText("saved-v1")).toBeTruthy();
      expect(screen.getByText("cloud-v1")).toBeTruthy();

      const changed = { ...conflict, [changedSide]: "new-v2" };
      const updated = { ...status, conflicts: [changed] };
      vi.mocked(commands.syncStatus).mockResolvedValue(updated);
      client.setQueryData(["sync-status"], updated);
      const review = await screen.findByRole("button", {
        name: "Review versions",
      });
      expect(
        screen.queryByRole("button", { name: "Use saved version" }),
      ).toBeNull();
      expect(
        screen.queryByRole("button", { name: "Use cloud version" }),
      ).toBeNull();
      expect(screen.queryByText("saved-v1")).toBeNull();
      expect(screen.queryByText("cloud-v1")).toBeNull();
      expect(commands.syncAction).not.toHaveBeenCalledWith(
        expect.objectContaining({ restore: expect.anything() }),
      );

      fireEvent.click(review);
      await screen.findByText("new-v2");
      fireEvent.click(
        screen.getByRole("button", {
          name:
            changedSide === "local" ? "Use saved version" : "Use cloud version",
        }),
      );
      await waitFor(() =>
        expect(commands.syncAction).toHaveBeenCalledWith({
          restore: { entity, revision: "new-v2" },
        }),
      );
    },
  );
});
