import { beforeEach, expect, it, vi } from "vitest";

import type { NativeRemoteConnection } from "./remote";
import { remoteMedia } from "./remote-media";

const source = {
  metadata: { meta: { id: "note" } },
  transcript_revision: "before",
  transcript_present: true,
  recording: { session_id: "note", recording_sha256: "old" },
};
const status = (phase = "running") => ({
  phase,
  media: {
    session_id: "reserved-note",
    request_id: "original",
    state: phase === "receiving" ? "review" : phase,
  },
  declaration: { request_id: "original" },
  revision: "admission",
});
let connection: {
  info: { capabilities: { features: string[] } };
  exportMediaSource: ReturnType<typeof vi.fn>;
  mediaSubmissions: ReturnType<typeof vi.fn>;
  mediaRequest: ReturnType<typeof vi.fn>;
  request: ReturnType<typeof vi.fn>;
  submitMedia: ReturnType<typeof vi.fn>;
  recoverMedia: ReturnType<typeof vi.fn>;
  cancelMedia: ReturnType<typeof vi.fn>;
  resumeMediaSource: ReturnType<typeof vi.fn>;
};
const prepared = {
  input: { filename: "speech.wav", sha256: "a".repeat(64), size_bytes: 123 },
  source: { kind: "path" as const, path: "/selected/speech.wav" },
};
beforeEach(() => {
  connection = {
    info: {
      capabilities: {
        features: [
          "media_jobs",
          "media_source_export",
          "processing_settings",
          "conditional_summary_jobs",
          "generated_titles",
          "transcription_policy",
        ],
      },
    },
    exportMediaSource: vi.fn(async () => ({
      path: "/downloads/Loofah/Original.wav",
    })),
    mediaSubmissions: vi.fn(async () => []),
    mediaRequest: vi.fn(async (_vault, input) =>
      input.kind === "source" ? source : status(),
    ),
    request: vi.fn(async (input) =>
      input.kind === "processing_settings"
        ? {
            settings: {
              summary_model: { provider: "operator", model: "public" },
            },
          }
        : { model: "server-model", batch_enabled: true, availability: "ready" },
    ),
    submitMedia: vi.fn(async () => status()),
    recoverMedia: vi.fn(async () => status("receiving")),
    cancelMedia: vi.fn(async () => status("cancelled")),
    resumeMediaSource: vi.fn(async () => status()),
  };
});
const api = () =>
  remoteMedia(connection as unknown as NativeRemoteConnection, "company");

it("saves the original source by its original request without declaring or recovering work", async () => {
  const abort = new AbortController();
  expect(
    await api().exportSource!("original", { signal: abort.signal }),
  ).toEqual({ path: "/downloads/Loofah/Original.wav" });
  expect(connection.exportMediaSource).toHaveBeenCalledWith(
    "company",
    "original",
    { signal: abort.signal },
  );
  expect(connection.submitMedia).not.toHaveBeenCalled();
  expect(connection.recoverMedia).not.toHaveBeenCalled();
  expect(connection.mediaRequest).not.toHaveBeenCalled();
});

it("keeps original source export unavailable without the advertised capability", () => {
  connection.info.capabilities.features = ["media_jobs"];
  expect(api().exportSource).toBeUndefined();
});

it("imports raw audio on a notes-only server without requesting optional processing", async () => {
  connection.info.capabilities.features = ["media_jobs"];
  await api().importAudio(
    { sessionId: null, prepared },
    { requestId: "original" },
  );
  expect(connection.request).not.toHaveBeenCalled();
  expect(connection.submitMedia).toHaveBeenCalledWith(
    "company",
    expect.objectContaining({
      transcription: null,
      summary: null,
      operation: expect.objectContaining({
        kind: "import_audio",
        transcribe: false,
      }),
    }),
    prepared.source,
    { requestId: "original" },
  );
});

it("imports subtitles without requiring optional summary processing", async () => {
  connection.info.capabilities.features = ["media_jobs"];
  await api().importSubtitles("note", prepared, { requestId: "original" });
  expect(connection.request).not.toHaveBeenCalled();
  expect(connection.submitMedia).toHaveBeenCalledWith(
    "company",
    expect.objectContaining({
      transcription: null,
      summary: null,
      operation: expect.objectContaining({ kind: "import_subtitles" }),
    }),
    prepared.source,
    { requestId: "original" },
  );
});

