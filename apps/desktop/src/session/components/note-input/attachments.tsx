import { useLingui } from "@lingui/react/macro";
import { onlineManager, useMutation } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as selectFile } from "@tauri-apps/plugin-dialog";
import { platform } from "@tauri-apps/plugin-os";
import {
  FileArchiveIcon,
  FileIcon,
  FileSpreadsheetIcon,
  FileTextIcon,
  FolderOpenIcon,
  ImageIcon,
  MoreHorizontalIcon,
  PaperclipIcon,
  PlusIcon,
  PresentationIcon,
  RotateCcwIcon,
  Trash2Icon,
} from "lucide-react";
import {
  type ReactNode,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useSyncExternalStore,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import {
  AttachmentActions,
  AttachmentFeedback,
  AttachmentResource,
} from "@hypr/editor/node-views";
import { Button } from "@hypr/ui/components/ui/button";
import { Spinner } from "@hypr/ui/components/ui/spinner";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn } from "@hypr/utils";

import { useSessionAttachments } from "~/session/hooks/useAttachmentResolver";
import { deviceFileName } from "~/shared/application/device-attachment-source";
import {
  ApplicationRequestError,
  connectionClosed,
} from "~/shared/application/errors";
import type { FileInfo } from "~/shared/application/protocol.gen";
import { RemoveAttachmentDialog } from "~/shared/application/remove-attachment-dialog";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { VaultAttachmentProvider } from "~/shared/application/vault-attachment-provider";
import type { VaultScope } from "~/shared/application/vault-scope";
import { formatFileSize } from "~/shared/format-file-size";
import {
  type FileUploadCandidate,
  type FileUploadState,
  useFileUpload,
  useFileUploadStates,
} from "~/shared/hooks/useFileUpload";
import { useNativeFileDrop } from "~/shared/hooks/useNativeFileDrop";

const IMAGE_EXTENSIONS = new Set([
  "avif",
  "bmp",
  "gif",
  "heic",
  "heif",
  "jpeg",
  "jpg",
  "png",
  "svg",
  "tif",
  "tiff",
  "webp",
]);
const TEXT_EXTENSIONS = new Set([
  "doc",
  "docx",
  "log",
  "md",
  "pages",
  "pdf",
  "rtf",
  "txt",
]);
const SPREADSHEET_EXTENSIONS = new Set([
  "csv",
  "numbers",
  "ods",
  "xls",
  "xlsx",
]);
const PRESENTATION_EXTENSIONS = new Set(["key", "odp", "ppt", "pptx"]);
const ARCHIVE_EXTENSIONS = new Set(["7z", "bz2", "gz", "rar", "tar", "zip"]);

