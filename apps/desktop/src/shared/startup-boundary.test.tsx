import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Toaster } from "@hypr/ui/components/ui/toast";

import { StartupBoundary } from "./startup-boundary";

const mocks = vi.hoisted(() => ({
  getStartupStatus: vi.fn(),
  startupHandler: null as null | ((event: { payload: any }) => void),
  selectFolder: vi.fn(),
  join: vi.fn(),
  classifyVaultDir: vi.fn(),
  setVaultBase: vi.fn(),
  relaunchNow: vi.fn(),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: { getStartupStatus: mocks.getStartupStatus },
  events: {
    startupProgress: {
      listen: vi.fn(async (handler) => {
        mocks.startupHandler = handler;
        return () => {};
      }),
    },
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.selectFolder,
}));

vi.mock("@tauri-apps/api/path", () => ({
  join: mocks.join,
}));

vi.mock("@hypr/plugin-settings", () => ({
  commands: {
    classifyVaultDir: mocks.classifyVaultDir,
    setVaultBase: mocks.setVaultBase,
  },
}));

vi.mock("./relaunch", () => ({
  relaunchNow: mocks.relaunchNow,
}));

describe("StartupBoundary", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({
        matches: false,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
      })),
    );
    mocks.startupHandler = null;
    mocks.getStartupStatus.mockResolvedValue(
      status(1, { kind: "scanning", sessions_found: 237 }),
    );
    mocks.selectFolder.mockResolvedValue(null);
    mocks.join.mockImplementation(async (...parts: string[]) =>
      parts.join("/"),
    );
    mocks.classifyVaultDir.mockResolvedValue({
      status: "ok",
      data: "empty_or_missing",
    });
    mocks.setVaultBase.mockResolvedValue({ status: "ok", data: null });
    mocks.relaunchNow.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("shows live scan counts without mounting the application", async () => {
    renderBoundary();

    expect(await screen.findByText("Scanning notes — 237 found")).toBeTruthy();
    expect(screen.queryByText("Application ready")).toBeNull();
  });

  it("mounts the application only after a ready event", async () => {
    renderBoundary();
    await screen.findByText("Scanning notes — 237 found");

    act(() => {
      mocks.startupHandler?.({
        payload: { status: status(2, { kind: "ready" }) },
      });
    });

    expect(await screen.findByText("Application ready")).toBeTruthy();
  });

  it("shows Google Drive guidance and recovery after five seconds", async () => {
    vi.useFakeTimers();
    mocks.getStartupStatus.mockResolvedValue(
      status(
        1,
        { kind: "scanning", sessions_found: 3 },
        "/Users/me/Library/CloudStorage/GoogleDrive-me/My Drive/vault",
        true,
      ),
    );

    renderBoundary();
    await act(async () => Promise.resolve());
    expect(
      screen.queryByText("Google Drive may still be downloading files"),
    ).toBeNull();

    act(() => vi.advanceTimersByTime(5000));

    expect(
      screen.getByText("Google Drive may still be downloading files"),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Switch vault" })).toBeTruthy();
  });

  it("switches to a safe child folder without reading the blocked vault", async () => {
    mocks.getStartupStatus.mockResolvedValue(
      status(1, { kind: "failed", message: "Drive unavailable" }),
    );
    mocks.selectFolder.mockResolvedValue("/Users/me/Documents");
    mocks.classifyVaultDir.mockResolvedValue({
      status: "ok",
      data: "other",
    });
    mocks.join.mockResolvedValue("/Users/me/Documents/Loofah");

    renderBoundary();
    fireEvent.click(
      await screen.findByRole("button", { name: "Switch vault" }),
    );

    await waitFor(() =>
      expect(mocks.setVaultBase).toHaveBeenCalledWith(
        "/Users/me/Documents/Loofah",
      ),
    );
    expect(mocks.relaunchNow).toHaveBeenCalledTimes(1);
  });

  it("keeps healthy notes available while displaying migration paths and reasons", async () => {
    mocks.getStartupStatus.mockResolvedValue({
      ...status(1, { kind: "ready" }),
      migrationIssues: [
        "sessions/Readable: destination sessions/s1 is occupied",
      ],
    });
    renderBoundary();
    expect(await screen.findByText("Application ready")).toBeTruthy();
    expect(
      await screen.findByText("Some notes could not be migrated"),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "sessions/Readable: destination sessions/s1 is occupied",
      ),
    ).toBeTruthy();
  });

  it("restarts the app when retrying a failed startup", async () => {
    mocks.getStartupStatus.mockResolvedValue(
      status(1, { kind: "failed", message: "Drive unavailable" }),
    );

    renderBoundary();
    fireEvent.click(await screen.findByRole("button", { name: "Try again" }));

    await waitFor(() => expect(mocks.relaunchNow).toHaveBeenCalledTimes(1));
  });
});

function renderBoundary() {
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <StartupBoundary>
        <div>Application ready</div>
      </StartupBoundary>
      <Toaster />
    </QueryClientProvider>,
  );
}

function status(
  revision: number,
  phase: any,
  vaultPath = "/Users/me/Documents/vault",
  isCloudStorage = false,
) {
  return { revision, vaultPath, isCloudStorage, phase };
}
