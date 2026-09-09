import { useLingui } from "@lingui/react/macro";
import { useMutation, useQueryClient } from "@tanstack/react-query";
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
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import {
  type AttachmentInfo,
  commands as fsSyncCommands,
} from "@hypr/plugin-fs-sync";
import { commands as openerCommands } from "@hypr/plugin-opener2";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";
import { Spinner } from "@hypr/ui/components/ui/spinner";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn } from "@hypr/utils";

import {
  sessionAttachmentPathsQueryKey,
  useSessionAttachments,
} from "~/session/hooks/useAttachmentResolver";
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
  const { t } = useLingui();
  const queryClient = useQueryClient();
  const attachmentsQuery = useSessionAttachments(sessionId);
  const uploadFile = useFileUpload(sessionId);
  const uploadStates = useFileUploadStates(sessionId);
  const localTargetRef = useRef<HTMLDivElement>(null);
  const targetRef = dropTargetRef ?? localTargetRef;
  const [attachmentToDelete, setAttachmentToDelete] =
    useState<AttachmentInfo | null>(null);

  const attachments = useMemo(
    () =>
      [...(attachmentsQuery.data ?? [])].sort((left, right) =>
        left.attachmentId.localeCompare(right.attachmentId, undefined, {
          sensitivity: "base",
          numeric: true,
        }),
      ),
    [attachmentsQuery.data],
  );

  const addCandidates = (candidates: FileUploadCandidate[]) => {
    if (candidates.length === 0) return;
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
    onDrop: (paths) =>
      addCandidates(
        paths.map((path) => ({
          kind: "path",
          path,
          name: fileNameFromPath(path),
        })),
      ),
  });

  const pickerMutation = useMutation({
    mutationFn: async () => {
      const selection = await selectFile({
        title: t`Add attachments`,
        multiple: true,
        directory: false,
      });
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
          name: fileNameFromPath(path),
        })),
      ),
    onError: (error) => {
      console.error("[attachments] file dialog failed", error);
      sonnerToast.error(t`Couldn’t choose attachments`);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: async (attachment: AttachmentInfo) => {
      const result = await fsSyncCommands.attachmentRemove(
        sessionId,
        attachment.attachmentId,
      );
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return attachment;
    },
    onSuccess: (attachment) => {
      void queryClient.invalidateQueries({
        queryKey: sessionAttachmentPathsQueryKey(sessionId),
      });
      setAttachmentToDelete(null);
      sonnerToast.success(t`“${attachment.attachmentId}” moved to Trash`);
    },
    onError: (error) => {
      console.error("[attachments] failed to remove attachment", error);
      sonnerToast.error(t`Couldn’t remove attachment`);
    },
  });

  const finderMutation = useMutation({
    mutationFn: async () => {
      const dirResult = await fsSyncCommands.attachmentDir(sessionId);
      if (dirResult.status === "error") {
        throw new Error(dirResult.error);
      }
      const openResult = await openerCommands.openPath(dirResult.data, null);
      if (openResult.status === "error") {
        throw new Error(openResult.error);
      }
    },
    onError: (error) => {
      console.error(
        "[attachments] failed to open attachments directory",
        error,
      );
      sonnerToast.error(
        platform() === "windows"
          ? t`Couldn’t open attachments in File Explorer`
          : t`Couldn’t open attachments in Finder`,
      );
    },
  });

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused) {
          void queryClient.invalidateQueries({
            queryKey: sessionAttachmentPathsQueryKey(sessionId),
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
  }, [queryClient, sessionId]);

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
        "relative min-h-full rounded-lg border border-transparent pb-6 transition-colors",
      ])}
    >
      {children}

      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-sm font-semibold">{t`Attachments`}</h2>
          {!attachmentsQuery.isLoading && !attachmentsQuery.isError ? (
            <p className="text-muted-foreground text-xs">
              {attachments.length + uploadStates.states.length === 1
                ? t`1 file`
                : t`${attachments.length + uploadStates.states.length} files`}
            </p>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={finderMutation.isPending}
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
          <Button
            type="button"
            size="sm"
            disabled={pickerMutation.isPending}
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
          <div className="text-muted-foreground grid grid-cols-[minmax(0,1fr)_7rem_6rem_2rem] items-center gap-3 border-b px-3 py-2 text-[11px] font-medium uppercase">
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
              key={attachment.attachmentId}
              attachment={attachment}
              onDelete={() => setAttachmentToDelete(attachment)}
            />
          ))}
        </div>
      )}

      {isDraggingFiles
        ? dropTargetRef && targetRef.current
          ? createPortal(dropOverlay, targetRef.current)
          : dropOverlay
        : null}

      <Dialog
        open={attachmentToDelete !== null}
        onOpenChange={(open) => {
          if (!open && !deleteMutation.isPending) {
            setAttachmentToDelete(null);
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t`Remove attachment?`}</DialogTitle>
            <DialogDescription>
              {attachmentToDelete
                ? t`“${attachmentToDelete.attachmentId}” will be moved to the vault Trash. Links to this file in notes or summaries may stop working.`
                : ""}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <DialogClose asChild>
              <Button
                type="button"
                variant="outline"
                disabled={deleteMutation.isPending}
              >
                {t`Cancel`}
              </Button>
            </DialogClose>
            <Button
              type="button"
              variant="destructive"
              disabled={!attachmentToDelete || deleteMutation.isPending}
              onClick={() => {
                if (attachmentToDelete) {
                  deleteMutation.mutate(attachmentToDelete);
                }
              }}
            >
              {deleteMutation.isPending ? <Spinner size={14} /> : null}
              {deleteMutation.isPending ? t`Removing…` : t`Move to Trash`}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function AttachmentRow({
  attachment,
  onDelete,
}: {
  attachment: AttachmentInfo;
  onDelete: () => void;
}) {
  const { t } = useLingui();
  const Icon = getFileIcon(attachment.extension);

  const openMutation = useMutation({
    mutationFn: async () => {
      const result = await openerCommands.openPath(attachment.path, null);
      if (result.status === "error") {
        throw new Error(result.error);
      }
    },
    onError: (error) => {
      console.error("[attachments] failed to open attachment", error);
      sonnerToast.error(t`Couldn’t open “${attachment.attachmentId}”`);
    },
  });

  return (
    <div className="border-border hover:bg-accent/50 flex items-center border-b last:border-b-0">
      <button
        type="button"
        className="grid min-w-0 flex-1 grid-cols-[minmax(0,1fr)_7rem_6rem] items-center gap-3 px-3 py-2.5 text-left"
        disabled={openMutation.isPending}
        onClick={() => openMutation.mutate()}
        title={t`Open ${attachment.attachmentId}`}
      >
        <span className="flex min-w-0 items-center gap-2.5">
          {openMutation.isPending ? (
            <Spinner size={16} className="shrink-0" />
          ) : (
            <Icon className="text-muted-foreground size-4 shrink-0" />
          )}
          <span className="truncate text-sm font-medium">
            {attachment.attachmentId}
          </span>
        </span>
        <span className="text-muted-foreground truncate text-xs">
          {formatFileType(attachment.extension)}
        </span>
        <span className="text-muted-foreground text-right text-xs tabular-nums">
          {formatFileSize(attachment.size)}
        </span>
      </button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="text-muted-foreground mr-1 size-7"
        aria-label={t`Remove ${attachment.attachmentId}`}
        title={t`Remove attachment`}
        onClick={onDelete}
      >
        <Trash2Icon className="size-3.5" />
      </Button>
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
              {upload.status === "pending" ? t`Copying…` : t`Copy failed`}
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

export function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function fileNameFromPath(path: string) {
  return path.split("/").pop() ?? path;
}

function extensionOfName(name: string) {
  const index = name.lastIndexOf(".");
  return index > 0 ? name.slice(index + 1) : "";
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
