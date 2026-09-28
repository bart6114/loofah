import type { EditorView } from "prosemirror-view";
import { forwardRef, useCallback, useMemo } from "react";

import { json2mdStrict, parseJsonContent } from "@hypr/editor/markdown";
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
import {
  captureNoteDraft,
  persistNoteDraft,
  usePendingNoteDraft,
} from "~/session/pending-note-drafts";
import { saveSessionNote, useRefreshSessionNote } from "~/session/queries";
import {
  ensureFirstLineTitle,
  extractFirstLineTitle,
  documentTitlePlaceholder,
} from "~/session/title-content";

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
    const draftKey = `session:${sessionId}:note`;
    const refreshContent = useRefreshSessionNote(sessionId);
    const resolveAttachment = useAttachmentResolver(sessionId);
    const initialContent = useMemo<JSONContent>(
      () => ensureFirstLineTitle(parseJsonContent(rawMd), sessionTitle),
      [rawMd, sessionTitle],
    );

    const { initialDraft, confirmedDraft } = usePendingNoteDraft(
      draftKey,
      initialContent,
    );
    const handleDraftChange = useCallback(
      (getContent: () => JSONContent) => captureNoteDraft(draftKey, getContent),
      [draftKey],
    );

    const handleChange = useCallback(
      (input: JSONContent) => {
        const portableInput = normalizePortableAttachmentUrls(input);
        const title = extractFirstLineTitle(portableInput);
        const nextTitle =
          title !== null || hasStoredNoteContent(rawMd)
            ? (title ?? "")
            : undefined;

        return persistNoteDraft(draftKey, portableInput, async () => {
          await saveSessionNote(
            sessionId,
            json2mdStrict(portableInput),
            nextTitle,
          );
          return refreshContent();
        }).catch((error) => {
          console.error("[raw-editor] failed to persist note", error);
          sonnerToast.error(`Note is NOT being saved: ${error}`, {
            id: `note-save-failed:${sessionId}`,
          });
          throw error;
        });
      },
      [draftKey, rawMd, refreshContent, sessionId],
    );

    const mentionConfig = useMentionConfig();
    return (
      <NoteEditor
        ref={ref}
        className={cn(["session-note-editor", className])}
        key={`session-${sessionId}-raw`}
        initialContent={initialContent}
        initialDraft={initialDraft}
        confirmedDraft={confirmedDraft}
        resolveAttachment={resolveAttachment}
        handleChange={handleChange}
        onDraftChange={handleDraftChange}
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
