import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { VersionHistory } from "./version-history";

import { commands } from "~/types/tauri.gen";

const mocks = vi.hoisted(() => ({ syncStatus: vi.fn(), openNew: vi.fn() }));
vi.mock("./sync", () => ({ useSyncStatus: mocks.syncStatus }));
vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (select: (state: { openNew: typeof mocks.openNew }) => unknown) =>
    select({ openNew: mocks.openNew }),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: { syncAction: vi.fn() },
}));

const entity = { kind: "session" as const, id: "session-1" };
const restore = { restore: { entity, revision: "revision-1" } };

beforeEach(() => {
  vi.clearAllMocks();
  mocks.syncStatus.mockReturnValue({
    data: {
      phase: "connected",
      currentDeviceId: "device-1",
      devices: [{ id: "device-1", name: "Office Mac" }],
    },
  });
  vi.mocked(commands.syncAction).mockImplementation(async (action) => ({
    status: "ok",
    data: {
      versions:
        typeof action === "object" && "history" in action
          ? [
              {
                id: "revision-1",
                operation: "operation-1",
                device_id: "device-1",
                created_at: 1,
                current: 1,
                pinned: 0,
              },
            ]
          : [],
      preview:
        typeof action === "object" && "preview" in action
          ? {
              text: "Earlier note",
              title: "Weekly planning",
              device_id: "device-1",
              created_at: 1,
              files: ["notes.md"],
              changed: ["notes.md"],
              captured_at: 1,
              missing_references: [],
              deleted: false,
            }
          : null,
      cancelled: false,
    },
  }));
});
afterEach(cleanup);

async function openPreview(
  beforeRestore: () => Promise<void>,
  afterRestore: () => Promise<void>,
) {
  show(beforeRestore, afterRestore);
  fireEvent.click(await screen.findByRole("button", { name: /Synced/ }));
  await screen.findByText("Earlier note");
}

function show(
  beforeRestore: () => Promise<void> = vi.fn(async () => {}),
  afterRestore: () => Promise<void> = vi.fn(async () => {}),
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <VersionHistory
        entity={entity}
        open
        onOpenChange={vi.fn()}
        beforeRestore={beforeRestore}
        afterRestore={afterRestore}
      />
    </QueryClientProvider>,
  );
}

it("waits for editor writes before restoring and refreshes the session afterward", async () => {
  let finishWrites!: () => void;
  const beforeRestore = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finishWrites = resolve;
      }),
  );
  const afterRestore = vi.fn(async () => {});
  await openPreview(beforeRestore, afterRestore);
  fireEvent.click(
    screen.getByRole("button", { name: "Restore whole session" }),
  );
  await waitFor(() => expect(beforeRestore).toHaveBeenCalledOnce());
  expect(commands.syncAction).not.toHaveBeenCalledWith(restore);
  expect(afterRestore).not.toHaveBeenCalled();
  finishWrites();
  await waitFor(() => expect(afterRestore).toHaveBeenCalledOnce());
  expect(commands.syncAction).toHaveBeenCalledWith(restore);
});

it("keeps the current session when pending editor writes fail", async () => {
  const beforeRestore = vi.fn(async () => {
    throw new Error("Local note could not be saved");
  });
  const afterRestore = vi.fn(async () => {});
  await openPreview(beforeRestore, afterRestore);
  fireEvent.click(
    screen.getByRole("button", { name: "Restore whole session" }),
  );
  await screen.findByText("Local note could not be saved");
  expect(commands.syncAction).not.toHaveBeenCalledWith(restore);
  expect(afterRestore).not.toHaveBeenCalled();
});

it("takes an unenrolled user to Sync settings without requesting unavailable history", async () => {
  mocks.syncStatus.mockReturnValue({ data: { phase: "save_recovery_kit" } });
  show();
  fireEvent.click(
    await screen.findByRole("button", { name: "Open Sync settings" }),
  );
  expect(mocks.openNew).toHaveBeenCalledWith({
    type: "settings",
    state: { tab: "sync" },
  });
  expect(commands.syncAction).not.toHaveBeenCalled();
});

it("explains that an enrolled session has no cloud versions yet", async () => {
  vi.mocked(commands.syncAction).mockResolvedValue({
    status: "ok",
    data: { versions: [], preview: null, cancelled: false },
  });
  show();
  await screen.findByText(/No cloud versions yet/);
  expect(
    screen.queryByRole("button", { name: "Restore whole session" }),
  ).toBeNull();
});

it("labels the originating Mac and session components in ordinary language", async () => {
  await openPreview(
    vi.fn(async () => {}),
    vi.fn(async () => {}),
  );
  expect(
    screen.getByRole("button", { name: /Office Mac \(This Mac\)/ }),
  ).toBeTruthy();
  expect(screen.getByText("Includes: Notes")).toBeTruthy();
  expect(screen.queryByText(/notes\.md/)).toBeNull();
});

it("does not report an export as saved when the file picker was cancelled", async () => {
  await openPreview(
    vi.fn(async () => {}),
    vi.fn(async () => {}),
  );
  vi.mocked(commands.syncAction).mockResolvedValueOnce({
    status: "ok",
    data: { versions: [], preview: null, cancelled: true },
  });
  fireEvent.click(screen.getByRole("button", { name: "Export this version…" }));
  await waitFor(() =>
    expect(commands.syncAction).toHaveBeenCalledWith({
      export: { entity, revision: "revision-1" },
    }),
  );
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "Export this version…" })
        .hasAttribute("disabled"),
    ).toBe(false),
  );
  expect(
    screen.queryByText("Version exported to the folder you selected."),
  ).toBeNull();
  await screen.findByText("Export cancelled.");
  expect(screen.queryByText("Completed.")).toBeNull();
});

it("confirms history deletion and removes the obsolete restore preview after success", async () => {
  const original = vi.mocked(commands.syncAction).getMockImplementation()!;
  let deleted = false;
  vi.mocked(commands.syncAction).mockImplementation(async (action) => {
    if (typeof action === "object" && "purge_version" in action) {
      deleted = true;
      return { status: "ok", data: null };
    }
    const result = await original(action);
    if (
      result.status === "ok" &&
      result.data &&
      typeof action === "object" &&
      "history" in action
    ) {
      result.data.versions = deleted
        ? []
        : result.data.versions.map((version) => ({ ...version, current: 0 }));
    }
    return result;
  });
  await openPreview(
    vi.fn(async () => {}),
    vi.fn(async () => {}),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Delete this version from history…" }),
  );
  const confirmation = await screen.findByRole("dialog", { name: /Delete/ });
  const purge = { purge_version: { revision: "revision-1" } };
  expect(commands.syncAction).not.toHaveBeenCalledWith(purge);
  fireEvent.click(
    within(confirmation).getByRole("button", { name: "Delete version" }),
  );
  await waitFor(() => expect(commands.syncAction).toHaveBeenCalledWith(purge));
  await waitFor(() => expect(screen.queryByText("Earlier note")).toBeNull());
  expect(
    screen.queryByRole("button", { name: "Restore whole session" }),
  ).toBeNull();
  await screen.findByText("Version deleted from history.");
});