export function Attachments({
  sessionId,
  children,
  dropTargetRef,
}: {
  sessionId: string;
  children?: ReactNode;
  dropTargetRef?: RefObject<HTMLDivElement | null>;
}) {
  const owner = useVaultReadScope();
  return owner.readable ? (
    <ScopedAttachments
      key={sessionId}
      sessionId={sessionId}
      scope={owner.scope}
      current={owner.current}
      dropTargetRef={dropTargetRef}
    >
      {children}
    </ScopedAttachments>
  ) : (
    <p role="alert">
      Attachment access is unavailable. Reconnect or sign in again.
    </p>
  );
}
function ScopedAttachments({
  sessionId,
  scope,
  current,
  children,
  dropTargetRef,
}: {
  sessionId: string;
  scope: VaultScope;
  current: () => boolean;
  children?: ReactNode;
  dropTargetRef?: RefObject<HTMLDivElement | null>;
}) {
  const { t } = useLingui();
  const queryClient = scope.queryClient;
  const mounted = useRef(false);
  useLayoutEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const online = useSyncExternalStore(
    (changed) => onlineManager.subscribe(changed),
    () => onlineManager.isOnline(),
  );
  const active = () => mounted.current && current();
  const requestable = () =>
    active() &&
    scope.getSnapshot().state === "connected" &&
    (scope.client.networkMode === "always" || onlineManager.isOnline());
  const canUpload = () =>
    requestable() &&
    scope.client.canEdit &&
    scope.client.features.includes("attachment_upload");
  const canRemove =
    scope.getSnapshot().state === "connected" &&
    scope.client.canEdit &&
    scope.client.features.includes("attachment_remove") &&
    scope.client.features.includes("attachment_metadata");
  const blocked =
    scope.getSnapshot().state !== "connected" ||
    (scope.client.networkMode === "online" && !online);
  const uploadDisabled =
    blocked ||
    !scope.client.canEdit ||
    !scope.client.features.includes("attachment_upload");
  const attachmentsQuery = useSessionAttachments(sessionId);
  const uploadFile = useFileUpload(sessionId);
  const uploadStates = useFileUploadStates(sessionId);
  const localTargetRef = useRef<HTMLDivElement>(null);
  const targetRef = dropTargetRef ?? localTargetRef;
  const [attachmentToDelete, setAttachmentToDelete] = useState<FileInfo | null>(
    null,
  );

  const attachments = useMemo(
    () =>
      [...(attachmentsQuery.data ?? [])].sort((left, right) =>
        left.filename.localeCompare(right.filename, undefined, {
          sensitivity: "base",
          numeric: true,
        }),
      ),
    [attachmentsQuery.data],
  );

  const addCandidates = (candidates: FileUploadCandidate[]) => {
    if (candidates.length === 0 || !canUpload()) return;
    void Promise.allSettled(
      candidates.map((candidate) => uploadFile(candidate)),
    )
      .then((results) => {
        return {
          added: results.filter((result) => result.status === "fulfilled")
            .length,
          failed: results.filter((result) => result.status === "rejected")
            .length,
        };
      })
      .then(({ added, failed }) => {
        if (!active()) return;
        if (added > 0) {
          sonnerToast.success(
            added === 1 ? t`Attachment added` : t`${added} attachments added`,
          );
        }
        if (failed > 0) {
          sonnerToast.error(
            failed === 1
              ? t`Couldn’t add 1 attachment`
              : t`Couldn’t add ${failed} attachments`,
          );
        }
      });
  };

  const { isHovering: isDraggingFiles } = useNativeFileDrop(targetRef, {
    enabled: !uploadDisabled,
    onDrop: (paths) =>
      addCandidates(
        paths.map((path) => ({
          kind: "path",
          path,
          name: deviceFileName(path),
        })),
      ),
  });

  const pickerMutation = useMutation(
    {
      networkMode: "always",
      retry: false,
      mutationFn: async () => {
        if (!canUpload()) throw connectionClosed();
        const selection = await selectFile({
          title: t`Add attachments`,
          multiple: true,
          directory: false,
        });
        if (!canUpload()) throw connectionClosed();
        return Array.isArray(selection)
          ? selection
          : selection
            ? [selection]
            : [];
      },
      onSuccess: (paths) =>
        addCandidates(
          paths.map((path) => ({
            kind: "path",
            path,
            name: deviceFileName(path),
          })),
        ),
      onError: (error) => {
        if (!active()) return;
        console.error("[attachments] file dialog failed", error);
        sonnerToast.error(t`Couldn’t choose attachments`);
      },
    },
    queryClient,
  );

  const directoryController = useRef<AbortController | null>(null);
  useEffect(
    () => () => {
      directoryController.current?.abort();
    },
    [],
  );
  const finderMutation = useMutation(
    {
      networkMode: "always",
      retry: false,
      mutationFn: async () => {
        const open = scope.client.openAttachmentDirectory;
        if (!requestable() || !open) throw connectionClosed();
        const controller = new AbortController();
        directoryController.current = controller;
        const stop = scope.subscribe(() => {
          if (!current()) controller.abort();
        });
        try {
          await scope.run(() => open(sessionId, { signal: controller.signal }));
          if (!requestable()) throw connectionClosed();
        } finally {
          stop();
          directoryController.current = null;
        }
      },
      onError: (error) => {
        if (!active()) return;
        console.error(
          "[attachments] failed to open attachment directory",
          error,
        );
        sonnerToast.error(
          platform() === "windows"
            ? t`Couldn’t open attachments in File Explorer`
            : t`Couldn’t open attachments in Finder`,
        );
      },
    },
    queryClient,
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused && mounted.current && current()) {
          void queryClient.invalidateQueries({
            queryKey: scope.attachmentsOptions(sessionId).queryKey,
          });
        }
      })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
        } else {
          unlisten = cleanup;
        }
      })
      .catch((error) => {
        console.error("[attachments] failed to watch window focus", error);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient, scope, sessionId, current]);

  const dropOverlay = (
    <div className="border-primary bg-background/85 pointer-events-none absolute inset-0 z-40 flex items-center justify-center rounded-lg border border-dashed backdrop-blur-[1px]">
      <div className="border-primary text-foreground flex items-center gap-2 rounded-lg border border-dashed px-4 py-3 text-sm font-medium">
        <PaperclipIcon className="size-4" />
        {t`Drop files to attach them`}
      </div>
    </div>
  );

  return (
    <div
      ref={dropTargetRef ? undefined : localTargetRef}
      data-allow-file-drop={dropTargetRef ? undefined : "true"}
      className={cn([
        "relative min-h-full rounded-lg border border-transparent pb-6 transition-none",
      ])}
    >
      {children}

      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-sm font-semibold">{t`Attachments`}</h2>
          {!attachmentsQuery.isLoading && !attachmentsQuery.isError ? (
            <p className="text-muted-foreground text-xs">
              {attachmentsQuery.hasNextPage
                ? t`More files are available`
                : attachments.length + uploadStates.states.length === 1
                  ? t`1 file`
                  : t`${attachments.length + uploadStates.states.length} files`}
            </p>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {scope.client.openAttachmentDirectory && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={blocked || finderMutation.isPending}
              onClick={() => finderMutation.mutate()}
            >
              {finderMutation.isPending ? (
                <Spinner size={14} />
              ) : (
                <FolderOpenIcon className="size-3.5" />
              )}
              {platform() === "windows"
                ? t`Show in File Explorer`
                : t`Show in Finder`}
            </Button>
          )}
          <Button
            type="button"
            size="sm"
            disabled={uploadDisabled || pickerMutation.isPending}
            onClick={() => pickerMutation.mutate()}
          >
            {pickerMutation.isPending ? (
              <Spinner size={14} />
            ) : (
              <PlusIcon className="size-3.5" />
            )}
            {pickerMutation.isPending ? t`Choosing…` : t`Add attachments`}
          </Button>
        </div>
      </div>

      {attachmentsQuery.isLoading && uploadStates.states.length === 0 ? (
        <div className="flex flex-col gap-2">
          {[0, 1, 2].map((row) => (
            <div key={row} className="bg-muted h-11 animate-pulse rounded-md" />
          ))}
        </div>
      ) : attachmentsQuery.isError && uploadStates.states.length === 0 ? (
        <div className="border-border flex min-h-40 flex-col items-center justify-center gap-3 rounded-lg border border-dashed px-6 text-center">
          <p className="text-muted-foreground text-sm">
            {t`Couldn’t load attachments.`}
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={blocked || !attachmentsQuery.supported}
            onClick={() => void attachmentsQuery.refetch()}
          >
            {t`Try again`}
          </Button>
        </div>
      ) : attachments.length === 0 && uploadStates.states.length === 0 ? (
        <div className="border-border flex min-h-40 flex-col items-center justify-center gap-2 rounded-lg border border-dashed px-6 text-center">
          <PaperclipIcon className="text-muted-foreground size-5" />
          <p className="text-sm font-medium">{t`No attachments yet`}</p>
          <p className="text-muted-foreground max-w-sm text-xs">
            {t`Add files here or drop them anywhere in this area.`}
          </p>
        </div>
      ) : (
        <div className="border-border overflow-hidden rounded-lg border">
          <div className="text-muted-foreground grid grid-cols-[minmax(0,1fr)_7rem_6rem_4rem] items-center gap-3 border-b px-3 py-2 text-[11px] font-medium uppercase">
            <span>{t`Name`}</span>
            <span>{t`Type`}</span>
            <span className="text-right">{t`Size`}</span>
            <span className="sr-only">{t`Actions`}</span>
          </div>
          {uploadStates.states.map((upload) => (
            <PendingAttachmentRow
              key={upload.clientId}
              upload={upload}
              onRetry={() => {
                void uploadFile(upload.candidate, upload.clientId).catch(
                  () => {},
                );
              }}
              onRemove={() => uploadStates.remove(upload.clientId)}
            />
          ))}
          {attachments.map((attachment) => (
            <AttachmentRow
              key={attachment.filename}
              attachment={attachment}
              sessionId={sessionId}
              scope={scope}
              current={active}
              blocked={blocked}
              onDelete={
                canRemove && !blocked
                  ? () => setAttachmentToDelete(attachment)
                  : undefined
              }
            />
          ))}
        </div>
      )}

      {attachmentsQuery.hasNextPage && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={blocked || attachmentsQuery.isFetching}
          onClick={() => void attachmentsQuery.fetchNextPage()}
        >{t`Load more`}</Button>
      )}

      {isDraggingFiles
        ? dropTargetRef && targetRef.current
          ? createPortal(dropOverlay, targetRef.current)
          : dropOverlay
        : null}

      {attachmentToDelete && scope && (
        <RemoveAttachmentDialog
          key={attachmentToDelete.filename}
          scope={scope}
          sessionId={sessionId}
          attachmentId={attachmentToDelete.filename}
          onClose={() => setAttachmentToDelete(null)}
          onRemoved={() => {
            if (!active()) return;
            void queryClient.invalidateQueries({
              queryKey: scope.attachmentsOptions(sessionId).queryKey,
            });
            sonnerToast.success(
              t`“${attachmentToDelete.filename}” moved to Trash`,
            );
            setAttachmentToDelete(null);
          }}
        />
      )}
    </div>
  );
}

