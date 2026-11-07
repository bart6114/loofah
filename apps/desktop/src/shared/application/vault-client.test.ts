import { beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  commands: {
    applicationScope: vi.fn(),
    applicationProcessingAvailable: vi.fn(),
    applicationProcessingPolicy: vi.fn(),
    applicationProcessingSettings: vi.fn(),
    applicationSaveProcessingSettings: vi.fn(),
    applicationSubmitSummaryJob: vi.fn(),
    applicationProcessingJobs: vi.fn(),
    applicationProcessingJob: vi.fn(),
    applicationProcessingJobResult: vi.fn(),
    applicationCancelProcessingJob: vi.fn(),
    applicationAgentList: vi.fn(),
    applicationAgentSearch: vi.fn(),
    applicationAgentMeeting: vi.fn(),
    applicationAgentTranscript: vi.fn(),
    applicationAgentExport: vi.fn(),
    applicationAgentSegments: vi.fn(),
    applicationReadNote: vi.fn(),
    applicationReadNoteEditor: vi.fn(),
    applicationSaveNoteEditor: vi.fn(),
    applicationReadMetadata: vi.fn(),
    applicationSaveMetadata: vi.fn(),
    applicationSaveNote: vi.fn(),
    applicationCreateSession: vi.fn(),
    applicationDeleteEmptySession: vi.fn(),
    applicationSessions: vi.fn(),
    applicationSessionSummaries: vi.fn(),
    applicationSession: vi.fn(),
    applicationSearch: vi.fn(),
    applicationDocuments: vi.fn(),
    applicationReadDocument: vi.fn(),
    applicationSaveDocument: vi.fn(),
    applicationTranscripts: vi.fn(),
    applicationAudioMetadata: vi.fn(),
    applicationAttachmentInfo: vi.fn(),
    applicationAttachmentMetadata: vi.fn(),
    devicePrepareAttachmentUpload: vi.fn(),
    applicationUploadAttachmentSource: vi.fn(),
    applicationAttachmentUploadStatus: vi.fn(),
    applicationRemoveAttachment: vi.fn(),
    applicationAttachments: vi.fn(),
    applicationDeleteRecording: vi.fn(),
    applicationTasks: vi.fn(),
    applicationTasksSnapshot: vi.fn(),
    applicationReplaceTasks: vi.fn(),
    applicationPeople: vi.fn(),
    applicationTags: vi.fn(),
  },
  listen: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: native.commands,
  events: { applicationChanged: { listen: native.listen } },
}));

import { ApplicationRequestError } from "./errors";
import type { NativeRemoteConnection } from "./remote";
import { connectLocalVault, connectRemoteVault } from "./vault-client";

it("selects client asset presentation independently of editor components", async () => {
  const local = await connectLocalVault();
  expect(
    local.attachmentPresentation({
      src: "asset:/local.png",
      path: "/local.png",
    }).src,
  ).toBe("asset:/local.png");
  const remote = connectRemoteVault(
    {
      info: { server: "https://example.com", capabilities: { features: [] } },
    } as unknown as NativeRemoteConnection,
    "member",
    { id: "vault", role: "editor" },
  );
  expect(
    remote.attachmentPresentation({
      src: "asset:/private.png",
      path: "/private.png",
    }),
  ).toEqual({ src: undefined, open: undefined, reveal: undefined });
});

const note = { markdown: "Note", title: "Title", revision: "a" };
const vault = {
  id: "first",
  name: "Vault",
  revision: "r",
  kind: "shared" as const,
  role: "editor" as const,
};

beforeEach(() => {
  vi.resetAllMocks();
  native.commands.applicationProcessingAvailable.mockResolvedValue(true);
  native.commands.applicationScope.mockResolvedValue({
    status: "ok",
    data: "local:canonical-vault",
  });
  native.commands.applicationReadNote.mockResolvedValue({
    status: "ok",
    data: note,
  });
  native.commands.applicationSaveNote.mockResolvedValue({
    status: "ok",
    data: { kind: "saved", note },
  });
  native.listen.mockResolvedValue(vi.fn());
  for (const command of [
    native.commands.applicationDocuments,
    native.commands.applicationTranscripts,
    native.commands.applicationTasks,
    native.commands.applicationPeople,
    native.commands.applicationTags,
  ])
    command.mockResolvedValue({ status: "ok", data: [] });
});

it("advertises local summary jobs only when the native worker is available", async () => {
  expect((await connectLocalVault()).features).toContain("summary_body_jobs");
  native.commands.applicationProcessingAvailable.mockResolvedValue(false);
  const local = await connectLocalVault();
  expect(local.features).not.toContain("summary_body_jobs");
  expect(local.canEdit).toBe(true);
  expect(local.networkMode).toBe("always");
});

