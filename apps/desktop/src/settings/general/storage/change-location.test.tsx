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

const mocks = vi.hoisted(() => ({
  vaultBase: vi.fn(),
  relocateVault: vi.fn(),
  setVaultBase: vi.fn(),
  classifyVaultDir: vi.fn(),
  selectFolder: vi.fn(),
  message: vi.fn(),
  openPath: vi.fn(),
  scheduleAutomaticRelaunch: vi.fn(),
  toastError: vi.fn(),
  homeDir: vi.fn(),
}));

vi.mock("@hypr/plugin-settings", () => ({
  commands: {
    vaultBase: mocks.vaultBase,
    setVaultBase: mocks.setVaultBase,
    classifyVaultDir: mocks.classifyVaultDir,
  },
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: { relocateVault: mocks.relocateVault },
}));

vi.mock("@hypr/plugin-opener2", () => ({
  commands: { openPath: mocks.openPath },
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.toastError },
}));

vi.mock("@tauri-apps/api/path", () => ({
  homeDir: mocks.homeDir,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.selectFolder,
  message: mocks.message,
}));

vi.mock("~/shared/relaunch", () => ({
  scheduleAutomaticRelaunch: mocks.scheduleAutomaticRelaunch,
}));

vi.mock("@lingui/react/macro", () => ({
  Trans: ({ children }: { children?: ReactNode }) => <>{children}</>,
  useLingui: () => ({
    t: (strings: TemplateStringsArray, ...values: unknown[]) =>
      strings.reduce(
        (message, part, index) =>
          `${message}${part}${index < values.length ? String(values[index]) : ""}`,
        "",
      ),
  }),
}));

import { ChangeLocationRow } from "./change-location";

function renderRow() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });

  return {
    queryClient,
    ...render(
      <QueryClientProvider client={queryClient}>
        <ChangeLocationRow />
      </QueryClientProvider>,
    ),
  };
}

async function openDialogFor(path: string) {
  mocks.selectFolder.mockResolvedValue(path);
  renderRow();
  await screen.findByText(/Drive\/vault/);
  fireEvent.click(screen.getByRole("button", { name: /change/i }));
  return await screen.findByRole("dialog");
}

describe("ChangeLocationRow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.vaultBase.mockResolvedValue({
      status: "ok",
      data: "/Users/x/Drive/vault",
    });
    mocks.relocateVault.mockResolvedValue({ status: "ok", data: null });
    mocks.setVaultBase.mockResolvedValue({ status: "ok", data: null });
    mocks.classifyVaultDir.mockResolvedValue({
      status: "ok",
      data: "empty_or_missing",
    });
    mocks.homeDir.mockResolvedValue("/Users/x");
    mocks.scheduleAutomaticRelaunch.mockResolvedValue("scheduled");
  });

  afterEach(() => {
    cleanup();
  });

  it("shows the current vault path and a change button", async () => {
    renderRow();
    expect(await screen.findByText(/Drive\/vault/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /change/i })).toBeTruthy();
  });

  it("moves the vault into an empty folder by default", async () => {
    const dialog = await openDialogFor("/Users/x/Desktop/new-vault");

    await within(dialog).findByText(/Use this folder for your vault/);
    fireEvent.click(
      within(dialog).getByRole("button", { name: /^move my vault$/i }),
    );

    await waitFor(() =>
      expect(mocks.relocateVault).toHaveBeenCalledWith(
        "/Users/x/Desktop/new-vault",
        false,
      ),
    );
    await waitFor(() =>
      expect(mocks.scheduleAutomaticRelaunch).toHaveBeenCalledTimes(1),
    );
  });

  it("copies when the copy choice is selected", async () => {
    const dialog = await openDialogFor("/Users/x/Desktop/new-vault");

    fireEvent.click(
      await within(dialog).findByRole("radio", { name: /copy my vault here/i }),
    );
    fireEvent.click(
      within(dialog).getByRole("button", { name: /^copy my vault$/i }),
    );

    await waitFor(() =>
      expect(mocks.relocateVault).toHaveBeenCalledWith(
        "/Users/x/Desktop/new-vault",
        true,
      ),
    );
  });

  it("starts a fresh vault via a plain switch when chosen", async () => {
    const dialog = await openDialogFor("/Users/x/Desktop/new-vault");

    fireEvent.click(
      await within(dialog).findByRole("radio", {
        name: /start a new empty vault here/i,
      }),
    );
    fireEvent.click(
      within(dialog).getByRole("button", { name: /^start new vault$/i }),
    );

    await waitFor(() =>
      expect(mocks.setVaultBase).toHaveBeenCalledWith(
        "/Users/x/Desktop/new-vault",
      ),
    );
    expect(mocks.relocateVault).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(mocks.scheduleAutomaticRelaunch).toHaveBeenCalledTimes(1),
    );
  });

  it("offers a plain switch when the folder already contains a vault", async () => {
    mocks.classifyVaultDir.mockResolvedValue({ status: "ok", data: "vault" });
    const dialog = await openDialogFor("/Users/x/Documents/work-vault");

    await within(dialog).findByText(/Switch to this vault/);
    fireEvent.click(within(dialog).getByRole("button", { name: /^switch$/i }));

    await waitFor(() =>
      expect(mocks.setVaultBase).toHaveBeenCalledWith(
        "/Users/x/Documents/work-vault",
      ),
    );
    expect(mocks.relocateVault).not.toHaveBeenCalled();
  });

  it("targets a fresh subfolder when the folder has unrelated files", async () => {
    mocks.classifyVaultDir.mockResolvedValue({ status: "ok", data: "other" });
    const dialog = await openDialogFor("/Users/x/Google Drive/My Drive");

    await within(dialog).findByText(/already has files/);
    fireEvent.click(
      within(dialog).getByRole("button", { name: /^move my vault$/i }),
    );

    await waitFor(() =>
      expect(mocks.relocateVault).toHaveBeenCalledWith(
        "/Users/x/Google Drive/My Drive/Loofah",
        false,
      ),
    );
  });

  it("starts a fresh vault in a subfolder when the folder has unrelated files", async () => {
    mocks.classifyVaultDir.mockResolvedValue({ status: "ok", data: "other" });
    const dialog = await openDialogFor("/Users/x/Google Drive/My Drive");

    fireEvent.click(
      await within(dialog).findByRole("radio", {
        name: /start a new empty vault here/i,
      }),
    );
    fireEvent.click(
      within(dialog).getByRole("button", { name: /^start new vault$/i }),
    );

    await waitFor(() =>
      expect(mocks.setVaultBase).toHaveBeenCalledWith(
        "/Users/x/Google Drive/My Drive/Loofah",
      ),
    );
    expect(mocks.relocateVault).not.toHaveBeenCalled();
  });

  it("surfaces a backend failure as a toast and inline error", async () => {
    mocks.relocateVault.mockResolvedValue({
      status: "error",
      error: "New location is a subdirectory of the current vault",
    });
    const dialog = await openDialogFor("/Users/x/Drive/vault/nested");

    fireEvent.click(
      await within(dialog).findByRole("button", { name: /^move my vault$/i }),
    );

    await waitFor(() =>
      expect(
        within(dialog).getByText(
          "New location is a subdirectory of the current vault",
        ),
      ).toBeTruthy(),
    );
    expect(mocks.toastError).toHaveBeenCalledWith(
      "New location is a subdirectory of the current vault",
    );
    expect(mocks.scheduleAutomaticRelaunch).not.toHaveBeenCalled();
  });
});