function AttachmentRow({
  attachment,
  sessionId,
  scope,
  current,
  blocked,
  onDelete,
}: {
  attachment: FileInfo;
  sessionId: string;
  scope: VaultScope;
  current: () => boolean;
  blocked: boolean;
  onDelete?: () => void;
}) {
  const { t } = useLingui();
  const [actions, setActions] = useState(false);
  const Icon = getFileIcon(extensionOfName(attachment.filename));

  const controller = useRef<AbortController | null>(null);
  useEffect(
    () => () => {
      controller.current?.abort();
    },
    [],
  );
  const openMutation = useMutation(
    {
      networkMode: "always",
      retry: false,
      mutationFn: async () => {
        if (
          !current() ||
          scope.getSnapshot().state !== "connected" ||
          (scope.client.networkMode === "online" && !onlineManager.isOnline())
        )
          throw connectionClosed();
        const active = new AbortController();
        controller.current = active;
        const stop = scope.subscribe(() => {
          if (!current()) active.abort();
        });
        let resource: Awaited<ReturnType<typeof scope.client.openAttachment>> =
          null;
        try {
          resource = await scope.run(() =>
            scope.client.openAttachment(sessionId, attachment.filename, {
              signal: active.signal,
            }),
          );
          active.signal.throwIfAborted();
          if (!current() || !resource) throw connectionClosed();
          await resource.open({ signal: active.signal });
        } finally {
          stop();
          controller.current = null;
          await resource?.close().catch(() => {});
        }
      },
      onError: (error) => {
        if (!current()) return;
        console.error("[attachments] failed to open attachment", error);
        sonnerToast.error(
          error instanceof ApplicationRequestError &&
            error.detail.mutation_outcome === "unknown"
            ? t`Could not confirm the file action. A copy may already be in Downloads/Loofah.`
            : t`Couldn’t open “${attachment.filename}”`,
        );
      },
    },
    scope.queryClient,
  );

  return (
    <div className="border-border border-b last:border-b-0">
      <div className="hover:bg-accent/50 flex items-center">
        <button
          type="button"
          className="grid min-w-0 flex-1 grid-cols-[minmax(0,1fr)_7rem_6rem] items-center gap-3 px-3 py-2.5 text-left"
          disabled={blocked || openMutation.isPending}
          onClick={() => openMutation.mutate()}
          title={t`Open ${attachment.filename}`}
        >
          <span className="flex min-w-0 items-center gap-2.5">
            {openMutation.isPending ? (
              <Spinner size={16} className="shrink-0" />
            ) : (
              <Icon className="text-muted-foreground size-4 shrink-0" />
            )}
            <span className="truncate text-sm font-medium">
              {attachment.filename}
            </span>
          </span>
          <span className="text-muted-foreground truncate text-xs">
            {formatFileType(extensionOfName(attachment.filename))}
          </span>
          <span className="text-muted-foreground text-right text-xs tabular-nums">
            {formatFileSize(attachment.sizeBytes)}
          </span>
        </button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="text-muted-foreground size-7"
          disabled={blocked}
          aria-expanded={actions}
          aria-label={t`Actions for ${attachment.filename}`}
          onClick={() => setActions((value) => !value)}
        >
          <MoreHorizontalIcon className="size-3.5" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="text-muted-foreground mr-1 size-7"
          aria-label={t`Remove ${attachment.filename}`}
          title={t`Remove attachment`}
          disabled={!onDelete}
          onClick={onDelete}
        >
          <Trash2Icon className="size-3.5" />
        </Button>
      </div>
      {actions && (
        <VaultAttachmentProvider scope={scope} sessionId={sessionId}>
          <AttachmentResource attachmentId={attachment.filename}>
            {(resource) =>
              resource && (
                <div className="px-3 pb-2">
                  <div className="flex gap-1">
                    <AttachmentActions resource={resource} />
                  </div>
                  <AttachmentFeedback resource={resource} />
                </div>
              )
            }
          </AttachmentResource>
        </VaultAttachmentProvider>
      )}
    </div>
  );
}

