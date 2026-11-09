import { useCallback } from "react";

import type { FileUploadCandidate } from "@hypr/editor/note";

import { attachmentUploadStopped } from "~/shared/application/attachment-upload-attempt";
import { ApplicationRequestError } from "~/shared/application/errors";
import type { FileMetadata } from "~/shared/application/protocol.gen";
import {
  useVaultFileUpload,
  useVaultFileUploadStates,
} from "~/shared/application/use-vault-file-upload";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";

export type { FileUploadCandidate } from "@hypr/editor/note";

export type FileUploadResult = {
  url: string;
  path: string;
  attachmentId: string;
  attachment: FileMetadata;
};

export type FileUploadState = {
  clientId: string;
  candidate: FileUploadCandidate;
  status: "pending" | "error";
  error: unknown;
  submittedAt: number;
};

export function useFileUpload(sessionId: string) {
  const { scope, current } = useVaultReadScope();
  const upload = useVaultFileUpload(scope, sessionId);
  return useCallback(
    async (
      candidate: FileUploadCandidate,
      clientId?: string,
    ): Promise<FileUploadResult> => {
      if (!current()) throw attachmentUploadStopped();
      const metadata = await upload(candidate, clientId);
      if (!current())
        throw new ApplicationRequestError({
          code: "cancelled",
          message:
            "The attachment was uploaded but this editor is unavailable. Check the attachment list before trying again.",
          request_id: clientId ?? null,
          retryable: false,
          current_revision: null,
          mutation_outcome: "unknown",
        });
      return {
        attachmentId: metadata.filename,
        attachment: metadata,
        url: "",
        path: "",
      };
    },
    [current, upload],
  );
}

export function useFileUploadStates(sessionId: string) {
  const owner = useVaultReadScope();
  const result = useVaultFileUploadStates(owner.scope, sessionId);
  return {
    states: owner.current() ? result.states : [],
    remove: result.remove,
  };
}
