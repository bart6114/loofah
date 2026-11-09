import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  deviceTranscriptionSubmissions: vi.fn(),
  deviceTranscriptionSubmissionPrepare: vi.fn(),
  deviceTranscriptionSubmissionGet: vi.fn(),
  deviceTranscriptionSubmissionObserve: vi.fn(),
  copy: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: native.copy,
}));

import type { TranscriptionJobSnapshot } from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultScope } from "./vault-scope";
import { VaultTranscription } from "./vault-transcription";

import type { TranscriptionSubmissionReceipt } from "~/types/tauri.gen";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});
beforeEach(() => {
  vi.resetAllMocks();
  native.copy.mockResolvedValue(undefined);
});

async function fixture({ canEdit = true } = {}) {
  let event!: (value: VaultClientEvent) => void;
  const data = {
    receipts: [] as TranscriptionSubmissionReceipt[],
    jobs: [] as TranscriptionJobSnapshot[],
  };
  const job: TranscriptionJobSnapshot = {
    id: "job",
    session_id: "note",
    state: "queued",
    phase: null,
    created_at: "2026-11-08T10:00:00Z",
    updated_at: "2026-11-08T10:00:00Z",
    failure: null,
    result_available: false,
    publication: null,
  };
  const receipt: TranscriptionSubmissionReceipt = {
    scope: "scope",
    request_id: crypto.randomUUID(),
    created_at_ms: 1,
    input: {
      session_id: "note",
      expected_transcript: "absent",
      expected_recording_sha256: "a".repeat(64),
      model: "original",
    },
    job: null,
  };
  native.deviceTranscriptionSubmissions.mockImplementation(async () => ({
    status: "ok",
    data: structuredClone(data.receipts),
  }));
  native.deviceTranscriptionSubmissionGet.mockImplementation(async () => ({
    status: "ok",
    data: structuredClone(data.receipts[0]),
  }));
  native.deviceTranscriptionSubmissionObserve.mockImplementation(
    async (_scope, _id, value) => {
      data.receipts[0].job = value;
      return { status: "ok", data: structuredClone(data.receipts[0]) };
    },
  );
  const api = {
    policy: vi.fn(async () => ({
      batch_enabled: true,
      live_enabled: false,
      model: "original",
      availability: "ready",
    })),
    list: vi.fn(async () => ({
      jobs: structuredClone(data.jobs),
      next_cursor: null,
    })),
    get: vi.fn(async () => ({ ...job, state: "succeeded" })),
    submit: vi.fn(async () => job),
    cancel: vi.fn(async () => ({ ...job, state: "cancelled" })),
    resumeResult: vi.fn(async () => job),
    result: vi.fn(async () => ({
      job_id: job.id,
      session_id: "note",
      transcript: {
        id: "transcript",
        session_id: "note",
        words: [
          { text: "Retained", start_ms: 0, end_ms: 100, channel: 0 },
          { text: "speech", start_ms: 100, end_ms: 200, channel: 0 },
        ],
      },
    })),
  };
  const client = {
    scope: "scope",
    canEdit,
    networkMode: "always",
    features: ["batch_transcription_jobs", "transcription_policy"],
    transcriptionJobs: api,
    audioMetadata: vi.fn(async () => ({
      filename: "audio.wav",
      contentType: "audio/wav",
      sizeBytes: 1000,
      sha256: "a".repeat(64),
    })),
    subscribe: async (listener: (value: VaultClientEvent) => void) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: "always" },
    mutations: { retry: false, networkMode: "always" },
  });
  scopes.push(scope);
  await scope.start();
  return {
    scope,
    client,
    api,
    data,
    job,
    receipt,
    emit: (value: VaultClientEvent) => event(value),
  };
}

it("loads unknown requests without dispatch and recovers the same request only on click", async () => {
  const f = await fixture();
  f.data.receipts.push(f.receipt);
  render(
    <VaultTranscription
      scope={f.scope}
      sessionId="note"
      hasTranscript={false}
    />,
  );
  const recover = await screen.findByRole("button", {
    name: "Recover request",
  });
  expect(f.api.submit).not.toHaveBeenCalled();
  expect(
    (
      screen.getByRole("button", {
        name: "Transcribe saved recording",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  fireEvent.click(recover);
  await waitFor(() =>
    expect(f.api.submit).toHaveBeenCalledWith(
      f.receipt.input,
      expect.objectContaining({ requestId: f.receipt.request_id }),
    ),
  );
  expect(native.deviceTranscriptionSubmissionPrepare).not.toHaveBeenCalled();
});

it("refreshes acknowledged active receipts outside the loaded history page without replay", async () => {
  const f = await fixture();
  f.data.receipts.push({ ...f.receipt, job: f.job });
  render(<VaultTranscription scope={f.scope} sessionId="note" hasTranscript />);
  expect(await screen.findByText("Transcript saved")).toBeTruthy();
  expect(f.api.get).toHaveBeenCalledWith(
    f.job.id,
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(f.api.submit).not.toHaveBeenCalled();
});

it("cancels only the selected session's admitted job", async () => {
  const f = await fixture();
  f.data.jobs.push(f.job, { ...f.job, id: "foreign", session_id: "other" });
  render(<VaultTranscription scope={f.scope} sessionId="note" hasTranscript />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Cancel transcription" }),
  );
  await waitFor(() =>
    expect(f.api.cancel).toHaveBeenCalledWith(
      "job",
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    ),
  );
  expect(f.api.submit).not.toHaveBeenCalled();
});

it("keeps conflicting results available to copy without automatically applying or generating", async () => {
  const f = await fixture({ canEdit: false });
  f.data.jobs.push({ ...f.job, state: "conflict", result_available: true });
  render(<VaultTranscription scope={f.scope} sessionId="note" hasTranscript />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Copy saved transcript" }),
  );
  await waitFor(() =>
    expect(native.copy).toHaveBeenCalledWith("Retained speech"),
  );
  expect(f.api.resumeResult).not.toHaveBeenCalled();
  expect(f.api.submit).not.toHaveBeenCalled();
});

it("does not copy private results after the original scope loses access", async () => {
  const f = await fixture();
  f.data.jobs.push({ ...f.job, state: "conflict", result_available: true });
  let release!: (value: Awaited<ReturnType<typeof f.api.result>>) => void;
  f.api.result.mockImplementation(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const result = {
    job_id: "job",
    session_id: "note",
    transcript: { id: "transcript", session_id: "note", words: [] },
  };
  render(<VaultTranscription scope={f.scope} sessionId="note" hasTranscript />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Copy saved transcript" }),
  );
  await waitFor(() => expect(f.api.result).toHaveBeenCalled());
  act(() =>
    f.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await act(async () => release(result));
  expect(native.copy).not.toHaveBeenCalled();
});

it("does not offer transcription on a note without a recording", async () => {
  const f = await fixture();
  vi.mocked(f.client.audioMetadata).mockResolvedValue(null);
  render(
    <VaultTranscription
      scope={f.scope}
      sessionId="note"
      hasTranscript={false}
    />,
  );
  await waitFor(() => expect(f.client.audioMetadata).toHaveBeenCalled());
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Transcribe saved recording" }),
    ).toBeNull(),
  );
  expect(f.api.submit).not.toHaveBeenCalled();
});
