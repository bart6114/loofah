import {
  useMutation,
  useMutationState,
  useQueryClient,
} from "@tanstack/react-query";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useCallback } from "react";

import type { FileUploadCandidate } from "@hypr/editor/note";
import type { AttachmentInfo } from "@hypr/plugin-fs-sync";

import { sessionAttachmentPathsQueryKey } from "~/session/hooks/useAttachmentResolver";
import {
  type AttachmentUploadAttempt,
  attachmentUploadStopped,
  uploadAttachmentAttempt,
} from "~/shared/application/attachment-upload-attempt";
import { useVaultHost } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

export type { FileUploadCandidate } from "@hypr/editor/note";

export type FileUploadResult = {
  url: string;
  path: string;
  attachmentId: string;
  attachment: AttachmentInfo;
};

export type FileUploadState = {
  clientId: string;
  candidate: FileUploadCandidate;
  status: "pending" | "error";
  error: unknown;
  submittedAt: number;
};

type FileUploadVariables = AttachmentUploadAttempt & {
  scope: VaultScope;
  sessionId: string;
};

function attachmentUploadMutationKey(
  scope: VaultScope | null,
  sessionId: string,
) {
  return [
    "vault",
    scope?.client.scope ?? null,
    "attachment_upload",
    sessionId,
  ] as const;
}

export function useFileUpload(sessionId: string) {
  const queryClient = useQueryClient();
  const { scope } = useVaultHost();
  const mutation = useMutation({
    mutationKey: attachmentUploadMutationKey(scope, sessionId),
    scope: {
      id: JSON.stringify(["attachment_upload", scope?.client.scope, sessionId]),
    },
    networkMode: "always",
    retry: false,
    gcTime: Infinity,
    mutationFn: async (attempt: FileUploadVariables) => {
      const { scope: owner, sessionId: target } = attempt;
      const metadata = await uploadAttachmentAttempt(owner, target, attempt);
      const resource = await owner.run(() =>
        owner.client.openAttachment(target, metadata.filename),
      );
      try {
        if (!resource?.path || owner.getSnapshot().state !== "connected")
          throw new Error(
            "The uploaded attachment is unavailable on this device",
          );
        const attachment: AttachmentInfo = {
          attachmentId: metadata.filename,
          path: resource.path,
          extension: extensionOf(metadata.filename),
          size: metadata.sizeBytes,
          modifiedAt: new Date().toISOString(),
        };
        return toUploadResult(attachment);
      } finally {
        await resource?.close().catch(() => {});
      }
    },
    onSuccess: ({ attachment }, attempt) => {
      void attempt.scope.queryClient.invalidateQueries({
        queryKey: [
          "vault",
          attempt.scope.client.scope,
          "attachment_list",
          attempt.sessionId,
        ],
      });
      if (attempt.scope.getSnapshot().state !== "connected") return;
      queryClient.setQueryData<AttachmentInfo[]>(
        sessionAttachmentPathsQueryKey(attempt.sessionId),
        (current = []) => [
          ...current.filter(
            (item) => item.attachmentId !== attachment.attachmentId,
          ),
          attachment,
        ],
      );
      void queryClient.invalidateQueries({
        queryKey: sessionAttachmentPathsQueryKey(attempt.sessionId),
      });
    },
  });

  return useCallback(
    (
      candidate: FileUploadCandidate,
      clientId: string = crypto.randomUUID(),
    ) => {
      if (!scope || scope.getSnapshot().state !== "connected")
        return Promise.reject(attachmentUploadStopped());
      scope.retainUploadCache(queryClient);
      const mutationCache = queryClient.getMutationCache();
      let attempt: FileUploadVariables | undefined;
      for (const previous of mutationCache.findAll({
        mutationKey: attachmentUploadMutationKey(scope, sessionId),
        exact: true,
      })) {
        const variables = previous.state.variables as
          | FileUploadVariables
          | undefined;
        if (variables?.clientId === clientId) {
          attempt = variables;
          if (previous.state.status !== "pending")
            mutationCache.remove(previous);
        }
      }
      return mutation
        .mutateAsync(attempt ?? { candidate, clientId, scope, sessionId })
        .then((result) => {
          for (const completed of mutationCache.findAll({
            mutationKey: attachmentUploadMutationKey(scope, sessionId),
            exact: true,
          }))
            if (
              (completed.state.variables as FileUploadVariables | undefined)
                ?.clientId === clientId &&
              completed.state.status === "success"
            )
              mutationCache.remove(completed);
          mutation.reset();
          return result;
        });
    },
    [mutation.mutateAsync, mutation.reset, queryClient, scope, sessionId],
  );
}

export function useFileUploadStates(sessionId: string) {
  const queryClient = useQueryClient();
  const { scope } = useVaultHost();
  const pending = useMutationState<FileUploadState>({
    filters: {
      mutationKey: attachmentUploadMutationKey(scope, sessionId),
      status: "pending",
    },
    select: (mutation) => ({
      clientId: (mutation.state.variables as FileUploadVariables).clientId,
      candidate: (mutation.state.variables as FileUploadVariables).candidate,
      status: "pending",
      error: null,
      submittedAt: mutation.state.submittedAt,
    }),
  });
  const errors = useMutationState<FileUploadState>({
    filters: {
      mutationKey: attachmentUploadMutationKey(scope, sessionId),
      status: "error",
    },
    select: (mutation) => ({
      clientId: (mutation.state.variables as FileUploadVariables).clientId,
      candidate: (mutation.state.variables as FileUploadVariables).candidate,
      status: "error",
      error: mutation.state.error,
      submittedAt: mutation.state.submittedAt,
    }),
  });

  const latest = new Map<string, FileUploadState>();
  for (const state of [...pending, ...errors]) {
    const previous = latest.get(state.clientId);
    if (!previous || previous.submittedAt <= state.submittedAt) {
      latest.set(state.clientId, state);
    }
  }

  const remove = useCallback(
    (clientId: string) => {
      const mutationCache = queryClient.getMutationCache();
      for (const mutation of mutationCache.findAll({
        mutationKey: attachmentUploadMutationKey(scope, sessionId),
      })) {
        const variables = mutation.state.variables as
          | FileUploadVariables
          | undefined;
        if (variables?.clientId === clientId) {
          variables.removed = true;
          void variables.prepared?.close().catch(() => {});
          mutationCache.remove(mutation);
        }
      }
    },
    [queryClient, scope, sessionId],
  );

  return { states: [...latest.values()], remove };
}

function toUploadResult(attachment: AttachmentInfo): FileUploadResult {
  return {
    attachment,
    path: attachment.path,
    attachmentId: attachment.attachmentId,
    url: convertFileSrc(attachment.path),
  };
}

function extensionOf(filename: string) {
  const index = filename.lastIndexOf(".");
  return index > 0 ? filename.slice(index + 1) : "";
}
