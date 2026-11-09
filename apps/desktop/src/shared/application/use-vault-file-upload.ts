import {
  onlineManager,
  useMutation,
  useMutationState,
} from "@tanstack/react-query";
import { useCallback } from "react";

import type { FileUploadCandidate } from "@hypr/editor/note";

import {
  type AttachmentUploadAttempt as Upload,
  attachmentUploadStopped as stopped,
  uploadAttachmentAttempt,
} from "./attachment-upload-attempt";
import type { VaultScope } from "./vault-scope";

function mutationKey(scope: VaultScope, sessionId: string) {
  return ["vault", scope.client.scope, "attachment_upload", sessionId] as const;
}

function assertUploadAvailable(scope: VaultScope) {
  if (
    scope.getSnapshot().state !== "connected" ||
    !scope.client.canEdit ||
    !scope.client.features.includes("attachment_upload") ||
    (scope.client.networkMode === "online" && !onlineManager.isOnline())
  )
    throw stopped();
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
      mutationFn: (attempt: Upload) =>
        uploadAttachmentAttempt(scope, sessionId, attempt, () =>
          assertUploadAvailable(scope),
        ),
      onSuccess: () => {
        void scope.queryClient.invalidateQueries({
          queryKey: ["vault", scope.client.scope, "attachment_list", sessionId],
        });
      },
    },
    scope.queryClient,
  );
  const { mutateAsync, reset } = mutation;
  return useCallback(
    (
      candidate: FileUploadCandidate,
      clientId: string = crypto.randomUUID(),
    ) => {
      try {
        assertUploadAvailable(scope);
      } catch (error) {
        return Promise.reject(error);
      }
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
      return mutateAsync(attempt ?? { clientId, candidate }).then(
        (metadata) => {
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
          reset();
          return metadata;
        },
      );
    },
    [scope, sessionId, mutateAsync, reset],
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
