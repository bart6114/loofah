import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import { downloadDir, join } from "@tauri-apps/api/path";
import { type ReactNode, useEffect } from "react";
import { createPortal } from "react-dom";

import { json2md } from "@hypr/editor/markdown";
import { toPortableAttachmentSrc } from "@hypr/editor/note";
import {
  commands as exportCommands,
  type ExportAttachment,
  type ExportMetadata,
  type TranscriptItem,
} from "@hypr/plugin-export";
import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";
import { commands as openerCommands } from "@hypr/plugin-opener2";
import { cn } from "@hypr/utils";

import { formatDate, formatDuration } from "./export-utils";

import { useTranscriptExportSegments } from "~/session/components/note-input/transcript/export-data";
import { useEnhancedNote, useSession } from "~/session/queries";
import type { EditorView } from "~/store/zustand/tabs/schema";
import { useSessionTranscriptMetadata } from "~/stt/queries";

type FileFormat = "pdf" | "txt" | "md" | "org";

const EMPTY_PARTICIPANT_NAMES: string[] = [];

export function ExportModal({
  sessionId,
  currentView,
  open,
  onOpenChange,
}: {
  sessionId: string;
  currentView: EditorView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useLingui();
  const form = useForm({
    defaultValues: {
      format: "pdf" as FileFormat,
      includeNote: true,
      includeSummary: true,
      includeTranscript: false,
    },
  });
  const { format, includeNote, includeSummary, includeTranscript } = useStore(
    form.store,
    (state) => state.values,
  );

  const session = useSession(sessionId);
  const sessionTitle = session?.title;
  const sessionCreatedAt = session?.created_at;
  const rawMd = session?.raw_md;

  const enhancedNoteId = currentView.type === "enhanced" ? currentView.id : "";
  const enhancedNoteContent = useEnhancedNote(enhancedNoteId)?.content;
  const participantNames: string[] = EMPTY_PARTICIPANT_NAMES;

  const metadataQuery = useSessionTranscriptMetadata(sessionId);
  const metadata = metadataQuery.data;
  const transcriptDuration =
    metadata?.started_at != null && metadata.ended_at != null
      ? formatDuration(metadata.started_at, metadata.ended_at)
      : null;

  const getNoteMd = (): string => {
    if (!rawMd) return "";
    try {
      const parsed = JSON.parse(rawMd);
      return json2md(parsed);
    } catch {
      return "";
    }
  };

  const getSummaryMd = (): string => {
    if (!enhancedNoteContent) return "";
    try {
      const parsed = JSON.parse(enhancedNoteContent);
      return json2md(parsed);
    } catch {
      return "";
    }
  };

  const buildExportContent = (
    transcriptItems: TranscriptItem[],
  ): {
    enhancedMd: string;
    noteMd: string | null;
    transcript: { items: TranscriptItem[] } | null;
    metadata: ExportMetadata | null;
  } => {
    const metadata: ExportMetadata = {
      title: sessionTitle || t`Untitled`,
      createdAt: sessionCreatedAt ? formatDate(sessionCreatedAt) : "",
      participants: participantNames,
      duration: transcriptDuration,
    };

    let noteMd: string | null = null;
    if (includeNote) {
      const note = getNoteMd();
      if (note) noteMd = note;
    }

    const parts: string[] = [];

    if (includeSummary) {
      const summary = getSummaryMd();
      if (summary) parts.push(summary);
    }

    return {
      enhancedMd: parts.join("\n\n"),
      noteMd,
      transcript:
        includeTranscript && transcriptItems.length > 0
          ? { items: transcriptItems }
          : null,
      metadata,
    };
  };

  // Session attachments referenced as images in the exported markdown, so the
  // PDF renderer can embed them.
  const collectPdfAttachments = async (content: {
    enhancedMd: string;
    noteMd: string | null;
  }): Promise<ExportAttachment[]> => {
    const result = await fsSyncCommands.attachmentList(sessionId);
    if (result.status === "error") {
      return [];
    }
    const haystack = `${content.noteMd ?? ""}\n${content.enhancedMd}`;
    return result.data
      .map((attachment) => ({
        src: toPortableAttachmentSrc(attachment.attachmentId),
        path: attachment.path,
      }))
      .filter((attachment) => haystack.includes(`](${attachment.src}`));
  };

  const {
    mutate,
    isPending,
    isSuccess,
    reset,
    error: exportError,
  } = useMutation({
    gcTime: 0,
    mutationFn: async ({
      content,
      format,
      title,
    }: {
      content: ReturnType<typeof buildExportContent>;
      format: FileFormat;
      title: string;
    }) => {
      const downloadsPath = await downloadDir();
      const sanitizedTitle = (title.trim() || t`Untitled`).replace(
        /[<>:"/\\|?*]/g,
        "_",
      );
      const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
      const filename = `${sanitizedTitle}_${timestamp}.${format}`;
      const path = await join(downloadsPath, filename);

      if (format === "pdf") {
        const attachments = await collectPdfAttachments(content);
        const result = await exportCommands.export(path, {
          ...content,
          attachments,
        });
        if (result.status === "error") {
          throw new Error(result.error);
        }
      } else {
        const result = await exportCommands.exportText(path, content, format, {
          untitled: t`Untitled`,
          created: t`Created`,
          participants: t`Participants`,
          duration: t`Duration`,
          metadata: t`Metadata`,
          note: t`Note`,
          summary: t`Summary`,
          transcript: t`Transcript`,
        });
        if (result.status === "error") {
          throw new Error(result.error);
        }
      }

      return path;
    },
    onSuccess: (path) => {
      if (path) {
        void openerCommands.revealItemInDir(path);
      }
      onOpenChange(false);
    },
    onError: console.error,
  });

  useEffect(() => {
    if (!isPending && (!open || isSuccess)) reset();
  }, [isPending, isSuccess, open, reset]);

  const hasAnyContentSelected =
    includeNote || includeSummary || includeTranscript;
  const renderExportAction = ({
    data,
    isLoading,
    error,
    refetch,
  }: ReturnType<typeof useTranscriptExportSegments>) => (
    <>
      {(error || metadataQuery.error) && (
        <div role="alert" className="text-sm">
          <Trans>Couldn't prepare export.</Trans>{" "}
          <button
            className="underline"
            onClick={() => {
              if (metadataQuery.isError) void metadataQuery.refetch();
              if (error) refetch();
            }}
          >
            <Trans>Retry</Trans>
          </button>
        </div>
      )}
      <button
        onClick={() =>
          mutate({
            content: buildExportContent(data),
            format,
            title: sessionTitle ?? t`Untitled`,
          })
        }
        disabled={
          isPending ||
          isLoading ||
          metadataQuery.isPending ||
          !!error ||
          metadataQuery.isError ||
          !hasAnyContentSelected
        }
        className="border-primary bg-primary text-primary-foreground hover:bg-primary/90 h-10 w-full rounded-full border-2 text-sm font-medium shadow-[0_4px_14px_rgba(87,83,78,0.4)] transition-none duration-200 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {isPending
          ? t`Exporting...`
          : isLoading
            ? t`Preparing transcript...`
            : metadataQuery.isPending
              ? t`Preparing export...`
              : t`Export`}
      </button>
    </>
  );
  if (!open) {
    return null;
  }

  return createPortal(
    <div
      className="fixed inset-0 z-50 bg-black/20 backdrop-blur-xs"
      onClick={() => onOpenChange(false)}
    >
      <div
        className="absolute top-1/2 left-1/2 w-full max-w-sm -translate-x-1/2 -translate-y-1/2 px-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div
          className={cn([
            "border-border/80 bg-background rounded-xl border",
            "shadow-[0_25px_50px_-12px_rgba(0,0,0,0.25)]",
            "flex flex-col gap-4 p-7 text-center",
          ])}
        >
          <div className="flex flex-col gap-1">
            <h2 className="text-base font-semibold">
              <Trans>Export</Trans>
            </h2>
            <p className="text-muted-foreground text-sm">
              <Trans>Choose a file format and what to include.</Trans>
            </p>
          </div>

          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <span className="text-sm font-medium">
                <Trans>File format</Trans>
              </span>
              <div className="flex justify-center gap-4">
                {(["pdf", "txt", "md", "org"] as const).map((f) => (
                  <label
                    key={f}
                    className="flex cursor-pointer items-center gap-1.5 text-sm"
                  >
                    <input
                      type="radio"
                      name="export-format"
                      checked={format === f}
                      onChange={() => form.setFieldValue("format", f)}
                      className="accent-primary"
                    />
                    {f === "md"
                      ? "Markdown"
                      : f === "org"
                        ? "Org"
                        : f.toUpperCase()}
                  </label>
                ))}
              </div>
            </div>

            <div className="flex flex-col gap-2">
              <span className="text-sm font-medium">
                <Trans>Include</Trans>
              </span>
              <div className="flex justify-center gap-4">
                {(
                  [
                    [
                      "note",
                      <Trans>Note</Trans>,
                      includeNote,
                      (value: boolean) =>
                        form.setFieldValue("includeNote", value),
                    ],
                    [
                      "summary",
                      <Trans>Summary</Trans>,
                      includeSummary,
                      (value: boolean) =>
                        form.setFieldValue("includeSummary", value),
                    ],
                    [
                      "transcript",
                      <Trans>Transcript</Trans>,
                      includeTranscript,
                      (value: boolean) =>
                        form.setFieldValue("includeTranscript", value),
                    ],
                  ] as const
                ).map(([id, label, checked, setter]) => (
                  <label
                    key={id}
                    className="flex cursor-pointer items-center gap-1.5 text-sm"
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={(e) => setter(e.target.checked)}
                      className="accent-primary"
                    />
                    {label}
                  </label>
                ))}
              </div>
            </div>
          </div>

          {exportError && (
            <p role="alert">
              <Trans>Export failed. Please try again.</Trans>
            </p>
          )}
          {includeTranscript ? (
            <TranscriptExportAction sessionId={sessionId}>
              {(result) => renderExportAction(result)}
            </TranscriptExportAction>
          ) : (
            renderExportAction({
              data: [],
              isLoading: false,
              error: null,
              refetch: () => {},
            })
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}

function TranscriptExportAction({
  sessionId,
  children,
}: {
  sessionId: string;
  children: (
    result: ReturnType<typeof useTranscriptExportSegments>,
  ) => ReactNode;
}) {
  const result = useTranscriptExportSegments(sessionId);
  return children(result);
}
