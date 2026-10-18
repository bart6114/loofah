import {
  useMutation,
  useMutationState,
  useQueryClient,
} from "@tanstack/react-query";
import { useCallback } from "react";

import {
  type AttachmentInfo,
  commands as fsSyncCommands,
} from "@hypr/plugin-fs-sync";

import { sessionAttachmentPathsQueryKey } from "~/session/hooks/useAttachmentResolver";
import { mediaSrc } from "~/shared/media-src";

export type FileUploadCandidate =
  | { kind: "path"; path: string; name: string }
  | { kind: "file"; file: File; name: string };

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

type FileUploadVariables = {
  clientId: string;
  candidate: FileUploadCandidate;
};

function attachmentUploadMutationKey(sessionId: string) {
  return ["attachment-upload", sessionId] as const;
}

export function useFileUpload(sessionId: string) {
  const queryClient = useQueryClient();
  const { mutateAsync } = useMutation({
    mutationKey: attachmentUploadMutationKey(sessionId),
    scope: { id: `attachment-upload:${sessionId}` },
    mutationFn: async ({ candidate }: FileUploadVariables) => {
      if (candidate.kind === "path") {
        const result = await fsSyncCommands.attachmentImportPath(
          sessionId,
          candidate.path,
        );
        if (result.status === "error") throw new Error(result.error);
        return toUploadResult(result.data);
      }

      const arrayBuffer = await candidate.file.arrayBuffer();
      const data = Array.from(new Uint8Array(arrayBuffer));
      const result = await fsSyncCommands.attachmentSave(
        sessionId,
        data,
        candidate.name,
      );
      if (result.status === "error") throw new Error(result.error);

      const attachment: AttachmentInfo = {
        ...result.data,
        extension: extensionOf(result.data.attachmentId),
        size: candidate.file.size,
        modifiedAt: new Date().toISOString(),
      };
      return toUploadResult(attachment);
    },
    onSuccess: ({ attachment }) => {
      queryClient.setQueryData<AttachmentInfo[]>(
        sessionAttachmentPathsQueryKey(sessionId),
        (current = []) => [
          ...current.filter(
            (item) => item.attachmentId !== attachment.attachmentId,
          ),
          attachment,
        ],
      );
      void queryClient.invalidateQueries({
        queryKey: sessionAttachmentPathsQueryKey(sessionId),
      });
    },
  });

  return useCallback(
    (
      candidate: FileUploadCandidate,
      clientId: string = crypto.randomUUID(),
    ) => {
      const mutationCache = queryClient.getMutationCache();
      for (const previous of mutationCache.findAll({
        mutationKey: attachmentUploadMutationKey(sessionId),
      })) {
        const variables = previous.state.variables as
          | FileUploadVariables
          | undefined;
        if (variables?.clientId === clientId) mutationCache.remove(previous);
      }
      return mutateAsync({ candidate, clientId });
    },
    [mutateAsync, queryClient, sessionId],
  );
}

export function useFileUploadStates(sessionId: string) {
  const queryClient = useQueryClient();
  const pending = useMutationState<FileUploadState>({
    filters: {
      mutationKey: attachmentUploadMutationKey(sessionId),
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
      mutationKey: attachmentUploadMutationKey(sessionId),
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
        mutationKey: attachmentUploadMutationKey(sessionId),
      })) {
        const variables = mutation.state.variables as
          | FileUploadVariables
          | undefined;
        if (variables?.clientId === clientId) mutationCache.remove(mutation);
      }
    },
    [queryClient, sessionId],
  );

  return { states: [...latest.values()], remove };
}

function toUploadResult(attachment: AttachmentInfo): FileUploadResult {
  return {
    attachment,
    path: attachment.path,
    attachmentId: attachment.attachmentId,
    url: mediaSrc(attachment.path),
  };
}

function extensionOf(filename: string) {
  const index = filename.lastIndexOf(".");
  return index > 0 ? filename.slice(index + 1) : "";
}
