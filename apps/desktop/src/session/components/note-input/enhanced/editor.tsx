import type { EditorView } from "prosemirror-view";
import {
  forwardRef,
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
} from "react";

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
  enhancedNoteDraftKey,
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
import { sessionEditorFlushKey } from "~/shared/application/session-cleanup-guard";
import { useVaultHost } from "~/shared/application/vault-provider";
import {
  drainDatabaseWrites,
  registerWriteFlusher,
} from "~/shared/write-queue";

const extraNodeViews = { appLink: AppLinkView, session: SessionNodeView };

const EnhancedEditorInner = forwardRef<
  NoteEditorRef,
  {
    sessionId: string;
    sessionTitle: string;
    readOnly?: boolean;
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
      readOnly = false,
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
    const { scope } = useVaultHost();
    const editorRef = useRef<NoteEditorRef | null>(null);
    const bindEditor = useCallback(
      (editor: NoteEditorRef | null) => {
        editorRef.current = editor;
        if (typeof ref === "function") ref(editor);
        else if (ref) ref.current = editor;
      },
      [ref],
    );
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
    const draftKey = scope
      ? enhancedNoteDraftKey(scope.client.scope, sessionId, enhancedNoteId)
      : undefined;
    useEffect(() => {
      if (!scope || !persistChanges) return;
      return registerWriteFlusher(
        sessionEditorFlushKey(scope.client.scope, sessionId),
        async () => {
          if (scope.getSnapshot().state === "connected" && scope.client.canEdit)
            editorRef.current?.flushPendingChanges();
          await drainDatabaseWrites([`session:${sessionId}`, "tasks"]);
        },
      );
    }, [scope, sessionId, persistChanges]);
    const { initialDraft, confirmedDraft } = usePendingNoteDraft(
      persistChanges ? draftKey : undefined,
      initialContent,
      generationId,
    );
    const handleDraftChange = useCallback(
      (getContent: () => JSONContent) => {
        if (draftKey) captureNoteDraft(draftKey, getContent, generationId);
      },
      [draftKey, generationId],
    );
    const editorKey = persistChanges
      ? `enhanced-note-${enhancedNoteId}`
      : `enhanced-note-${enhancedNoteId}-preview`;

    const handleChange = useCallback(
      (input: JSONContent) => {
        if (!draftKey)
          return Promise.reject(new Error("The editor has no vault scope"));
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
          ref={bindEditor}
          readOnly={readOnly || !scope}
          className="session-note-editor enhanced-summary-editor"
          key={editorKey}
          initialContent={initialContent}
          initialDraft={initialDraft}
          confirmedDraft={confirmedDraft}
          resolveAttachment={resolveAttachment}
          handleChange={persistChanges && scope ? handleChange : undefined}
          onDraftChange={
            persistChanges && scope ? handleDraftChange : undefined
          }
          placeholderComponent={documentTitlePlaceholder}
          mentionConfig={mentionConfig}
          sessionMentionDropConfig={
            readOnly ? undefined : sessionMentionDropConfig
          }
          onNavigateToTitle={onNavigateToTitle}
          onLinkOpen={openEditorLink}
          fileHandlerConfig={readOnly ? undefined : fileHandlerConfig}
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
