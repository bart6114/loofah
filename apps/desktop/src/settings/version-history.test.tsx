import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { VersionHistory } from "./version-history";

import { commands } from "~/types/tauri.gen";

vi.mock("~/types/tauri.gen", () => ({
  commands: { syncAction: vi.fn() },
}));

const entity = { kind: "session" as const, id: "session-1" };
const restore = { restore: { entity, revision: "revision-1" } };

beforeEach(() => {
  vi.clearAllMocks();
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
              files: ["notes.md"],
              changed: ["notes.md"],
              captured_at: 1,
              missing_references: [],
              deleted: false,
            }
          : null,
    },
  }));
});
afterEach(cleanup);

async function openPreview(
  beforeRestore: () => Promise<void>,
  afterRestore: () => Promise<void>,
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
  fireEvent.click(await screen.findByRole("button", { name: /Synced/ }));
  await screen.findByText("Earlier note");
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
