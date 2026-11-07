import type { FileUploadCandidate } from "@hypr/editor/note";

import type { PreparedAttachmentUpload } from "./device-attachment-source";
import { ApplicationRequestError } from "./errors";
import type { VaultScope } from "./vault-scope";

export type AttachmentUploadAttempt = {
  clientId: string;
  candidate: FileUploadCandidate;
  prepared?: PreparedAttachmentUpload;
  removed?: boolean;
};

export async function uploadAttachmentAttempt(
  scope: VaultScope,
  sessionId: string,
  attempt: AttachmentUploadAttempt,
) {
  if (attempt.removed) throw attachmentUploadStopped();
  if (!attempt.prepared)
    attempt.prepared = await scope.prepareAttachmentSource(attempt.candidate);
  if (attempt.removed) {
    void attempt.prepared.close().catch(() => {});
    throw attachmentUploadStopped();
  }
  const metadata = await scope.uploadAttachmentSource(
    sessionId,
    attempt.prepared,
    attempt.clientId,
  );
  void attempt.prepared.close().catch(() => {});
  return metadata;
}

export function attachmentUploadStopped() {
  return new ApplicationRequestError({
    code: "cancelled",
    message:
      "Attachment upload is unavailable or was removed; reconnect before trying again",
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: "not_committed",
  });
}
