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
