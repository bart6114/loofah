import { beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  commands: {
    applicationScope: vi.fn(),
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
    applicationSessions: vi.fn(),
    applicationSession: vi.fn(),
    applicationSearch: vi.fn(),
    applicationDocuments: vi.fn(),
    applicationReadDocument: vi.fn(),
    applicationSaveDocument: vi.fn(),
    applicationTranscripts: vi.fn(),
    applicationAudioMetadata: vi.fn(),
    applicationAttachmentInfo: vi.fn(),
    applicationAttachmentMetadata: vi.fn(),
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
