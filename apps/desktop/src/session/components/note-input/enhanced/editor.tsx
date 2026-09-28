import type { EditorView } from "prosemirror-view";
import { forwardRef, memo, useCallback, useMemo } from "react";

import { parseJsonContent } from "@hypr/editor/markdown";
import {
  type FileHandlerConfig,
  NoteEditor,
  type JSONContent,
  type NoteEditorRef,
  normalizePortableAttachmentUrls,
} from "@hypr/editor/note";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

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
import {
  useUpdateEnhancedNoteContent,
  useRefreshEnhancedNote,
} from "~/session/queries";
import {
  ensureFirstLineTitle,
  extractFirstLineTitle,
  documentTitlePlaceholder,
} from "~/session/title-content";

const extraNodeViews = { appLink: AppLinkView, session: SessionNodeView };

const EnhancedEditorInner = forwardRef<
  NoteEditorRef,
  {
    sessionId: string;
    sessionTitle: string;
    enhancedNoteId: string;
    content: string;
    generationId?: string;
    contentOverride?: JSONContent;
    fileHandlerConfig?: FileHandlerConfig;
    onNavigateToTitle?: (pixelWidth?: number) => void;
    onViewReady?: (view: EditorView) => void;
    onViewDisposed?: (view: EditorView) => void;
    titleTrailerElement?: HTMLElement;
  }
>(
  (
    {
      sessionId,
      sessionTitle,
      enhancedNoteId,
      content,
      generationId,
      contentOverride,
      fileHandlerConfig,
      onNavigateToTitle,
      onViewReady,
      onViewDisposed,
      titleTrailerElement,
    },
    ref,
  ) => {
    const refreshContent = useRefreshEnhancedNote(
      enhancedNoteId,
      sessionId,
      generationId,
    );
    const resolveAttachment = useAttachmentResolver(sessionId);
    const updateContent = useUpdateEnhancedNoteContent(
      enhancedNoteId,
      sessionId,
    );

    const initialContent = useMemo<JSONContent>(
      () =>
        ensureFirstLineTitle(
          contentOverride ?? parseJsonContent(content),
          sessionTitle,
        ),
      [content, contentOverride, sessionTitle],
    );
    const persistChanges = contentOverride === undefined;
    const draftKey = `session:${sessionId}:enhanced:${enhancedNoteId}`;
    const { initialDraft, confirmedDraft } = usePendingNoteDraft(
      persistChanges ? draftKey : undefined,
      initialContent,
      generationId,
    );
    const handleDraftChange = useCallback(
      (getContent: () => JSONContent) =>
        captureNoteDraft(draftKey, getContent, generationId),
      [draftKey, generationId],
    );
    const editorKey = persistChanges
      ? `enhanced-note-${enhancedNoteId}`
      : `enhanced-note-${enhancedNoteId}-preview`;

    const handleChange = useCallback(
      (input: JSONContent) => {
        const portableInput = normalizePortableAttachmentUrls(input);
        const title = extractFirstLineTitle(portableInput);
        const nextTitle =
          title !== null || hasStoredNoteContent(content)
            ? (title ?? "")
            : undefined;
        return persistNoteDraft(
          draftKey,
          portableInput,
          async () => {
            await updateContent(JSON.stringify(portableInput), nextTitle);
            return refreshContent();
          },
          generationId,
        ).catch((error) => {
          console.error("[enhanced-editor] failed to persist summary", error);
          sonnerToast.error(`Summary is NOT being saved: ${error}`, {
            id: `summary-save-failed:${enhancedNoteId}`,
          });
          throw error;
        });
      },
      [
        content,
        draftKey,
        enhancedNoteId,
        generationId,
        refreshContent,
        updateContent,
      ],
    );

    const mentionConfig = useMentionConfig();

    return (
      <div className="relative h-full">
        <NoteEditor
          ref={ref}
          className="session-note-editor enhanced-summary-editor"
          key={editorKey}
          initialContent={initialContent}
          initialDraft={initialDraft}
          confirmedDraft={confirmedDraft}
          resolveAttachment={resolveAttachment}
          handleChange={persistChanges ? handleChange : undefined}
          onDraftChange={persistChanges ? handleDraftChange : undefined}
          placeholderComponent={documentTitlePlaceholder}
          mentionConfig={mentionConfig}
          sessionMentionDropConfig={sessionMentionDropConfig}
          onNavigateToTitle={onNavigateToTitle}
          onLinkOpen={openEditorLink}
          fileHandlerConfig={fileHandlerConfig}
          taskSource={
            persistChanges
              ? {
                  type:
                    enhancedNoteId === sessionId
                      ? "session_summary"
                      : "enhanced_note",
                  id: enhancedNoteId,
                }
              : undefined
          }
          extraNodeViews={extraNodeViews}
          onViewReady={onViewReady}
          onViewDisposed={onViewDisposed}
          syncContentWhenFocused={!persistChanges}
          titleTrailerElement={titleTrailerElement}
        />
      </div>
    );
  },
);

export const EnhancedEditor = memo(EnhancedEditorInner);
