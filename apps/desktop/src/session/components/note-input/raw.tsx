import type { EditorView } from "prosemirror-view";
import { forwardRef, useCallback, useMemo } from "react";

import { json2md, parseJsonContent } from "@hypr/editor/markdown";
import {
  type FileHandlerConfig,
  NoteEditor,
  type JSONContent,
  type NoteEditorRef,
  normalizePortableAttachmentUrls,
} from "@hypr/editor/note";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn } from "@hypr/utils";

import { AppLinkView } from "~/editor-bridge/app-link-view";
import { useMentionConfig } from "~/editor-bridge/mention-config";
import { openEditorLink } from "~/editor-bridge/open-editor-link";
import { sessionMentionDropConfig } from "~/editor-bridge/session-mention-drop";
import { SessionNodeView } from "~/editor-bridge/session-view";
import { hasStoredNoteContent } from "~/session/components/shared";
import { useAttachmentResolver } from "~/session/hooks/useAttachmentResolver";
import { useUpdateSession } from "~/session/queries";
import {
  ensureFirstLineTitle,
  extractFirstLineTitle,
  documentTitlePlaceholder,
} from "~/session/title-content";
import { enqueueDatabaseWrite } from "~/shared/write-queue";
import { commands } from "~/types/tauri.gen";

const extraNodeViews = { appLink: AppLinkView, session: SessionNodeView };

export const RawEditor = forwardRef<
  NoteEditorRef,
  {
    sessionId: string;
    rawMd: string;
    sessionTitle: string;
    className?: string;
    onNavigateToTitle?: (pixelWidth?: number) => void;
    syncTasks?: boolean;
    showFormatToolbar?: boolean;
    fileHandlerConfig?: FileHandlerConfig;
    onViewReady?: (view: EditorView) => void;
    onViewDisposed?: (view: EditorView) => void;
    titleTrailerElement?: HTMLElement;
  }
>(
  (
    {
      sessionId,
      rawMd,
      sessionTitle,
      className,
      onNavigateToTitle,
      syncTasks = true,
      showFormatToolbar = true,
      fileHandlerConfig,
      onViewReady,
      onViewDisposed,
      titleTrailerElement,
    },
    ref,
  ) => {
    const updateSession = useUpdateSession(sessionId);
    const resolveAttachment = useAttachmentResolver(sessionId);
    const initialContent = useMemo<JSONContent>(
      () => ensureFirstLineTitle(parseJsonContent(rawMd), sessionTitle),
      [rawMd, sessionTitle],
    );

    const persistChange = useCallback(
      async (input: JSONContent) => {
        const portableInput = normalizePortableAttachmentUrls(input);
        const title = extractFirstLineTitle(portableInput);

        const titleWrite =
          title !== null || hasStoredNoteContent(rawMd)
            ? updateSession({ title: title ?? "" })
            : Promise.resolve();

        const markdown = json2md(portableInput);
        const noteWrite = enqueueDatabaseWrite(
          `session:${sessionId}:note`,
          async () => {
            const result = await commands.sessionWriteNote(sessionId, markdown);
            if (result.status === "error") {
              throw new Error(result.error);
            }
          },
        );

        await Promise.all([titleWrite, noteWrite]);
      },
      [rawMd, sessionId, updateSession],
    );

    const handleChange = useCallback(
      (input: JSONContent) => {
        void persistChange(input).catch((error) => {
          console.error("[raw-editor] failed to persist note", error);
          sonnerToast.error(`Note is NOT being saved: ${error}`, {
            id: `note-save-failed:${sessionId}`,
          });
        });
      },
      [persistChange, sessionId],
    );

    const mentionConfig = useMentionConfig();
    return (
      <NoteEditor
        ref={ref}
        className={cn(["session-note-editor", className])}
        key={`session-${sessionId}-raw`}
        initialContent={initialContent}
        resolveAttachment={resolveAttachment}
        handleChange={handleChange}
        placeholderComponent={documentTitlePlaceholder}
        mentionConfig={mentionConfig}
        sessionMentionDropConfig={sessionMentionDropConfig}
        onNavigateToTitle={onNavigateToTitle}
        onLinkOpen={openEditorLink}
        fileHandlerConfig={fileHandlerConfig}
        taskSource={
          syncTasks ? { type: "session_raw_note", id: sessionId } : undefined
        }
        extraNodeViews={extraNodeViews}
        showFormatToolbar={showFormatToolbar}
        onViewReady={onViewReady}
        onViewDisposed={onViewDisposed}
        titleTrailerElement={titleTrailerElement}
      />
    );
  },
);
