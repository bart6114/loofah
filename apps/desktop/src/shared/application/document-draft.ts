import { ApplicationRequestError } from "./errors";
import type { NoteDraftClient } from "./note-draft";
import type {
  DocumentSnapshot,
  DocumentTarget,
  NoteSnapshot,
} from "./protocol.gen";
import type { VaultClient } from "./vault-client";

export function documentDraftId(
  sessionId: string,
  target: DocumentTarget,
): string {
  return JSON.stringify(
    target.kind === "summary"
      ? ["summary", sessionId]
      : ["template_output", sessionId, target.document_id],
  );
}

export function documentDraftClient(
  client: VaultClient,
  sessionId: string,
  input: DocumentTarget,
): NoteDraftClient {
  const target: DocumentTarget =
    input.kind === "summary"
      ? { kind: "summary" }
      : { kind: "template_output", document_id: input.document_id };
  const key = documentDraftId(sessionId, target);
  const validate = (id: string) => {
    if (id !== key)
      throw new Error("This draft belongs to a different document");
  };
  const project = (value: DocumentSnapshot): NoteSnapshot => {
    const doc = value.document;
    if (!doc && target.kind === "template_output") {
      throw new ApplicationRequestError({
        code: "not_found",
        message: "This output no longer exists",
        request_id: null,
        retryable: false,
        current_revision: value.revision,
        mutation_outcome: "not_committed",
      });
    }
    if (
      doc &&
      (doc.session_id !== sessionId ||
        doc.kind !==
          (target.kind === "summary" ? "summary" : "template_output") ||
        doc.id !== (target.kind === "summary" ? sessionId : target.document_id))
    ) {
      throw new ApplicationRequestError({
        code: "incompatible_version",
        message: "The server returned a different document",
        request_id: null,
        retryable: false,
        current_revision: null,
        mutation_outcome: "unknown",
      });
    }
    // Output metadata stays outside the body draft so a rename cannot leave a save dirty.
    return {
      markdown: doc?.markdown ?? null,
      title: "",
      revision: value.revision,
    };
  };
  return {
    scope: client.scope,
    canEdit: client.canEdit,
    readNote: async (id, options) => {
      validate(id);
      return project(await client.readDocument(sessionId, target, options));
    },
    saveNote: async (id, value, options) => {
      validate(id);
      const result = await client.saveDocument(
        sessionId,
        target,
        {
          markdown: value.markdown,
          expected_revision: value.expected_revision,
        },
        options,
      );
      if (result.kind === "saved" && !result.snapshot.document) {
        throw new ApplicationRequestError({
          code: "incompatible_version",
          message: "The server omitted the saved document",
          request_id: null,
          retryable: false,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      }
      return { kind: result.kind, note: project(result.snapshot) };
    },
  };
}
