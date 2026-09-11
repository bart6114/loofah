import type { EditorView } from "prosemirror-view";
import { forwardRef, useMemo } from "react";

import type { FileHandlerConfig, NoteEditorRef } from "@hypr/editor/note";

import { EnhancedEditor } from "./editor";
import { EmptySummary } from "./empty-summary";
import { EnhanceError } from "./enhance-error";
import { StreamingView } from "./streaming";

import { useAITaskTask } from "~/ai/hooks";
import { hasStoredNoteContent } from "~/session/components/shared";
import { useEnhancedNote } from "~/session/queries";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";

export const Enhanced = forwardRef<
  NoteEditorRef,
  {
    sessionId: string;
    sessionTitle: string;
    enhancedNoteId: string;
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
      fileHandlerConfig,
      onNavigateToTitle,
      onViewReady,
      onViewDisposed,
      titleTrailerElement,
    },
    ref,
  ) => {
    const taskId = createTaskId(enhancedNoteId, "enhance");
    const { status, error, streamedText } = useAITaskTask(taskId, "enhance");
    // The task can finish before the coalesced index event refreshes the old summary cache.
    const generationId = useMemo(
      () => (status === "success" ? crypto.randomUUID() : undefined),
      [status, enhancedNoteId],
    );
    const enhancedNote = useEnhancedNote(enhancedNoteId, generationId);
    const content = enhancedNote?.content;

    const hasContent = hasStoredNoteContent(content);
    const isAwaitingPersistedContent =
      status === "success" && streamedText.trim().length > 0 && !hasContent;
    const showStreaming = status === "generating" || isAwaitingPersistedContent;

    if (status === "error") {
      return (
        <EnhanceError
          sessionId={sessionId}
          enhancedNoteId={enhancedNoteId}
          error={error}
        />
      );
    }

    if (!enhancedNote) {
      return showStreaming ? (
        <StreamingView
          sessionId={sessionId}
          sessionTitle={sessionTitle}
          enhancedNoteId={enhancedNoteId}
        />
      ) : null;
    }

    if (showStreaming) {
      return (
        <StreamingView
          sessionId={sessionId}
          sessionTitle={sessionTitle}
          enhancedNoteId={enhancedNoteId}
        />
      );
    }

    if (!hasContent) {
      return (
        <EmptySummary
          titleTrailerElement={titleTrailerElement}
          sessionId={sessionId}
          sessionTitle={sessionTitle}
          enhancedNoteId={enhancedNoteId}
        />
      );
    }

    return (
      <EnhancedEditor
        ref={ref}
        sessionId={sessionId}
        sessionTitle={sessionTitle}
        enhancedNoteId={enhancedNoteId}
        fileHandlerConfig={fileHandlerConfig}
        content={enhancedNote.content}
        onNavigateToTitle={onNavigateToTitle}
        onViewReady={onViewReady}
        onViewDisposed={onViewDisposed}
        titleTrailerElement={titleTrailerElement}
      />
    );
  },
);
