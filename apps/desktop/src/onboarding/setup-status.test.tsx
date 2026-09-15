import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  config: {
    current_stt_provider: "",
    current_stt_model: "",
    current_llm_provider: "",
    current_llm_model: "",
  },
  downloaded: false,
  downloading: false,
}));
vi.mock("~/shared/config", () => ({ useConfigValues: () => mocks.config }));
vi.mock("@hypr/plugin-local-stt", () => ({
  commands: {
    isModelDownloaded: async () => ({ status: "ok", data: mocks.downloaded }),
    isModelDownloading: async () => ({ status: "ok", data: mocks.downloading }),
  },
}));

import { SummarySetupStatus, TranscriptionSetupStatus } from "./setup-status";

function renderStatus() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <TranscriptionSetupStatus />
      <SummarySetupStatus />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  Object.assign(mocks.config, {
    current_stt_provider: "",
    current_stt_model: "",
    current_llm_provider: "",
    current_llm_model: "",
  });
  mocks.downloaded = false;
  mocks.downloading = false;
});
afterEach(cleanup);

it("reports skipped services as not set up", () => {
  renderStatus();
  expect(screen.getByText("Transcription not set up")).toBeTruthy();
  expect(screen.getByText("Summaries not enabled")).toBeTruthy();
});

it("does not mistake a selected model for a completed download", async () => {
  Object.assign(mocks.config, {
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-batch",
  });
  mocks.downloading = true;
  renderStatus();
  await screen.findByText("Transcription model downloading");
  expect(screen.queryByText("Transcription model downloaded")).toBeNull();
});

it("reports a failed or removed download as not set up", async () => {
  Object.assign(mocks.config, {
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-batch",
  });
  renderStatus();
  await screen.findByText("Transcription not set up");
});

it("reports downloaded transcription and a selected summary service separately", async () => {
  Object.assign(mocks.config, {
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-batch",
    current_llm_provider: "openai",
    current_llm_model: "example-model",
  });
  mocks.downloaded = true;
  renderStatus();
  await screen.findByText("Transcription model downloaded");
  expect(screen.getByText("Summary service selected")).toBeTruthy();
});
