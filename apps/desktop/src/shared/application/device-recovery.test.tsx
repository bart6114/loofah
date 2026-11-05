import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  draftCacheInventory: vi.fn(),
  draftCacheRecovery: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import { DeviceRecovery } from "./device-recovery";

const entry = {
  id: "a".repeat(64),
  sessionId: '["template_output","deleted-note","deleted-output"]',
  title: "",
  uncertain: true,
};
const value = (scope: string) => ({
  scope,
  sessionId: entry.sessionId,
  baseline: { markdown: "Original", title: "", revision: "a" },
  draft: {
    markdown: "Unsaved output\n\n![image](file:///private/never-load.png)",
    title: "",
  },
  uncertain: true,
});

function open(scope = "server:alice:vault") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const result = render(
    <QueryClientProvider client={client}>
      <DeviceRecovery scope={scope} />
    </QueryClientProvider>,
  );
  return { client, ...result };
}

beforeEach(() => {
  vi.resetAllMocks();
  native.draftCacheInventory.mockResolvedValue({ status: "ok", data: [entry] });
  native.draftCacheRecovery.mockResolvedValue({
    status: "ok",
    data: value("server:alice:vault"),
  });
});
afterEach(cleanup);

describe("device recovery inventory", () => {
  it("opens orphaned output copies on demand and copies exact plain text without a vault operation", async () => {
    open();
    expect(native.draftCacheInventory).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Device recovery drafts" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /Template output draft/ }),
    );
    const markdown = await screen.findByRole("textbox", {
      name: "Recovered Markdown",
    });
    expect((markdown as HTMLTextAreaElement).value).toBe(
      value("server:alice:vault").draft.markdown,
    );
    expect((markdown as HTMLTextAreaElement).readOnly).toBe(true);
    expect(screen.queryByRole("img")).toBeNull();
    expect(native.draftCacheRecovery).toHaveBeenCalledWith(
      "server:alice:vault",
      entry.id,
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy saved text" }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(
        value("server:alice:vault").draft.markdown,
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "Reload drafts" }));
    await waitFor(() =>
      expect(native.draftCacheInventory).toHaveBeenCalledTimes(2),
    );
  });

  it("keeps device recovery usable while the browser reports offline", async () => {
    const { onlineManager } = await import("@tanstack/react-query");
    onlineManager.setOnline(false);
    try {
      open();
      fireEvent.click(
        screen.getByRole("button", { name: "Device recovery drafts" }),
      );
      fireEvent.click(
        await screen.findByRole("button", { name: /Template output draft/ }),
      );
      expect(
        (
          (await screen.findByRole("textbox", {
            name: "Recovered Markdown",
          })) as HTMLTextAreaElement
        ).value,
      ).toBe(value("server:alice:vault").draft.markdown);
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("keeps missing or damaged copies explicit and never replaces them with blank editable text", async () => {
    native.draftCacheRecovery.mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    open();
    fireEvent.click(
      screen.getByRole("button", { name: "Device recovery drafts" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /Template output draft/ }),
    );
    expect(
      await screen.findByText("This recovery copy has already been cleared."),
    ).not.toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    native.draftCacheInventory.mockResolvedValueOnce({
      status: "error",
      error: {
        code: "incompatible_version",
        message: "Future saved draft. Files kept.",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: null,
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Reload drafts" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "Future saved draft. Files kept.",
    );
  });

  it("rejects another account's recovery response and drops late content after teardown", async () => {
    native.draftCacheRecovery.mockResolvedValueOnce({
      status: "ok",
      data: value("server:bob:vault"),
    });
    const { client, unmount } = open();
    fireEvent.click(
      screen.getByRole("button", { name: "Device recovery drafts" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: /Template output draft/ }),
    );
    expect((await screen.findByRole("alert")).textContent).toContain(
      "different account or vault",
    );
    expect(screen.queryByRole("textbox")).toBeNull();
    let finish!: (value: unknown) => void;
    native.draftCacheRecovery.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Reload drafts" }));
    await waitFor(() =>
      expect(native.draftCacheRecovery).toHaveBeenCalledTimes(2),
    );
    unmount();
    client.clear();
    finish({ status: "ok", data: value("server:alice:vault") });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(
      client.getQueryData([
        "device_recovery",
        "server:alice:vault",
        "record",
        entry.id,
      ]),
    ).toBeUndefined();
  });
});
