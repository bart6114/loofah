import { DownloadIcon, ExternalLinkIcon, FolderIcon } from "lucide-react";

import type { AttachmentResolution } from "./attachment-resolver";

export function AttachmentActions({
  resource,
}: {
  resource: AttachmentResolution;
}) {
  return (
    <>
      {(
        [
          [
            resource.open,
            resource.labels?.open ?? "Open file",
            ExternalLinkIcon,
          ],
          [
            resource.reveal,
            resource.labels?.reveal ?? "Show in Finder",
            FolderIcon,
          ],
          [
            resource.save,
            resource.labels?.save ?? "Save original copy",
            DownloadIcon,
          ],
        ] as const
      ).map(([action, label, Icon], index) =>
        action ? (
          <button
            key={index}
            type="button"
            disabled={resource.pending}
            onMouseDown={(event) => {
              event.preventDefault();
              event.stopPropagation();
            }}
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void action();
            }}
            className="hover:bg-accent rounded p-1 disabled:opacity-50"
            title={label}
            aria-label={label}
          >
            <Icon size={14} className="text-muted-foreground" />
          </button>
        ) : null,
      )}
    </>
  );
}

export function AttachmentFeedback({
  resource,
}: {
  resource: AttachmentResolution;
}) {
  if (!resource.feedback) return null;
  return (
    <div
      contentEditable={false}
      className="text-muted-foreground flex items-center gap-2 py-1 text-xs"
    >
      <span role={resource.feedback.error ? "alert" : "status"}>
        {resource.feedback.message}
      </span>
      {resource.retry && resource.feedback.error && (
        <button
          type="button"
          disabled={resource.pending}
          onMouseDown={(event) => {
            event.preventDefault();
            event.stopPropagation();
          }}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            void resource.retry?.();
          }}
          className="hover:text-foreground underline disabled:opacity-50"
        >
          {resource.labels?.retry ?? "Retry"}
        </button>
      )}
    </div>
  );
}
