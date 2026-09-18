import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@hypr/ui/components/ui/tooltip";

const mocks = vi.hoisted(() => ({ storage: vi.fn(), vault: vi.fn() }));
vi.mock("~/types/tauri.gen", () => ({
  commands: { vaultStorageStats: mocks.storage },
}));
vi.mock("@hypr/plugin-settings", () => ({
  commands: { vaultBase: mocks.vault },
}));
vi.mock("@lingui/react", () => {
  const translate = ({
    message,
    id,
    values = {},
  }: {
    message?: string;
    id?: string;
    values?: Record<string, unknown>;
  }) =>
    (message ?? id ?? "").replace(/\{(\w+)\}/g, (_, key: string) =>
      String(values[key] ?? ""),
    );
  return {
    Trans: (props: {
      message?: string;
      id?: string;
      values?: Record<string, unknown>;
      children?: ReactNode;
    }) => <>{props.children ?? translate(props)}</>,
    useLingui: () => ({
      _: translate,
      i18n: {
        _: translate,
        locale: "en",
        number: (value: number) => value.toLocaleString(),
        date: (value: Date) => value.toISOString(),
      },
    }),
  };
});

import {
  formatStorageBytes,
  formatStorageShare,
  StorageSection,
} from "./about-storage";

import type { VaultStorageStats } from "~/types/tauri.gen";

const result: VaultStorageStats = {
  total_bytes: 1000,
  files: 3,
  trash_bytes: 200,
  trash_files: 1,
  unreadable_entries: 0,
  skipped_links: 0,
  scan_limited: false,
  measured_at: "2026-09-16T10:00:00Z",
  categories: [
    { category: "mp3", bytes: 800, files: 2 },
    { category: "markdown", bytes: 200, files: 1 },
    ...(["wav", "images", "pdf", "json", "other"] as const).map((category) => ({
      category,
      bytes: 0,
      files: 0,
    })),
  ],
};

function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <StorageSection />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

afterEach(cleanup);
beforeEach(() => {
  vi.clearAllMocks();
  mocks.vault.mockResolvedValue({ status: "ok", data: "/test/vault" });
  mocks.storage.mockResolvedValue({ status: "ok", data: result });
});

describe("vault storage", () => {
  it("keeps zero categories and shows trash as an included subset", async () => {
    mount();
    const table = await screen.findByRole("table");
    expect(within(table).getAllByRole("row")).toHaveLength(8);
    expect(
      within(table).getByRole("row", { name: /WAV audio/ }).textContent,
    ).toContain("0 B");
    expect(screen.getByText("Trash (included)")).toBeTruthy();
    expect(mocks.storage).toHaveBeenCalledTimes(1);
    expect(mocks.storage).toHaveBeenCalledWith(false);
  });

  it("retains measured rows during refresh and a failed refresh", async () => {
    mount();
    await screen.findByRole("table");
    let complete!: (value: unknown) => void;
    mocks.storage.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(mocks.storage).toHaveBeenLastCalledWith(true));
    expect(screen.getByRole("table")).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Updating…" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    complete({ status: "error", error: "offline volume" });
    await screen.findByText(
      "Couldn’t update storage. Showing the previous measurement.",
    );
    expect(screen.getByRole("table")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("does not present an incomplete zero measurement as an empty vault", async () => {
    mocks.storage.mockResolvedValue({
      status: "ok",
      data: { ...result, total_bytes: 0, files: 0, unreadable_entries: 1 },
    });
    mount();
    await screen.findByText("Measured vault size");
    expect(screen.queryByText("No files in this vault yet.")).toBeNull();
    expect(
      screen.getByText(
        "Some files couldn’t be measured. Shares show measured files only.",
      ),
    ).toBeTruthy();
  });

  it("reports an initial error and can retry without inventing a zero total", async () => {
    mocks.storage.mockResolvedValueOnce({
      status: "error",
      error: "missing vault",
    });
    mount();
    await screen.findByText("Couldn’t calculate vault storage.");
    expect(screen.queryByRole("table")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByRole("table");
  });

  it("uses decimal units and preserves tiny nonzero shares", () => {
    expect(formatStorageBytes(0)).toBe("0 B");
    expect(formatStorageBytes(4000000)).toBe("4 MB");
    expect(formatStorageBytes(2400000000)).toBe("2.4 GB");
    expect(formatStorageShare(1, 1000000)).toBe("<0.1%");
    expect(formatStorageShare(0, 0)).toBe("0%");
  });
});
