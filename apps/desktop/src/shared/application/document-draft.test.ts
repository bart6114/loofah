import { describe, expect, it, vi } from "vitest";

import { documentDraftClient, documentDraftId } from "./document-draft";
import { ApplicationRequestError } from "./errors";
import { NoteDraft, NoteConflictError } from "./note-draft";
import type { DocumentSnapshot, DocumentTarget } from "./protocol.gen";
import type { VaultClient } from "./vault-client";

const target: DocumentTarget = {
  kind: "template_output",
  document_id: "output",
};
function snapshot(
  markdown: string,
  revision: string,
  title = "Output",
): DocumentSnapshot {
  return {
    document: {
      id: "output",
      session_id: "session",
      kind: "template_output",
      title,
      template_id: "template",
      sort_order: 2,
      markdown,
    },
    revision,
  };
}
function fixture() {
  const client = {
    scope: "server:alice:vault",
    canEdit: true,
    readDocument: vi.fn().mockResolvedValue(snapshot("Original", "a")),
    saveDocument: vi.fn(),
  } as unknown as VaultClient;
  return {
    client,
    draftClient: documentDraftClient(client, "session", target),
    id: documentDraftId("session", target),
  };
}

describe("document text draft contract", () => {
  it("shares the vault scope and isolates resources while serializing only body and revision", async () => {
    const { client, draftClient, id } = fixture();
    expect(draftClient.scope).toBe(client.scope);
    expect(id).not.toBe("session");
    expect(id).not.toBe(documentDraftId("session", { kind: "summary" }));
    const value = await draftClient.readNote(id);
    expect(value).toEqual({ markdown: "Original", title: "", revision: "a" });
    vi.mocked(client.saveDocument).mockResolvedValue({
      kind: "saved",
      snapshot: snapshot("Changed", "b", "Renamed"),
    });
    const options = { requestId: crypto.randomUUID() };
    expect(
      await draftClient.saveNote(
        id,
        { markdown: "Changed", title: "", expected_revision: "a" },
        options,
      ),
    ).toEqual({
      kind: "saved",
      note: { markdown: "Changed", title: "", revision: "b" },
    });
    expect(client.saveDocument).toHaveBeenCalledWith(
      "session",
      target,
      { markdown: "Changed", expected_revision: "a" },
      options,
    );
    await expect(
      draftClient.saveNote("another", {
        markdown: "Wrong",
        title: "",
        expected_revision: "a",
      }),
    ).rejects.toThrow("different document");
    expect(client.saveDocument).toHaveBeenCalledOnce();
  });

  it("reviews a stale output and preserves current metadata without leaving a title dirty", async () => {
    const { client, draftClient, id } = fixture();
    const draft = new NoteDraft(
      draftClient,
      id,
      await draftClient.readNote(id),
    );
    draft.edit("My output", "");
    vi.mocked(client.saveDocument).mockResolvedValueOnce({
      kind: "conflict",
      snapshot: snapshot("Their output", "b", "Renamed elsewhere"),
    });
    await expect(draft.save()).rejects.toBeInstanceOf(NoteConflictError);
    expect(draft.getSnapshot().draft.markdown).toBe("My output");
    draft.keepDraft("b");
    vi.mocked(client.saveDocument).mockResolvedValueOnce({
      kind: "saved",
      snapshot: snapshot("My output", "c", "Renamed elsewhere"),
    });
    await draft.save();
    expect(draft.getSnapshot().dirty).toBe(false);
    expect(vi.mocked(client.saveDocument).mock.calls[1][2]).toEqual({
      markdown: "My output",
      expected_revision: "b",
    });
  });

  it("rejects missing outputs and mismatched identities instead of accepting another resource", async () => {
    const { client, draftClient, id } = fixture();
    vi.mocked(client.readDocument).mockResolvedValueOnce({
      document: null,
      revision: "missing",
    });
    await expect(draftClient.readNote(id)).rejects.toMatchObject({
      detail: { code: "not_found" },
    });
    const wrong = snapshot("Private text", "b");
    wrong.document!.session_id = "another-session";
    vi.mocked(client.readDocument).mockResolvedValueOnce(wrong);
    await expect(draftClient.readNote(id)).rejects.toBeInstanceOf(
      ApplicationRequestError,
    );
    const summary = documentDraftClient(client, "session", { kind: "summary" });
    vi.mocked(client.readDocument).mockResolvedValueOnce({
      document: null,
      revision: "absent",
    });
    expect(
      await summary.readNote(documentDraftId("session", { kind: "summary" })),
    ).toEqual({ markdown: null, title: "", revision: "absent" });
  });

  it("keeps an uncertain output response recoverable and does not accept an omitted saved document", async () => {
    const { client, draftClient, id } = fixture();
    const draft = new NoteDraft(
      draftClient,
      id,
      await draftClient.readNote(id),
    );
    draft.edit("Recover me", "");
    vi.mocked(client.saveDocument).mockResolvedValue({
      kind: "saved",
      snapshot: { document: null, revision: "b" },
    });
    await expect(draft.save()).rejects.toMatchObject({
      detail: { code: "incompatible_version", mutation_outcome: "unknown" },
    });
    expect(draft.recovery()).toMatchObject({
      scope: client.scope,
      sessionId: id,
      uncertain: true,
      draft: { markdown: "Recover me" },
    });
  });
});
