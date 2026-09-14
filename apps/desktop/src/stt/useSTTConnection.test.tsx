import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, expect, test, vi } from "vitest";

import { useSTTConnection } from "./useSTTConnection";

const { config, commands } = vi.hoisted(() => ({
  config: {
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-batch",
  },
  commands: {
    isModelDownloaded: vi.fn(),
    getServerForModel: vi.fn(),
    downloadModel: vi.fn(),
    startServer: vi.fn(),
  },
}));

vi.mock("@hypr/plugin-local-stt", () => ({ commands, events: {} }));
vi.mock("~/shared/config", () => ({ useConfigValues: () => config }));

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderHook(useSTTConnection, {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  config.current_stt_provider = "fmtr";
  config.current_stt_model = "soniqo-parakeet-batch";
  commands.isModelDownloaded.mockResolvedValue({ status: "ok", data: false });
  commands.getServerForModel.mockResolvedValue({ status: "ok", data: null });
});

test("a migrated selection without its download waits for model setup", async () => {
  const { result } = setup();
  await waitFor(() => {
    expect(result.current.local.data?.status).toBe("not_downloaded");
  });
  expect(result.current.isLocalModel).toBe(true);
  expect(result.current.conn).toBeNull();
  expect(commands.isModelDownloaded).toHaveBeenCalledWith(
    "soniqo-parakeet-batch",
  );
  expect(commands.downloadModel).not.toHaveBeenCalled();
  expect(commands.startServer).not.toHaveBeenCalled();
});

test("switching from Soniqo to Whisper waits for the selected model's server", async () => {
  commands.isModelDownloaded.mockResolvedValue({ status: "ok", data: true });
  commands.getServerForModel.mockImplementation(async (model: string) => ({
    status: "ok",
    data:
      model === "soniqo-parakeet-batch"
        ? { status: "ready", url: "soniqo://local", model }
        : { status: "loading", url: null, model },
  }));
  const { result, rerender } = setup();
  await waitFor(() => {
    expect(result.current.conn?.model).toBe("soniqo-parakeet-batch");
  });
  config.current_stt_model = "QuantizedTiny";
  rerender();
  expect(result.current.conn).toBeNull();
  await waitFor(() => {
    expect(result.current.local.data?.status).toBe("loading");
  });
  expect(commands.getServerForModel).toHaveBeenCalledWith("QuantizedTiny");
});

test.each(["am-parakeet-v3", "soniqo-qwen3-small", "soniqo-qwen3-large"])(
  "an unmigrated %s selection cannot query or start a model",
  (model) => {
    config.current_stt_model = model;
    const { result } = setup();
    expect(result.current.isLocalModel).toBe(false);
    expect(result.current.conn).toBeNull();
    expect(commands.isModelDownloaded).not.toHaveBeenCalled();
    expect(commands.getServerForModel).not.toHaveBeenCalled();
    expect(commands.startServer).not.toHaveBeenCalled();
  },
);
