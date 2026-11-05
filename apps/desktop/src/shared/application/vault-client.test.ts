import { beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  commands: {
    applicationScope: vi.fn(),
    applicationReadNote: vi.fn(),
    applicationSaveNote: vi.fn(),
    applicationCreateSession: vi.fn(),
    applicationSessions: vi.fn(),
    applicationSession: vi.fn(),
    applicationSearch: vi.fn(),
    applicationDocuments: vi.fn(),
    applicationTranscripts: vi.fn(),
    applicationTasks: vi.fn(),
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

describe("shared vault request and subscription contract", () => {
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
