import { useMutation, useMutationState } from "@tanstack/react-query";
import { useCallback } from "react";

import type { FileUploadCandidate } from "@hypr/editor/note";

import type { PreparedAttachmentUpload } from "./device-attachment-source";
import { ApplicationRequestError } from "./errors";
import type { FileMetadata } from "./protocol.gen";
import type { VaultScope } from "./vault-scope";

type Upload = {
  clientId: string;
  candidate: FileUploadCandidate;
  prepared?: PreparedAttachmentUpload;
  removed?: boolean;
};

function mutationKey(scope: VaultScope, sessionId: string) {
  return ["vault", scope.client.scope, "attachment_upload", sessionId] as const;
}

export function useVaultFileUpload(scope: VaultScope, sessionId: string) {
  const mutation = useMutation(
    {
      mutationKey: mutationKey(scope, sessionId),
      scope: {
        id: JSON.stringify([
          "attachment_upload",
          scope.client.scope,
          sessionId,
        ]),
      },
      networkMode: "always",
      retry: false,
      gcTime: Infinity,
      mutationFn: async (attempt: Upload): Promise<FileMetadata> => {
        if (attempt.removed) throw stopped();
        if (!attempt.prepared)
          attempt.prepared = await scope.prepareAttachmentSource(
            attempt.candidate,
          );
        if (attempt.removed) {
          void attempt.prepared.close().catch(() => {});
          throw stopped();
        }
        const metadata = await scope.uploadAttachmentSource(
          sessionId,
          attempt.prepared,
          attempt.clientId,
        );
        void attempt.prepared.close().catch(() => {});
        return metadata;
      },
      onSuccess: () => {
        void scope.queryClient.invalidateQueries({
          queryKey: ["vault", scope.client.scope, "attachment_list", sessionId],
        });
      },
    },
    scope.queryClient,
  );
  return useCallback(
    (
      candidate: FileUploadCandidate,
      clientId: string = crypto.randomUUID(),
    ) => {
      if (
        scope.getSnapshot().state !== "connected" ||
        !scope.client.canEdit ||
        !scope.client.features.includes("attachment_upload")
      )
        return Promise.reject(stopped());
      const cache = scope.queryClient.getMutationCache();
      let attempt: Upload | undefined;
      for (const previous of cache.findAll({
        mutationKey: mutationKey(scope, sessionId),
        exact: true,
      })) {
        const variables = previous.state.variables as Upload | undefined;
        if (variables?.clientId === clientId) {
          attempt = variables;
          if (previous.state.status !== "pending") cache.remove(previous);
        }
      }
      return mutation
        .mutateAsync(attempt ?? { clientId, candidate })
        .then((metadata) => {
          for (const completed of cache.findAll({
            mutationKey: mutationKey(scope, sessionId),
            exact: true,
          })) {
            if (
              (completed.state.variables as Upload | undefined)?.clientId ===
                clientId &&
              completed.state.status === "success"
            )
              cache.remove(completed);
          }
          mutation.reset();
          return metadata;
        });
    },
    [scope, sessionId, mutation.mutateAsync, mutation.reset],
  );
}

export function useVaultFileUploadStates(scope: VaultScope, sessionId: string) {
  const results = useMutationState(
    {
      filters: {
        mutationKey: mutationKey(scope, sessionId),
        exact: true,
        predicate: (mutation) =>
          mutation.state.status === "pending" ||
          mutation.state.status === "error",
      },
      select: (mutation) => ({
        ...(mutation.state.variables as Upload),
        status: mutation.state.status as "pending" | "error",
        error: mutation.state.error,
        submittedAt: mutation.state.submittedAt,
      }),
    },
    scope.queryClient,
  );
  const latest = new Map<string, (typeof results)[number]>();
  for (const result of results) {
    const previous = latest.get(result.clientId);
    if (!previous || result.submittedAt >= previous.submittedAt)
      latest.set(result.clientId, result);
  }
  const remove = useCallback(
    (clientId: string) => {
      const cache = scope.queryClient.getMutationCache();
      for (const mutation of cache.findAll({
        mutationKey: mutationKey(scope, sessionId),
        exact: true,
      })) {
        const attempt = mutation.state.variables as Upload | undefined;
        if (attempt?.clientId === clientId) {
          attempt.removed = true;
          void attempt.prepared?.close().catch(() => {});
          cache.remove(mutation);
        }
      }
    },
    [scope, sessionId],
  );
  return { states: [...latest.values()], remove };
}

function stopped() {
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
