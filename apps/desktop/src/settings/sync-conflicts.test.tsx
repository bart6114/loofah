import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { ConflictReview } from "./sync-conflicts";

import { commands, type SyncEntity } from "~/types/tauri.gen";

vi.mock("~/types/tauri.gen", () => ({ commands: { syncAction: vi.fn() } }));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(commands.syncAction).mockImplementation(async (action) => ({
    status: "ok",
    data: {
      versions: [],
      cancelled: false,
      preview:
        typeof action === "object" && "preview" in action
          ? {
              title: "Planning",
              text:
                action.preview.revision === "saved-revision"
                  ? "Saved edits"
                  : "Cloud edits",
              device_id: "device-1",
              created_at: 1,
              captured_at: 1,
              files: ["notes.md", "audio.mp3"],
              changed: ["notes.md"],
              deleted: false,
              missing_references: [],
            }
          : null,
    },
  }));
});
afterEach(cleanup);

function show(entity: SyncEntity = { kind: "session", id: "session-1" }) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ConflictReview
        conflict={{ entity, local: "saved-revision", cloud: "cloud-revision" }}
      />
    </QueryClientProvider>,
  );
}

it("waits for each preview before requesting the next and requires review before resolving", async () => {
  const original = vi.mocked(commands.syncAction).getMockImplementation()!;
  let releaseSaved!: () => void;
  const saved = new Promise<void>((resolve) => {
    releaseSaved = resolve;
  });
  vi.mocked(commands.syncAction).mockImplementation(async (action) => {
    if (
      typeof action === "object" &&
      "preview" in action &&
      action.preview.revision === "saved-revision"
    )
      await saved;
    return original(action);
  });
  show();
  expect(
    screen.queryByRole("button", { name: "Use cloud version" }),
  ).toBeNull();
  expect(commands.syncAction).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Review versions" }));
  await screen.findByRole("button", { name: "Loading both versions…" });
  expect(commands.syncAction).toHaveBeenCalledTimes(1);
  releaseSaved();
  await screen.findByText("Saved edits");
  expect(screen.getByText("Cloud edits")).toBeTruthy();
  expect(screen.getByRole("heading", { name: "Planning" })).toBeTruthy();
  expect(screen.getAllByText("Includes: Notes, Recording")).toHaveLength(2);
  expect(screen.queryByText(/notes\.md/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Use cloud version" }));
  await waitFor(() =>
    expect(commands.syncAction).toHaveBeenCalledWith({
      restore: {
        entity: { kind: "session", id: "session-1" },
        revision: "cloud-revision",
      },
    }),
  );
});

it("explains the whole-vault effect of choosing a people version", async () => {
  show({ kind: "people" });
  expect(
    screen.getByText("This choice replaces all people in this vault."),
  ).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Review versions" }));
  fireEvent.click(
    await screen.findByRole("button", { name: "Use saved version" }),
  );
  await waitFor(() =>
    expect(commands.syncAction).toHaveBeenCalledWith({
      restore: { entity: { kind: "people" }, revision: "saved-revision" },
    }),
  );
});

it("keeps resolution unavailable when either version cannot be loaded", async () => {
  vi.mocked(commands.syncAction).mockResolvedValue({
    status: "error",
    error: "Cloud unavailable",
  });
  show();
  fireEvent.click(screen.getByRole("button", { name: "Review versions" }));
  await screen.findByRole("alert");
  expect(
    screen.queryByRole("button", { name: "Use saved version" }),
  ).toBeNull();
  expect(
    screen.queryByRole("button", { name: "Use cloud version" }),
  ).toBeNull();
  expect(
    screen
      .getByRole("button", { name: "Review versions" })
      .hasAttribute("disabled"),
  ).toBe(false);
});
