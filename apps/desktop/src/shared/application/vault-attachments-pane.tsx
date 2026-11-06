import { Trans, useLingui } from "@lingui/react/macro";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useState, useSyncExternalStore } from "react";

import {
  AttachmentResource,
  AttachmentActions,
  AttachmentFeedback,
} from "@hypr/editor/node-views";
import { Button } from "@hypr/ui/components/ui/button";

import type { FileInfo } from "./protocol.gen";
import { VaultAttachmentProvider } from "./vault-attachment-provider";
import type { VaultScope } from "./vault-scope";

import { formatFileSize } from "~/shared/format-file-size";

export function VaultAttachmentsPane({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const { t } = useLingui();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const query = useInfiniteQuery(
    {
      ...scope.attachmentsOptions(sessionId),
      enabled: connection.state === "connected",
    },
    scope.queryClient,
  );
  if (
    ["access_denied", "login_required", "closed", "incompatible"].includes(
      connection.state,
    )
  )
    return (
      <p role="alert">
        <Trans>
          Attachment access is unavailable. Reconnect or sign in again.
        </Trans>
      </p>
    );
  const files = query.data?.pages.flatMap((page) => page.files) ?? [];
  return (
    <section
      aria-label={t`Attachments`}
      className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto"
    >
      <h3 className="text-sm font-medium">
        <Trans>Attachments</Trans>
      </h3>
      {query.isPending && (
        <p role="status">
          <Trans>Loading attachments…</Trans>
        </p>
      )}
      {query.isError && (
        <div role="alert">
          <p>
            <Trans>Couldn’t load attachments.</Trans>
          </p>
          <Button
            size="sm"
            variant="outline"
            disabled={connection.state !== "connected" || query.isFetching}
            onClick={() => void query.refetch()}
          >
            <Trans>Try again</Trans>
          </Button>
        </div>
      )}
      {query.isSuccess && files.length === 0 && (
        <p>
          <Trans>No attachments yet</Trans>
        </p>
      )}
      <VaultAttachmentProvider scope={scope} sessionId={sessionId}>
        <ul className="divide-border divide-y">
          {files.map((file) => (
            <AttachmentRow key={file.filename} file={file} />
          ))}
        </ul>
      </VaultAttachmentProvider>
      {query.hasNextPage && (
        <Button
          size="sm"
          variant="outline"
          disabled={query.isFetching}
          onClick={() => void query.fetchNextPage()}
        >
          <Trans>Load more</Trans>
        </Button>
      )}
    </section>
  );
}

function AttachmentRow({ file }: { file: FileInfo }) {
  const { t } = useLingui();
  const [expanded, setExpanded] = useState(false);
  return (
    <li className="py-2">
      <button
        type="button"
        aria-expanded={expanded}
        aria-label={t`Actions for ${file.filename}`}
        onClick={() => setExpanded((value) => !value)}
        className="hover:bg-muted flex w-full items-center gap-3 rounded px-2 py-2 text-left"
      >
        <span className="min-w-0 flex-1 truncate text-sm">{file.filename}</span>
        <span
          className="text-muted-foreground max-w-40 truncate text-xs"
          title={file.contentType}
        >
          {file.contentType}
        </span>
        <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
          {formatFileSize(file.sizeBytes)}
        </span>
      </button>
      {expanded && (
        <AttachmentResource attachmentId={file.filename}>
          {(resource) =>
            resource && (
              <div className="px-2 py-1">
                <div className="flex items-center gap-1">
                  <AttachmentActions resource={resource} />
                </div>
                <AttachmentFeedback resource={resource} />
              </div>
            )
          }
        </AttachmentResource>
      )}
    </li>
  );
}
