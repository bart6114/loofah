import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  arch: vi.fn(),
  platform: vi.fn(),
  isModelDownloaded: vi.fn(),
  isModelDownloading: vi.fn(),
  downloadModel: vi.fn(),
  progressListeners: [] as Array<(event: { payload: unknown }) => void>,
  activeDownloads: [] as Array<{
    model: string;
    displayName: string;
    progress: number;
  }>,
}));

vi.mock("@tauri-apps/plugin-os", () => ({
  arch: mocks.arch,
  platform: mocks.platform,
}));

vi.mock("@hypr/plugin-local-stt", () => ({
  commands: {
    isModelDownloaded: mocks.isModelDownloaded,
    isModelDownloading: mocks.isModelDownloading,
    downloadModel: mocks.downloadModel,
    cancelDownload: vi.fn(),
    deleteModel: vi.fn(),
  },
  events: {
    downloadProgressPayload: {
      listen: vi.fn((listener: (event: { payload: unknown }) => void) => {
        mocks.progressListeners.push(listener);
        return Promise.resolve(() => {});
      }),
    },
  },
}));

vi.mock("~/contexts/notifications", () => ({
  useNotifications: () => ({ activeDownloads: mocks.activeDownloads }),
}));

import { DiarizationStatus } from "./diarization-status";

function renderStatus() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <DiarizationStatus />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  mocks.platform.mockReturnValue("macos");
  mocks.arch.mockResolvedValue("aarch64");
  mocks.isModelDownloaded.mockResolvedValue({ status: "ok", data: false });
  mocks.isModelDownloading.mockResolvedValue({ status: "ok", data: false });
  mocks.downloadModel.mockResolvedValue({ status: "ok", data: null });
  mocks.progressListeners.length = 0;
  mocks.activeDownloads = [];
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("DiarizationStatus", () => {
  it("renders nothing on non-Apple-Silicon", async () => {
    mocks.arch.mockResolvedValue("x86_64");
    renderStatus();

    await waitFor(() => expect(mocks.arch).toHaveBeenCalled());
    expect(screen.queryByText("Speaker detection")).toBeNull();
  });

  it("shows the muted not-downloaded state by default", async () => {
    renderStatus();

    await screen.findByText("Speaker detection");
    await screen.findByText("Not downloaded");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it.each(["x86_64", "aarch64"])(
    "shows speaker model status on Windows %s",
    async (architecture) => {
      mocks.platform.mockReturnValue("windows");
      mocks.arch.mockResolvedValue(architecture);
      mocks.isModelDownloaded.mockResolvedValue({ status: "ok", data: true });
      renderStatus();

      await screen.findByText("Speaker detection");
      await screen.findByText("Ready");
    },
  );

  it("shows a ready check when the model is downloaded", async () => {
    mocks.isModelDownloaded.mockResolvedValue({ status: "ok", data: true });
    renderStatus();

    await screen.findByText("Ready");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("shows a spinner with percent while downloading", async () => {
    mocks.isModelDownloading.mockResolvedValue({ status: "ok", data: true });
    mocks.activeDownloads = [
      {
        model: "diarizer-fluid-community",
        displayName: "diarizer-fluid-community",
        progress: 42,
      },
    ];
    renderStatus();

    await screen.findByText("42%");
  });

  it("shows the error and retries the download on failure", async () => {
    renderStatus();

    await screen.findByText("Speaker detection");
    act(() => {
      for (const listener of mocks.progressListeners) {
        listener({
          payload: {
            model: "diarizer-fluid-community",
            status: { failed: "network unreachable" },
          },
        });
      }
    });

    await screen.findByText("Download failed");
    const retry = await screen.findByRole("button", { name: "Retry" });
    act(() => retry.click());

    await waitFor(() =>
      expect(mocks.downloadModel).toHaveBeenCalledWith(
        "diarizer-fluid-community",
      ),
    );
  });
});
