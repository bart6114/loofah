import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { IphoneCaptureWorker } from "./iphone-capture-worker";

import type { CaptureStage } from "~/types/tauri.gen";

const mocks = vi.hoisted(() => ({
  jobs: [] as [string, CaptureStage][],
  settingsReady: true,
  llmReady: true,
  sttReady: true,
  iphoneCaptureJobs: vi.fn(),
  iphoneCaptureAdvance: vi.fn(),
  iphoneCaptureAudio: vi.fn(),
  sessionHasTranscript: vi.fn(),
  runBatch: vi.fn(),
  finishImportedCaptureSummary: vi.fn(),
}));

vi.mock("~/types/tauri.gen", () => ({ commands: mocks }));
vi.mock("~/shared/index-query", async () => ({
  useIndexQuery: (await import("@tanstack/react-query")).useQuery,
}));
vi.mock("~/shared/config/store", () => ({
  useConfigStateSelector: () => mocks.settingsReady,
}));
vi.mock("~/session/queries", () => ({
  useSession: (id: string) => (id ? { id } : null),
}));
vi.mock("~/session/audio-operations", () => ({
  enqueueSessionAudioOperation: (_id: string, run: () => Promise<void>) =>
    run(),
}));
vi.mock("~/services/enhancer", () => ({
  getEnhancerService: () => ({
    finishImportedCaptureSummary: mocks.finishImportedCaptureSummary,
  }),
}));
vi.mock("~/ai/hooks/useLLMConnection", () => ({
  useLLMConnection: () => ({
    conn: mocks.llmReady ? {} : null,
    status: mocks.llmReady
      ? { status: "ready" }
      : { status: "pending", reason: "missing_provider" },
  }),
}));
vi.mock("./useSTTConnection", () => ({
  useSTTConnection: () => ({ conn: mocks.sttReady ? {} : null }),
}));
vi.mock("./useRunBatch", () => ({ useRunBatch: () => mocks.runBatch }));

function ok<T>(data: T) {
  return { status: "ok", data };
}

function renderWorker() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const worker = () => (
    <QueryClientProvider client={client}>
      <IphoneCaptureWorker />
    </QueryClientProvider>
  );
  const view = render(worker());
  return () => view.rerender(worker());
}

describe("iPhone capture processing", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.settingsReady = true;
    mocks.llmReady = true;
    mocks.sttReady = true;
    mocks.iphoneCaptureJobs.mockImplementation(async () => ok(mocks.jobs));
    mocks.iphoneCaptureAdvance.mockImplementation(
      async (id: string, stage: CaptureStage) => {
        mocks.jobs = stage === "transcribe" ? [[id, "summarize"]] : [];
        return ok(null);
      },
    );
    mocks.iphoneCaptureAudio.mockResolvedValue(ok("/vault/audio.wav"));
    mocks.sessionHasTranscript.mockResolvedValue(ok(false));
    mocks.finishImportedCaptureSummary.mockResolvedValue(true);
  });

  afterEach(cleanup);

  it("persists transcription before starting the summary", async () => {
    mocks.jobs = [["recording-1", "transcribe"]];
    renderWorker();

    await waitFor(() =>
      expect(mocks.iphoneCaptureJobs).toHaveBeenCalledTimes(3),
    );
    expect(mocks.runBatch).toHaveBeenCalledExactlyOnceWith("/vault/audio.wav", {
      imported: true,
      persistAfterCompletion: true,
    });
    expect(mocks.iphoneCaptureAdvance).toHaveBeenNthCalledWith(
      1,
      "recording-1",
      "transcribe",
    );
    expect(mocks.runBatch).toHaveBeenCalledBefore(
      mocks.finishImportedCaptureSummary,
    );
    expect(mocks.finishImportedCaptureSummary).toHaveBeenCalledExactlyOnceWith(
      "recording-1",
    );
    expect(mocks.iphoneCaptureAdvance).toHaveBeenNthCalledWith(
      2,
      "recording-1",
      "summarize",
    );
  });

  it("waits for settings and summarizes a written capture without transcription", async () => {
    mocks.jobs = [["note-1", "summarize"]];
    mocks.settingsReady = false;
    mocks.llmReady = false;
    mocks.sttReady = false;
    const rerender = renderWorker();

    await waitFor(() =>
      expect(mocks.iphoneCaptureJobs).toHaveBeenCalledTimes(1),
    );
    expect(mocks.iphoneCaptureAdvance).not.toHaveBeenCalled();

    mocks.settingsReady = true;
    mocks.llmReady = true;
    rerender();
    await waitFor(() =>
      expect(mocks.iphoneCaptureJobs).toHaveBeenCalledTimes(2),
    );
    expect(mocks.finishImportedCaptureSummary).toHaveBeenCalledExactlyOnceWith(
      "note-1",
    );
    expect(mocks.iphoneCaptureAdvance).toHaveBeenCalledExactlyOnceWith(
      "note-1",
      "summarize",
    );
    expect(mocks.runBatch).not.toHaveBeenCalled();
  });
});