function PendingAttachmentRow({
  upload,
  onRetry,
  onRemove,
}: {
  upload: FileUploadState;
  onRetry: () => void;
  onRemove: () => void;
}) {
  const { t } = useLingui();
  const uncertain =
    upload.status === "error" &&
    (!(upload.error instanceof ApplicationRequestError) ||
      upload.error.detail.mutation_outcome !== "not_committed");
  const extension = extensionOfName(upload.candidate.name);
  const Icon = getFileIcon(extension);
  const size =
    upload.candidate.kind === "file" ? upload.candidate.file.size : null;

  return (
    <div className="border-border flex items-center border-b last:border-b-0">
      <div className="grid min-w-0 flex-1 grid-cols-[minmax(0,1fr)_7rem_6rem] items-center gap-3 px-3 py-2.5 text-left">
        <span className="flex min-w-0 items-center gap-2.5">
          {upload.status === "pending" ? (
            <Spinner size={16} className="shrink-0" />
          ) : (
            <Icon className="text-destructive size-4 shrink-0" />
          )}
          <span className="min-w-0">
            <span className="block truncate text-sm font-medium">
              {upload.candidate.name}
            </span>
            <span
              className={cn([
                "block truncate text-xs",
                upload.status === "error"
                  ? "text-destructive"
                  : "text-muted-foreground",
              ])}
              title={
                upload.status === "error"
                  ? errorMessage(upload.error)
                  : undefined
              }
            >
              {upload.status === "pending"
                ? t`Copying…`
                : uncertain
                  ? t`Upload could not be confirmed`
                  : t`Copy failed`}
            </span>
          </span>
        </span>
        <span className="text-muted-foreground truncate text-xs">
          {formatFileType(extension)}
        </span>
        <span className="text-muted-foreground text-right text-xs tabular-nums">
          {size == null ? "—" : formatFileSize(size)}
        </span>
      </div>
      {upload.status === "error" ? (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="text-muted-foreground size-7"
          aria-label={t`Retry ${upload.candidate.name}`}
          title={t`Retry`}
          onClick={onRetry}
        >
          <RotateCcwIcon className="size-3.5" />
        </Button>
      ) : null}
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="text-muted-foreground mr-1 size-7"
        aria-label={t`Remove ${upload.candidate.name}`}
        title={t`Remove`}
        disabled={upload.status === "pending"}
        onClick={onRemove}
      >
        <Trash2Icon className="size-3.5" />
      </Button>
    </div>
  );
}

function getFileIcon(extension: string) {
  const normalized = extension.toLowerCase();
  if (IMAGE_EXTENSIONS.has(normalized)) return ImageIcon;
  if (TEXT_EXTENSIONS.has(normalized)) return FileTextIcon;
  if (SPREADSHEET_EXTENSIONS.has(normalized)) return FileSpreadsheetIcon;
  if (PRESENTATION_EXTENSIONS.has(normalized)) return PresentationIcon;
  if (ARCHIVE_EXTENSIONS.has(normalized)) return FileArchiveIcon;
  return FileIcon;
}

export function formatFileType(extension: string) {
  return extension ? extension.toLocaleUpperCase() : "File";
}

export { formatFileSize } from "~/shared/format-file-size";

function extensionOfName(name: string) {
  const index = name.lastIndexOf(".");
  return index > 0 ? name.slice(index + 1) : "";
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
