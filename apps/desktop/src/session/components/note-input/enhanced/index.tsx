import type { EditorView } from "prosemirror-view";
import { forwardRef } from "react";

import type { FileHandlerConfig, NoteEditorRef } from "@hypr/editor/note";

import { EnhancedEditor } from "./editor";

import { hasStoredNoteContent } from "~/session/components/shared";
import { useSummarySource } from "~/session/hooks/useSummarySource";
import { useEnhancedNote, useEnhancedDocumentTarget } from "~/session/queries";
import { VaultProcessing } from "~/shared/application/vault-processing";
import { useVaultHost } from "~/shared/application/vault-provider";

export const Enhanced = forwardRef<
  NoteEditorRef,
  {
    sessionId: string;
    sessionTitle: string;
    readOnly?: boolean;
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
      readOnly,
      enhancedNoteId,
      fileHandlerConfig,
      onNavigateToTitle,
      onViewReady,
      onViewDisposed,
      titleTrailerElement,
    },
    ref,
  ) => {
    const { scope } = useVaultHost();
    const hasSource = useSummarySource(sessionId);
    const enhancedNote = useEnhancedNote(enhancedNoteId, undefined, sessionId);
    const target = useEnhancedDocumentTarget(sessionId, enhancedNoteId);
    const content = enhancedNote?.content;
    if (target === null)
      return (
        <p role="alert">
          This document cannot be identified safely. Editing and regeneration
          are unavailable.
        </p>
      );

    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {scope && target && (
          <div className="shrink-0 px-3 pt-3">
            <VaultProcessing
              scope={scope}
              sessionId={sessionId}
              target={target}
              defaultTitlePolicy="generate_if_empty"
              sourceAvailable={hasSource}
            />
            {enhancedNote && !hasStoredNoteContent(content) && (
              <p className="text-muted-foreground mt-2 text-sm">
                Generate a summary from your saved notes or transcript, or write
                here.
              </p>
            )}
          </div>
        )}
        <EnhancedEditor
          ref={ref}
          readOnly={readOnly}
          sessionId={sessionId}
          sessionTitle={sessionTitle}
          enhancedNoteId={enhancedNoteId}
          fileHandlerConfig={fileHandlerConfig}
          content={enhancedNote?.content ?? ""}
          onNavigateToTitle={onNavigateToTitle}
          onViewReady={onViewReady}
          onViewDisposed={onViewDisposed}
          titleTrailerElement={titleTrailerElement}
        />
      </div>
    );
  },
);