it("keeps CPU-only transcription independent of a summary provider", async () => {
  connection.info.capabilities.features = [
    "media_jobs",
    "transcription_policy",
  ];
  await api().importAudio(
    { sessionId: null, prepared },
    { requestId: "original" },
  );
  expect(connection.request).toHaveBeenCalledExactlyOnceWith(
    { kind: "transcription_policy", vault_id: "company" },
    "transcription_policy",
    { requestId: "original" },
  );
  expect(connection.submitMedia.mock.calls[0][1]).toMatchObject({
    summary: null,
    transcription: { model: "server-model" },
    operation: { transcribe: true },
  });
});

it("preserves the title when optional generated titles are unsupported", async () => {
  connection.info.capabilities.features = [
    "media_jobs",
    "processing_settings",
    "conditional_summary_jobs",
  ];
  await api().importSubtitles("note", prepared, { requestId: "original" });
  expect(connection.submitMedia.mock.calls[0][1].summary.title_policy).toBe(
    "preserve",
  );
});

it("freezes the original server baseline, model and summary before submitting exact selected bytes", async () => {
  await api().importAudio(
    { sessionId: "note", prepared },
    { requestId: "original" },
  );
  expect(connection.submitMedia).toHaveBeenCalledWith(
    "company",
    {
      request_id: "original",
      operation: {
        kind: "import_audio",
        session_id: "note",
        expected: source,
        filename: "speech.wav",
        content_type: null,
        created_at: null,
        transcribe: true,
        source: { sha256: "a".repeat(64), size_bytes: 123 },
      },
      transcription: { model: "server-model", languages: [] },
      summary: {
        target: { kind: "summary" },
        title_policy: "generate_if_empty",
      },
    },
    prepared.source,
    { requestId: "original" },
  );
});

it("repeated submission only reads the original status without adopting fresh settings or source", async () => {
  connection.mediaSubmissions.mockResolvedValue([
    { declaration: { request_id: "original" } },
  ]);
  await api().importAudio(
    { sessionId: "note", prepared },
    { requestId: "original" },
  );
  expect(connection.mediaRequest).toHaveBeenCalledWith(
    "company",
    { kind: "status", request_id: "original" },
    "status",
    { requestId: "original" },
  );
  expect(connection.request).not.toHaveBeenCalled();
  expect(connection.submitMedia).not.toHaveBeenCalled();
  expect(connection.recoverMedia).not.toHaveBeenCalled();
});

it("keeps Receiving pending rather than reporting saved media", async () => {
  const value = await api().recover("original");
  expect(value.state).toBe("receiving");
  expect(value.session_id).toBe("reserved-note");
  expect(connection.resumeMediaSource).not.toHaveBeenCalled();
});

it("retains raw import support when server transcription is unavailable", async () => {
  connection.request.mockImplementation(async (input) =>
    input.kind === "processing_settings"
      ? { settings: { summary_model: null } }
      : { model: null, batch_enabled: false, availability: "unavailable" },
  );
  await api().importAudio(
    { sessionId: null, prepared },
    { requestId: "original" },
  );
  const declaration = connection.submitMedia.mock.calls[0][1];
  expect(declaration.operation.transcribe).toBe(false);
  expect(declaration.transcription).toBeNull();
  expect(declaration.summary).toBeNull();
});

it("requires a ready server model for explicit retranscription", async () => {
  connection.request.mockImplementation(async (input) =>
    input.kind === "processing_settings"
      ? { settings: { summary_model: null } }
      : { model: null, batch_enabled: false, availability: "unavailable" },
  );
  await expect(
    api().retranscribe(source.recording as never, { requestId: "original" }),
  ).rejects.toThrow("not ready");
  expect(connection.submitMedia).not.toHaveBeenCalled();
});

it("resumes source only through an explicit original-request operation", async () => {
  await api().resumeSource!("original", prepared);
  expect(connection.resumeMediaSource).toHaveBeenCalledWith(
    "company",
    "original",
    prepared.source,
    undefined,
  );
  expect(connection.submitMedia).not.toHaveBeenCalled();
});
