import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  config: { current_stt_provider: "", current_stt_model: "" },
  downloaded: false,
  downloading: false,
  downloads: [] as { model: string; progress: number }[],
  startDownload: vi.fn(),
  setSelection: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-os", () => ({ arch: () => "aarch64" }));
vi.mock("~/shared/config", () => ({ useConfigValues: () => mocks.config }));
vi.mock("~/settings/providers", () => ({
  useAiProvidersState: () => ({ isReady: true }),
}));
vi.mock("~/settings/queries", () => ({
  useSetSettingValues: () => mocks.setSelection,
}));
vi.mock("~/settings/ai/shared/persist-selection", () => ({
  PersistAiSelection: () => null,
}));
vi.mock("~/contexts/notifications", () => ({
  useNotifications: () => ({ activeDownloads: mocks.downloads }),
}));
vi.mock("./context", () => ({
  useSttSettings: () => ({
    startDownload: mocks.startDownload,
    queuedDownloads: [],
  }),
}));
vi.mock("./health", () => ({
  useConnectionHealth: () => ({ status: "success" }),
}));
vi.mock("./diarization-status", () => ({ DiarizationStatus: () => null }));
vi.mock("@hypr/plugin-local-stt", () => ({
  commands: {
    listSupportedModels: async () => ({
      status: "ok",
      data: [
        {
          key: "soniqo-parakeet-batch",
          display_name: "Parakeet",
          model_type: "soniqo",
          size_bytes: 1024 * 1024 * 600,
        },
      ],
    }),
    isModelDownloaded: async () => ({ status: "ok", data: mocks.downloaded }),
    isModelDownloading: async () => ({ status: "ok", data: mocks.downloading }),
  },
}));

import { TooltipProvider } from "@hypr/ui/components/ui/tooltip";

import { SelectProviderAndModel } from "./select";

function renderSettings() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <SelectProviderAndModel showAlerts={false} />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  Object.assign(mocks.config, {
    current_stt_provider: "",
    current_stt_model: "",
  });
  mocks.downloaded = false;
  mocks.downloading = false;
  mocks.downloads = [];
});
afterEach(cleanup);

it("offers a direct first download without choosing a provider or opening alternatives", async () => {
  renderSettings();
  const download = await screen.findByRole("button", {
    name: "Download transcription model · ~600 MB",
  });
  expect(screen.queryByText("Select a provider")).toBeNull();
  expect(
    screen.getByText("Choose another model").closest("details")?.open,
  ).toBe(false);
  fireEvent.click(download);
  expect(mocks.setSelection).toHaveBeenCalledWith({
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-batch",
  });
  expect(mocks.startDownload).toHaveBeenCalledWith("soniqo-parakeet-batch");
});

it("shows download progress and prevents another primary download", async () => {
  mocks.downloading = true;
  mocks.downloads = [{ model: "soniqo-parakeet-batch", progress: 43 }];
  renderSettings();
  await screen.findByText("Downloading · 43%");
  expect(screen.getByRole("progressbar").getAttribute("value")).toBe("43");
  expect(
    screen.queryByRole("button", { name: /Download transcription model/ }),
  ).toBeNull();
});

it("recognizes a download already running before settings opened", async () => {
  mocks.downloading = true;
  renderSettings();
  await screen.findByText("Downloading transcription model…");
  expect(
    screen.queryByRole("button", { name: /Download transcription model/ }),
  ).toBeNull();
});

it("shows readiness only after the model is downloaded", async () => {
  mocks.downloaded = true;
  Object.assign(mocks.config, {
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-batch",
  });
  renderSettings();
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("Downloaded and ready"),
  );
  expect(
    screen.queryByRole("button", { name: /Download transcription model/ }),
  ).toBeNull();
});