it("uses matching bounded session-summary queries across both transports", async () => {
  const input = { kind: "ids" as const, ids: ["same", "missing"] };
  const rows = [{ id: "same", title: "Title", created_at: "2026-11-07" }];
  native.commands.applicationSessionSummaries.mockResolvedValue({
    status: "ok",
    data: rows,
  });
  const local = await connectLocalVault();
  expect(local.features).toContain("session_summaries");
  expect(await local.sessionSummaries(input)).toEqual(rows);
  expect(native.commands.applicationSessionSummaries).toHaveBeenCalledWith(
    input,
  );
  const request = vi.fn().mockResolvedValue(rows);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://example.com",
        capabilities: { features: ["session_summaries"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    vault,
  );
  const options = { signal: new AbortController().signal };
  expect(await remote.sessionSummaries(input, options)).toEqual(rows);
  expect(request).toHaveBeenCalledWith(
    { kind: "session_summaries", vault_id: "first", input },
    "session_summaries",
    options,
  );
});

it("uses one typed cleanup request through IPC and HTTPS without a local login", async () => {
  const input = { session_id: "same" };
  const outcome = { kind: "not_empty" as const };
  const options = { requestId: crypto.randomUUID() };
  native.commands.applicationDeleteEmptySession.mockResolvedValue({
    status: "ok",
    data: outcome,
  });
  const local = await connectLocalVault();
  expect(local.features).toContain("session_cleanup");
  expect(await local.deleteEmptySession(input, options)).toEqual(outcome);
  expect(native.commands.applicationDeleteEmptySession).toHaveBeenCalledWith(
    options.requestId,
    input,
  );
  const request = vi.fn().mockResolvedValue(outcome);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://example.com",
        capabilities: { features: ["session_cleanup"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    vault,
  );
  expect(await remote.deleteEmptySession(input, options)).toEqual(outcome);
  expect(request).toHaveBeenCalledWith(
    { kind: "delete_empty_session", vault_id: "first", input },
    "session_cleanup",
    options,
  );
});

it("keeps cleanup cancellation before dispatch separate from an uncertain sent mutation", async () => {
  const local = await connectLocalVault();
  const before = new AbortController();
  before.abort();
  await expect(
    local.deleteEmptySession({ session_id: "same" }, { signal: before.signal }),
  ).rejects.toThrow();
  expect(native.commands.applicationDeleteEmptySession).not.toHaveBeenCalled();
  const sent = new AbortController();
  native.commands.applicationDeleteEmptySession.mockImplementation(async () => {
    sent.abort();
    return { status: "ok", data: { kind: "deleted" } };
  });
  const requestId = crypto.randomUUID();
  await expect(
    local.deleteEmptySession(
      { session_id: "same" },
      { signal: sent.signal, requestId },
    ),
  ).rejects.toMatchObject({
    detail: {
      code: "cancelled",
      request_id: requestId,
      mutation_outcome: "unknown",
    },
  });
  expect(native.commands.applicationDeleteEmptySession).toHaveBeenCalledTimes(
    1,
  );
});

it("uses matching typed processing settings contracts across both transports", async () => {
  const policy = { providers: [], allow_custom_templates: false };
  const snapshot = {
    settings: {
      summary_model: null,
      ai_language: "en",
      summary_prompt: "",
      auto_apply_high_confidence_tags: true,
    },
    revision: "a".repeat(64),
  };
  const input = {
    settings: snapshot.settings,
    expected_revision: snapshot.revision,
  };
  const outcome = { kind: "saved", snapshot };
  native.commands.applicationProcessingPolicy.mockResolvedValue({
    status: "ok",
    data: policy,
  });
  native.commands.applicationProcessingSettings.mockResolvedValue({
    status: "ok",
    data: snapshot,
  });
  native.commands.applicationSaveProcessingSettings.mockResolvedValue({
    status: "ok",
    data: outcome,
  });
  const local = await connectLocalVault();
  expect(local.features).toContain("processing_settings");
  expect(await local.processingPolicy()).toEqual(policy);
  expect(await local.processingSettings()).toEqual(snapshot);
  expect(await local.saveProcessingSettings(input)).toEqual(outcome);
  expect(
    native.commands.applicationSaveProcessingSettings,
  ).toHaveBeenCalledWith(input);
  const request = vi
    .fn()
    .mockResolvedValueOnce(policy)
    .mockResolvedValueOnce(snapshot)
    .mockResolvedValueOnce(outcome);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://example.com",
        capabilities: { features: ["processing_settings"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    vault,
  );
  await remote.processingPolicy();
  await remote.processingSettings();
  await remote.saveProcessingSettings(input);
  expect(request.mock.calls).toEqual([
    [
      { kind: "processing_policy", vault_id: "first" },
      "processing_policy",
      undefined,
    ],
    [
      { kind: "processing_settings", vault_id: "first" },
      "processing_settings",
      undefined,
    ],
    [
      { kind: "save_processing_settings", vault_id: "first", input },
      "processing_settings_save",
      undefined,
    ],
  ]);
});

it("preserves summary receipt UUIDs and identical job operations across both transports", async () => {
  const input = {
    session_id: "note",
    target: { kind: "summary" as const },
    expected_revision: "baseline",
  };
  const list = {
    session_id: "note",
    active_only: false,
    limit: 50,
    after: null,
  };
  const job = {
    id: crypto.randomUUID(),
    session_id: "note",
    target: input.target,
    state: "succeeded",
    phase: null,
    failure: null,
    created_at: "time",
    updated_at: "time",
  };
  const page = { items: [job], next_cursor: null };
  const result = {
    vault_id: "first",
    job_id: job.id,
    session_id: "note",
    target: input.target,
    markdown: "Recovered result",
    suggested_tags: null,
  };
  for (const command of [
    native.commands.applicationSubmitSummaryJob,
    native.commands.applicationProcessingJob,
    native.commands.applicationCancelProcessingJob,
  ])
    command.mockResolvedValue({ status: "ok", data: job });
  native.commands.applicationProcessingJobs.mockResolvedValue({
    status: "ok",
    data: page,
  });
  native.commands.applicationProcessingJobResult.mockResolvedValue({
    status: "ok",
    data: result,
  });
  const options = { requestId: crypto.randomUUID() };
  const local = await connectLocalVault();
  expect(await local.submitSummaryJob(input, options)).toEqual(job);
  expect(await local.processingJobs(list)).toEqual(page);
  expect(await local.processingJob(job.id)).toEqual(job);
  expect(await local.processingJobResult(job.id)).toEqual(result);
  expect(await local.cancelProcessingJob(job.id, options)).toEqual(job);
  expect(native.commands.applicationSubmitSummaryJob).toHaveBeenCalledWith(
    options.requestId,
    input,
  );
  const request = vi.fn().mockResolvedValue(job);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://example.test/",
        capabilities: { features: ["summary_body_jobs"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    vault,
  );
  await remote.submitSummaryJob(input, options);
  await remote.processingJobs(list);
  await remote.processingJob(job.id);
  await remote.processingJobResult(job.id);
  await remote.cancelProcessingJob(job.id, options);
  expect(request.mock.calls).toEqual([
    [
      { kind: "submit_summary_job", vault_id: "first", input },
      "processing_job",
      options,
    ],
    [
      { kind: "processing_jobs", vault_id: "first", input: list },
      "processing_jobs",
      undefined,
    ],
    [
      { kind: "processing_job", vault_id: "first", job_id: job.id },
      "processing_job",
      undefined,
    ],
    [
      { kind: "processing_job_result", vault_id: "first", job_id: job.id },
      "processing_job_result",
      undefined,
    ],
    [
      { kind: "cancel_processing_job", vault_id: "first", job_id: job.id },
      "processing_job",
      options,
    ],
  ]);
});

it("treats an aborted local submission as uncertain and reuses its UUID for explicit replay", async () => {
  const controller = new AbortController();
  const options = { requestId: crypto.randomUUID(), signal: controller.signal };
  const input = {
    session_id: "note",
    target: { kind: "summary" as const },
    expected_revision: "baseline",
  };
  native.commands.applicationSubmitSummaryJob
    .mockImplementationOnce(async () => {
      controller.abort();
      return { status: "ok", data: { id: "persisted" } };
    })
    .mockResolvedValue({ status: "ok", data: { id: "persisted" } });
  const local = await connectLocalVault();
  await expect(local.submitSummaryJob(input, options)).rejects.toMatchObject({
    detail: {
      code: "cancelled",
      request_id: options.requestId,
      mutation_outcome: "unknown",
    },
  });
  expect(
    await local.submitSummaryJob(input, { requestId: options.requestId }),
  ).toEqual({ id: "persisted" });
  expect(native.commands.applicationSubmitSummaryJob.mock.calls).toEqual([
    [options.requestId, input],
    [options.requestId, input],
  ]);
});

it("uses device preparation and the same stable upload declaration through either vault adapter", async () => {
  const input = {
    filename: "chart ü.pdf",
    size_bytes: 7,
    sha256: "1".repeat(64),
  };
  const metadata = {
    filename: "chart ü 1.pdf",
    sizeBytes: 7,
    sha256: input.sha256,
    contentType: "application/pdf",
  };
  const id = crypto.randomUUID();
  const options = { requestId: id };
  native.commands.devicePrepareAttachmentUpload.mockResolvedValue({
    status: "ok",
    data: input,
  });
  native.commands.applicationUploadAttachmentSource.mockResolvedValue({
    status: "ok",
    data: metadata,
  });
  native.commands.applicationAttachmentUploadStatus.mockResolvedValue({
    status: "ok",
    data: { kind: "committed", value: metadata },
  });
  const local = await connectLocalVault();
  expect(await local.prepareAttachmentUpload("/device/chart ü.pdf")).toEqual(
    input,
  );
  expect(
    await local.uploadAttachmentPath(
      "note",
      "/device/chart ü.pdf",
      input,
      options,
    ),
  ).toEqual(metadata);
  expect(
    native.commands.applicationUploadAttachmentSource,
  ).toHaveBeenCalledWith(id, "note", input, {
    kind: "path",
    path: "/device/chart ü.pdf",
  });
  expect(await local.attachmentUploadStatus("note", id, input)).toEqual({
    kind: "committed",
    value: metadata,
  });
  expect(
    native.commands.applicationAttachmentUploadStatus,
  ).toHaveBeenCalledWith(id, "note", input);
  const uploadAttachmentPath = vi.fn().mockResolvedValue(metadata);
  const request = vi
    .fn()
    .mockResolvedValue({ kind: "committed", value: metadata });
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://server.example/",
        capabilities: { features: ["attachment_upload"] },
      },
      uploadAttachmentPath,
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    vault,
  );
  expect(await remote.prepareAttachmentUpload("/device/chart ü.pdf")).toEqual(
    input,
  );
  expect(
    await remote.uploadAttachmentPath(
      "note",
      "/device/chart ü.pdf",
      input,
      options,
    ),
  ).toEqual(metadata);
  expect(uploadAttachmentPath).toHaveBeenCalledWith(
    vault.id,
    "note",
    "/device/chart ü.pdf",
    input,
    options,
  );
  expect(await remote.attachmentUploadStatus("note", id, input)).toEqual({
    kind: "committed",
    value: metadata,
  });
  expect(request).toHaveBeenCalledWith(
    {
      kind: "attachment_upload_status",
      upload: { vault_id: vault.id, session_id: "note", request_id: id, input },
    },
    "attachment_upload_status",
    undefined,
  );
});

it("does not begin device review or local upload when already cancelled", async () => {
  const local = await connectLocalVault();
  const signal = AbortSignal.abort();
  await expect(
    local.prepareAttachmentUpload("/device/file.pdf", { signal }),
  ).rejects.toThrow();
  await expect(
    local.uploadAttachmentPath(
      "note",
      "/device/file.pdf",
      { filename: "file.pdf", size_bytes: 0, sha256: "1".repeat(64) },
      { requestId: crypto.randomUUID(), signal },
    ),
  ).rejects.toThrow();
  expect(native.commands.devicePrepareAttachmentUpload).not.toHaveBeenCalled();
  expect(
    native.commands.applicationUploadAttachmentSource,
  ).not.toHaveBeenCalled();
});

it("uses the same typed checked deletion through local IPC or remote requests", async () => {
  const input = { expected_sha256: "1".repeat(64) };
  native.commands.applicationDeleteRecording.mockResolvedValue({
    status: "ok",
    data: true,
  });
  const local = await connectLocalVault();
  expect(await local.deleteRecording("session", input)).toBe(true);
  expect(native.commands.applicationDeleteRecording).toHaveBeenCalledWith(
    "session",
    input,
  );
  const request = vi.fn().mockResolvedValue(false);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://server.example/",
        capabilities: { features: ["recording_delete"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    vault,
  );
  expect(await remote.deleteRecording("session", input)).toBe(false);
  expect(request).toHaveBeenCalledWith(
    {
      kind: "delete_recording",
      vault_id: vault.id,
      session_id: "session",
      input,
    },
    "recording_deleted",
    undefined,
  );
});

it("reads recording metadata through either transport without a filesystem path", async () => {
  const metadata = {
    filename: "audio.mp3",
    contentType: "audio/mpeg",
    sizeBytes: 42,
    sha256: "a".repeat(64),
  };
  native.commands.applicationAudioMetadata.mockResolvedValue({
    status: "ok",
    data: metadata,
  });
  const local = await connectLocalVault();
  expect(await local.audioMetadata("same")).toEqual(metadata);
  expect(native.commands.applicationAudioMetadata).toHaveBeenCalledWith("same");
  const request = vi.fn().mockResolvedValue(metadata);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://server.example/",
        capabilities: { features: ["audio_metadata_read"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "alice",
    vault,
  );
  expect(await remote.audioMetadata("same")).toEqual(metadata);
  expect(request).toHaveBeenCalledWith(
    { kind: "audio_metadata", vault_id: vault.id, session_id: "same" },
    "audio_metadata",
    undefined,
  );
  expect(metadata).not.toHaveProperty("path");
});

it("reads attachment information through either transport without a path or download lease", async () => {
  const info = {
    filename: "chart + # ü.png",
    contentType: "image/png",
    sizeBytes: 42,
  };
  native.commands.applicationAttachmentInfo.mockResolvedValue({
    status: "ok",
    data: info,
  });
  const local = await connectLocalVault();
  const controller = new AbortController();
  const options = { signal: controller.signal };
  expect(local.features).toContain("attachment_info");
  expect(await local.attachmentInfo("same", info.filename, options)).toEqual(
    info,
  );
  expect(native.commands.applicationAttachmentInfo).toHaveBeenCalledWith(
    "same",
    info.filename,
  );
  const request = vi.fn().mockResolvedValue(info);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://server.example/",
        capabilities: { features: ["attachment_info"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "alice",
    vault,
  );
  expect(await remote.attachmentInfo("same", info.filename, options)).toEqual(
    info,
  );
  expect(request).toHaveBeenCalledWith(
    {
      kind: "attachment_info",
      vault_id: vault.id,
      session_id: "same",
      attachment_id: info.filename,
    },
    "attachment_info",
    options,
  );
  native.commands.applicationAttachmentInfo.mockResolvedValue({
    status: "ok",
    data: null,
  });
  expect(await local.attachmentInfo("same", "missing.png")).toBeNull();
  request.mockResolvedValue(null);
  expect(await remote.attachmentInfo("same", "missing.png")).toBeNull();
  controller.abort();
  native.commands.applicationAttachmentInfo.mockClear();
  await expect(
    local.attachmentInfo("same", info.filename, options),
  ).rejects.toMatchObject({
    name: "AbortError",
  });
  expect(native.commands.applicationAttachmentInfo).not.toHaveBeenCalled();
  expect(info).not.toHaveProperty("path");
  expect(info).not.toHaveProperty("sha256");
});

it("reads one coherent note/task snapshot through both transports", async () => {
  const snapshot = { note, tasks: { tasks: [], revision: "tasks" } };
  native.commands.applicationReadNoteEditor.mockResolvedValue({
    status: "ok",
    data: snapshot,
  });
  const local = await connectLocalVault();
  expect(await local.readNoteEditor("same")).toEqual(snapshot);
  expect(native.commands.applicationReadNoteEditor).toHaveBeenCalledWith(
    "same",
  );
  const request = vi.fn().mockResolvedValue(snapshot);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://server.example/",
        capabilities: { features: ["note_editor_read"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "alice",
    vault,
  );
  expect(await remote.readNoteEditor("same")).toEqual(snapshot);
  expect(request).toHaveBeenCalledWith(
    { kind: "note_editor", vault_id: vault.id, session_id: "same" },
    "note_editor",
    undefined,
  );
});

it("sends editor task identity and both revisions through both transports", async () => {
  const input = {
    markdown: "Updated",
    title: "Title",
    expected_revision: "a",
    expected_tasks_revision: "tasks-a",
    tasks: [],
  };
  const outcome = {
    kind: "conflict",
    snapshot: { note, tasks: { tasks: [], revision: "tasks-b" } },
  };
  native.commands.applicationSaveNoteEditor.mockResolvedValue({
    status: "ok",
    data: outcome,
  });
  const local = await connectLocalVault();
  expect(await local.saveNoteEditor("same", input)).toEqual(outcome);
  expect(native.commands.applicationSaveNoteEditor).toHaveBeenCalledWith(
    "same",
    input,
  );
  const request = vi.fn().mockResolvedValue(outcome);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://server.example/",
        capabilities: { features: ["note_editor_write"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "alice",
    vault,
  );
  expect(await remote.saveNoteEditor("same", input)).toEqual(outcome);
  expect(request).toHaveBeenCalledWith(
    { kind: "save_note_editor", vault_id: vault.id, session_id: "same", input },
    "note_editor_save",
    undefined,
  );
});

describe("shared vault request and subscription contract", () => {
  it("shares checked source-task replacements across IPC and a pinned remote vault", async () => {
    const snapshot = { tasks: [], revision: "tasks-r" };
    const input = {
      source: { kind: "raw_note" as const },
      tasks: [],
      expected_revision: "tasks-r",
    };
    const outcome = { kind: "conflict", snapshot };
    native.commands.applicationTasksSnapshot.mockResolvedValue({
      status: "ok",
      data: snapshot,
    });
    native.commands.applicationReplaceTasks.mockResolvedValue({
      status: "ok",
      data: outcome,
    });
    const local = await connectLocalVault();
    expect(await local.tasksSnapshot("same")).toEqual(snapshot);
    expect(await local.replaceTasks("same", input)).toEqual(outcome);
    expect(native.commands.applicationReplaceTasks).toHaveBeenCalledWith(
      "same",
      input,
    );
    const request = vi
      .fn()
      .mockResolvedValueOnce(snapshot)
      .mockResolvedValueOnce(outcome);
    const remote = connectRemoteVault(
      {
        info: {
          server: "https://company.example/",
          capabilities: { features: ["tasks_edit"] },
        },
        request,
      } as unknown as NativeRemoteConnection,
      "alice",
      vault,
    );
    expect(await remote.tasksSnapshot("same")).toEqual(snapshot);
    expect(await remote.replaceTasks("same", input)).toEqual(outcome);
    expect(request).toHaveBeenNthCalledWith(
      1,
      { kind: "tasks_snapshot", vault_id: "first", session_id: "same" },
      "tasks_snapshot",
      undefined,
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      { kind: "replace_tasks", vault_id: "first", session_id: "same", input },
      "tasks_save",
      undefined,
    );
  });

  it("uses the same revision-checked metadata patch in local IPC and pinned HTTPS", async () => {
    const snapshot = {
      meta: { id: "same", title: "Title" },
      revision: "baseline",
    };
    const outcome = { kind: "conflict", snapshot };
    const input = {
      patch: { title: "New title" },
      expected_revision: "baseline",
    };
    native.commands.applicationReadMetadata.mockResolvedValue({
      status: "ok",
      data: snapshot,
    });
    native.commands.applicationSaveMetadata.mockResolvedValue({
      status: "ok",
      data: outcome,
    });
    const local = await connectLocalVault();
    expect(await local.readMetadata("same")).toEqual(snapshot);
    expect(await local.saveMetadata("same", input)).toEqual(outcome);
    expect(native.commands.applicationSaveMetadata).toHaveBeenCalledWith(
      "same",
      input,
    );
    const request = vi
      .fn()
      .mockResolvedValueOnce(snapshot)
      .mockResolvedValueOnce(outcome);
    const remote = connectRemoteVault(
      {
        info: {
          server: "https://company.example/",
          capabilities: { features: ["metadata_edit"] },
        },
        request,
      } as unknown as NativeRemoteConnection,
      "alice",
      vault,
    );
    expect(await remote.readMetadata("same")).toEqual(snapshot);
    expect(await remote.saveMetadata("same", input)).toEqual(outcome);
    expect(request).toHaveBeenNthCalledWith(
      1,
      { kind: "metadata", vault_id: "first", session_id: "same" },
      "metadata",
      undefined,
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      { kind: "save_metadata", vault_id: "first", session_id: "same", input },
      "metadata_save",
      undefined,
    );
  });

  it("pins agent reads to the same vault and preserves canonical CLI projections across IPC and HTTPS", async () => {
    const list = {
      query: null,
      tags: ["project"],
      untagged: false,
      limit: 20,
      offset: 0,
    };
    const search = { query: "term", limit: 20, offset: 0 };
    const options = { signal: new AbortController().signal };
    const request = vi.fn();
    const remote = connectRemoteVault(
      {
        info: {
          server: "https://company.example/",
          capabilities: { features: ["agent_read"] },
        },
        request,
      } as unknown as NativeRemoteConnection,
      "alice",
      vault,
    );
    const local = await connectLocalVault();
    const operations = [
      [
        () => local.agentList(list, options),
        () => remote.agentList(list, options),
        native.commands.applicationAgentList,
        [list],
        { kind: "agent_list", vault_id: "first", input: list },
      ],
      [
        () => local.agentSearch(search, options),
        () => remote.agentSearch(search, options),
        native.commands.applicationAgentSearch,
        [search],
        { kind: "agent_search", vault_id: "first", input: search },
      ],
      [
        () => local.agentMeeting("same", options),
        () => remote.agentMeeting("same", options),
        native.commands.applicationAgentMeeting,
        ["same"],
        { kind: "agent_meeting", vault_id: "first", session_id: "same" },
      ],
      [
        () => local.agentTranscript("same", options),
        () => remote.agentTranscript("same", options),
        native.commands.applicationAgentTranscript,
        ["same"],
        { kind: "agent_transcript", vault_id: "first", session_id: "same" },
      ],
      [
        () => local.agentExport("same", options),
        () => remote.agentExport("same", options),
        native.commands.applicationAgentExport,
        ["same"],
        { kind: "agent_export", vault_id: "first", session_id: "same" },
      ],
      [
        () => local.agentSegments("same", options),
        () => remote.agentSegments("same", options),
        native.commands.applicationAgentSegments,
        ["same"],
        { kind: "agent_segments", vault_id: "first", session_id: "same" },
      ],
    ] as const;
    for (const [
      readLocal,
      readRemote,
      command,
      args,
      operation,
    ] of operations) {
      const projection = { marker: operation.kind };
      command.mockResolvedValueOnce({ status: "ok", data: projection });
      request.mockResolvedValueOnce(projection);
      expect(await readLocal()).toEqual(projection);
      expect(command).toHaveBeenCalledWith(...args);
      expect(await readRemote()).toEqual(projection);
      expect(request).toHaveBeenLastCalledWith(
        operation,
        operation.kind,
        options,
      );
    }
  });

  it("uses the same document target and checked replacement through IPC and HTTPS", async () => {
    const target = { kind: "template_output" as const, document_id: "output" };
    const input = { markdown: "Edited output", expected_revision: "baseline" };
    const snapshot = { document: null, revision: "baseline" };
    const conflict = { kind: "conflict", snapshot };
    native.commands.applicationReadDocument.mockResolvedValue({
      status: "ok",
      data: snapshot,
    });
    native.commands.applicationSaveDocument.mockResolvedValue({
      status: "ok",
      data: conflict,
    });
    const local = await connectLocalVault();
    expect(await local.readDocument("session", target)).toEqual(snapshot);
    expect(await local.saveDocument("session", target, input)).toEqual(
      conflict,
    );
    expect(native.commands.applicationSaveDocument).toHaveBeenCalledWith(
      "session",
      target,
      input,
    );
    const request = vi
      .fn()
      .mockResolvedValueOnce(snapshot)
      .mockResolvedValueOnce(conflict);
    const remote = connectRemoteVault(
      {
        info: {
          server: "https://company.example/",
          capabilities: { features: ["document_edit"] },
        },
        request,
      } as unknown as NativeRemoteConnection,
      "alice",
      vault,
    );
    expect(await remote.readDocument("session", target)).toEqual(snapshot);
    expect(await remote.saveDocument("session", target, input)).toEqual(
      conflict,
    );
    expect(request).toHaveBeenNthCalledWith(
      1,
      { kind: "document", vault_id: "first", session_id: "session", target },
      "document",
      undefined,
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      {
        kind: "save_document",
        vault_id: "first",
        session_id: "session",
        target,
        input,
      },
      "document_save",
      undefined,
    );
  });
  it("reads content through IPC locally and pins every remote read to its selected vault", async () => {
    const local = await connectLocalVault();
    await Promise.all([
      local.documents("same"),
      local.transcripts("same"),
      local.tasks("same"),
      local.people(),
      local.tags(),
    ]);
    expect(native.commands.applicationDocuments).toHaveBeenCalledWith("same");
    expect(native.commands.applicationTranscripts).toHaveBeenCalledWith("same");
    expect(native.commands.applicationTasks).toHaveBeenCalledWith("same");
    expect(native.commands.applicationPeople).toHaveBeenCalledOnce();
    expect(native.commands.applicationTags).toHaveBeenCalledOnce();
    const request = vi.fn().mockResolvedValue([]);
    const connection = {
      info: {
        server: "https://company.example/",
        capabilities: { features: ["content_read"] },
      },
      request,
    } as unknown as NativeRemoteConnection;
    const remote = connectRemoteVault(connection, "alice", vault);
    await Promise.all([
      remote.documents("same"),
      remote.transcripts("same"),
      remote.tasks("same"),
      remote.people(),
      remote.tags(),
    ]);
    expect(request.mock.calls.map(([operation]) => operation)).toEqual([
      { kind: "documents", vault_id: "first", session_id: "same" },
      { kind: "transcripts", vault_id: "first", session_id: "same" },
      { kind: "tasks", vault_id: "first", session_id: "same" },
      { kind: "people", vault_id: "first" },
      { kind: "tags", vault_id: "first" },
    ]);
  });
  it("keeps local reads on typed IPC with an always-available network mode and no HTTP requests", async () => {
    const http = vi.spyOn(globalThis, "fetch");
    const client = await connectLocalVault();
    expect(client.scope).toBe("local:canonical-vault");
    expect(client.networkMode).toBe("always");
    expect(await client.readNote("session")).toEqual(note);
    expect(native.commands.applicationReadNote).toHaveBeenCalledWith("session");
    expect(http).not.toHaveBeenCalled();
    http.mockRestore();
  });

  it("scopes local notifications and revalidates after the listener is attached", async () => {
    const client = await connectLocalVault();
    const listener = vi.fn();
    const unsubscribe = await client.subscribe(listener);
    expect(listener.mock.calls.map(([event]) => event.kind)).toEqual([
      "connection",
      "change",
    ]);
    const onChange = native.listen.mock.calls[0][0];
    onChange({
      payload: {
        event: { vault_id: "other-vault", change: { kind: "search_ready" } },
      },
    });
    expect(listener).toHaveBeenCalledTimes(2);
    onChange({
      payload: {
        event: { vault_id: client.scope, change: { kind: "search_ready" } },
      },
    });
    expect(listener).toHaveBeenLastCalledWith({
      kind: "change",
      change: { kind: "search_ready" },
    });
    unsubscribe();
    expect(await native.listen.mock.results[0].value).toHaveBeenCalledOnce();
  });

  it("passes the same expected revision through IPC without replacing it from a cache", async () => {
    const client = await connectLocalVault();
    const input = {
      markdown: "Edited",
      title: "Title",
      expected_revision: "draft-baseline",
    };
    await client.saveNote("session", input);
    expect(native.commands.applicationSaveNote).toHaveBeenCalledWith(
      "session",
      input,
    );
    const error = {
      code: "forbidden",
      message: "No access",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    };
    native.commands.applicationReadNote.mockResolvedValueOnce({
      status: "error",
      error,
    });
    await expect(client.readNote("session")).rejects.toBeInstanceOf(
      ApplicationRequestError,
    );
  });

  it("pins remote operations and scopes to the original server, account and vault", async () => {
    const connection = {
      info: {
        server: "https://company.example/",
        capabilities: { features: ["notes"] },
      },
      request: vi.fn().mockResolvedValue(note),
      subscribe: vi.fn(),
    } as unknown as NativeRemoteConnection;
    const first = connectRemoteVault(connection, "alice", vault);
    const second = connectRemoteVault(connection, "bob", {
      ...vault,
      id: "second",
    });
    expect(first.scope).not.toBe(second.scope);
    expect(first.networkMode).toBe("online");
    await first.readNote("same-session");
    await second.readNote("same-session");
    expect(connection.request).toHaveBeenNthCalledWith(
      1,
      { kind: "note", vault_id: "first", session_id: "same-session" },
      "note",
      undefined,
    );
    expect(connection.request).toHaveBeenNthCalledWith(
      2,
      { kind: "note", vault_id: "second", session_id: "same-session" },
      "note",
      undefined,
    );
    expect(
      connectRemoteVault(connection, "alice", { ...vault, role: "reader" })
        .canEdit,
    ).toBe(false);
  });
});

it("uses the same attachment page contract over IPC and HTTPS with opaque cursors", async () => {
  const page = {
    files: [
      {
        filename: "chart + ü.pdf",
        contentType: "application/pdf",
        sizeBytes: 42,
      },
    ],
    next_cursor: null,
  };
  native.commands.applicationAttachments.mockResolvedValue({
    status: "ok",
    data: page,
  });
  const local = await connectLocalVault();
  expect(await local.attachments("note", 10, "a.pdf")).toEqual(page);
  expect(native.commands.applicationAttachments).toHaveBeenCalledWith(
    "note",
    10,
    "a.pdf",
  );
  const request = vi.fn().mockResolvedValue(page);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://example.com",
        capabilities: { features: ["attachment_list"] },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    { id: "vault", role: "reader" },
  );
  expect(await remote.attachments("note", 10, "a.pdf")).toEqual(page);
  expect(request).toHaveBeenCalledWith(
    {
      kind: "attachments",
      vault_id: "vault",
      session_id: "note",
      limit: 10,
      after: "a.pdf",
    },
    "attachments",
    undefined,
  );
});

it("shares checked attachment review and removal contracts with a stable request ID", async () => {
  const metadata = {
    filename: "file.pdf",
    contentType: "application/pdf",
    sizeBytes: 42,
    sha256: "0".repeat(64),
  };
  const input = { expected_sha256: metadata.sha256 };
  const options = { requestId: crypto.randomUUID() };
  native.commands.applicationAttachmentMetadata.mockResolvedValue({
    status: "ok",
    data: metadata,
  });
  native.commands.applicationRemoveAttachment.mockResolvedValue({
    status: "ok",
    data: true,
  });
  const local = await connectLocalVault();
  expect(await local.attachmentMetadata("note", metadata.filename)).toEqual(
    metadata,
  );
  expect(
    await local.removeAttachment("note", metadata.filename, input, options),
  ).toBe(true);
  expect(native.commands.applicationRemoveAttachment).toHaveBeenCalledWith(
    options.requestId,
    "note",
    metadata.filename,
    input,
  );
  const request = vi
    .fn()
    .mockResolvedValueOnce(metadata)
    .mockResolvedValueOnce(true);
  const remote = connectRemoteVault(
    {
      info: {
        server: "https://example.com",
        capabilities: {
          features: ["attachment_metadata", "attachment_remove"],
        },
      },
      request,
    } as unknown as NativeRemoteConnection,
    "member",
    { id: "vault", role: "editor" },
  );
  expect(await remote.attachmentMetadata("note", metadata.filename)).toEqual(
    metadata,
  );
  expect(
    await remote.removeAttachment("note", metadata.filename, input, options),
  ).toBe(true);
  expect(request).toHaveBeenLastCalledWith(
    {
      kind: "remove_attachment",
      vault_id: "vault",
      session_id: "note",
      attachment_id: metadata.filename,
      input,
    },
    "attachment_removed",
    options,
  );
});
