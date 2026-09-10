import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  config: {
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-batch",
    ai_language: "zh",
    spoken_languages: [] as string[],
  },
  listSupportedModels: vi.fn(),
  isSupportedLanguagesLive: vi.fn(),
  isSupportedLanguagesBatch: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
  dismiss: vi.fn(),
}));

vi.mock("~/shared/config", () => ({
  useConfigValues: () => mocks.config,
}));
vi.mock("./health", () => ({
  useConnectionHealth: () => ({ status: "success" }),
  HealthStatusIndicator: () => null,
}));
vi.mock("@hypr/plugin-local-stt", () => ({
  commands: { listSupportedModels: mocks.listSupportedModels },
}));
vi.mock("@hypr/plugin-transcription", () => ({
  commands: {
    isSupportedLanguagesLive: mocks.isSupportedLanguagesLive,
    isSupportedLanguagesBatch: mocks.isSupportedLanguagesBatch,
  },
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    warning: mocks.warning,
    info: mocks.info,
    dismiss: mocks.dismiss,
  },
}));

import { TranscriptionLanguageWarningToast } from "./select";

import { localSttKeys } from "~/stt/useLocalSttModel";

const models = [
  { key: "soniqo-parakeet-batch", display_name: "Soniqo Parakeet Batch" },
  {
    key: "soniqo-parakeet-streaming",
    display_name: "Soniqo Parakeet Streaming",
  },
  { key: "whisper-large-v3", display_name: "Whisper Large V3 (Multilingual)" },
];

function renderWarning() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const element = () => (
    <QueryClientProvider client={client}>
      <TranscriptionLanguageWarningToast />
    </QueryClientProvider>
  );
  const view = render(element());
  return { client, rerender: () => view.rerender(element()) };
}

beforeEach(() => {
  vi.clearAllMocks();
  Object.assign(mocks.config, {
    current_stt_model: "soniqo-parakeet-batch",
    ai_language: "zh",
    spoken_languages: [],
  });
  mocks.listSupportedModels.mockResolvedValue({ status: "ok", data: models });
  mocks.isSupportedLanguagesLive.mockImplementation(
    async (_provider: string, model: string, languages: string[]) => ({
      status: "ok",
      data:
        model === "soniqo-parakeet-streaming" &&
        languages.every((language) => language === "en"),
    }),
  );
  mocks.isSupportedLanguagesBatch.mockImplementation(
    async (_provider: string, model: string, languages: string[]) => ({
      status: "ok",
      data: model === "whisper-large-v3" || !languages.includes("zh"),
    }),
  );
});

afterEach(cleanup);

test("names the model when the main language alone is unsupported", async () => {
  renderWarning();

  await waitFor(() =>
    expect(mocks.warning).toHaveBeenCalledWith(
      expect.stringContaining("Soniqo Parakeet Batch can't transcribe Chinese"),
      expect.any(Object),
    ),
  );
  expect(mocks.isSupportedLanguagesBatch).toHaveBeenCalledWith(
    "fmtr",
    "soniqo-parakeet-batch",
    ["zh"],
  );
});

test("checks the main and additional languages together", async () => {
  mocks.config.ai_language = "en";
  mocks.config.spoken_languages = ["zh"];
  renderWarning();

  await waitFor(() => expect(mocks.warning).toHaveBeenCalled());
  expect(mocks.isSupportedLanguagesBatch).toHaveBeenCalledWith(
    "fmtr",
    "soniqo-parakeet-batch",
    ["en", "zh"],
  );
});

test("refreshes and clears the warning when only the main language changes", async () => {
  mocks.config.ai_language = "en";
  const view = renderWarning();
  await waitFor(() =>
    expect(mocks.isSupportedLanguagesBatch).toHaveBeenCalledWith(
      "fmtr",
      "soniqo-parakeet-batch",
      ["en"],
    ),
  );
  expect(mocks.warning).not.toHaveBeenCalled();

  mocks.config.ai_language = "zh";
  view.rerender();
  await waitFor(() => expect(mocks.warning).toHaveBeenCalled());
  mocks.dismiss.mockClear();

  mocks.config.ai_language = "en";
  view.rerender();
  await waitFor(() =>
    expect(mocks.dismiss).toHaveBeenCalledWith(
      "transcription-language-warning",
    ),
  );
});

test("uses an informational batch fallback for a Dutch main language with streaming", async () => {
  mocks.config.current_stt_model = "soniqo-parakeet-streaming";
  mocks.config.ai_language = "nl";
  renderWarning();

  await waitFor(() =>
    expect(mocks.info).toHaveBeenCalledWith(
      expect.stringContaining("Live transcription isn't available for Dutch"),
      expect.any(Object),
    ),
  );
  expect(mocks.warning).not.toHaveBeenCalled();
});

test("does not warn about Chinese when a multilingual Whisper model supports it", async () => {
  mocks.config.current_stt_model = "whisper-large-v3";
  const { client } = renderWarning();
  await waitFor(() => expect(client.isFetching()).toBe(0));

  expect(mocks.isSupportedLanguagesBatch).toHaveBeenCalledWith(
    "fmtr",
    "whisper-large-v3",
    ["zh"],
  );
  expect(mocks.warning).not.toHaveBeenCalled();
  expect(mocks.info).not.toHaveBeenCalled();
});

test("updates an existing toast when model display names become available", async () => {
  mocks.listSupportedModels.mockResolvedValue({ status: "ok", data: [] });
  const { client } = renderWarning();
  await waitFor(() =>
    expect(mocks.warning).toHaveBeenCalledWith(
      expect.stringContaining("soniqo-parakeet-batch can't transcribe Chinese"),
      expect.any(Object),
    ),
  );

  act(() => {
    client.setQueryData([...localSttKeys.all, "supported-models"], {
      status: "ok",
      data: models,
    });
  });
  await waitFor(() =>
    expect(mocks.warning).toHaveBeenLastCalledWith(
      expect.stringContaining("Soniqo Parakeet Batch can't transcribe Chinese"),
      expect.any(Object),
    ),
  );
});
