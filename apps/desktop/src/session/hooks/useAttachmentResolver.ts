import { useCallback, useMemo } from "react";

import type { AttachmentResolver } from "@hypr/editor/node-views";
import {
  type AttachmentInfo,
  commands as fsSyncCommands,
} from "@hypr/plugin-fs-sync";

import { useIndexQuery } from "~/shared/index-query";
import { mediaSrc } from "~/shared/media-src";

export function sessionAttachmentPathsQueryKey(sessionId: string) {
  return ["session", sessionId, "attachment-paths"] as const;
}

export function useSessionAttachments(sessionId: string) {
  return useIndexQuery({
    entity: "artifacts",
    refetchOnMount: "always",
    ids: [sessionId],
    queryKey: sessionAttachmentPathsQueryKey(sessionId),
    queryFn: async () => {
      const result = await fsSyncCommands.attachmentList(sessionId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    refetchInterval: (query) =>
      query.state.data?.some((attachment) => /^https?:/.test(attachment.path))
        ? 240_000
        : false,
    retry: false,
  });
}

export function useAttachmentResolver(sessionId: string): AttachmentResolver {
  const { data = EMPTY_ATTACHMENTS } = useSessionAttachments(sessionId);
  const attachments = useMemo(
    () =>
      new Map(
        data.map((attachment) => [
          attachment.attachmentId,
          {
            path: attachment.path,
            src: mediaSrc(attachment.path),
          },
        ]),
      ),
    [data],
  );

  return useCallback(
    (attachmentId: string) => attachments.get(attachmentId) ?? null,
    [attachments],
  );
}

const EMPTY_ATTACHMENTS: AttachmentInfo[] = [];
