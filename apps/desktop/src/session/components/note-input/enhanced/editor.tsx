import type { EditorView } from "prosemirror-view";
import { forwardRef, memo, useCallback, useMemo } from "react";

import {
  type FileHandlerConfig,
  NoteEditor,
  type JSONContent,
  type NoteEditorRef,
  useMentionNavigation,
} from "@hypr/editor/note";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { AppLinkView } from "~/editor-bridge/app-link-view";
import { useMentionConfig } from "~/editor-bridge/mention-config";
import { openEditorLink } from "~/editor-bridge/open-editor-link";
import { sessionMentionDropConfig } from "~/editor-bridge/session-mention-drop";
import { SessionNodeView } from "~/editor-bridge/session-view";
import { useEnhancedDocumentTarget } from "~/session/queries";
import {
  ensureFirstLineTitle,
  documentTitlePlaceholder,
} from "~/session/title-content";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { VaultAttachmentProvider } from "~/shared/application/vault-attachment-provider";
import { VaultDocumentEditor } from "~/shared/application/vault-document-editor";
import type { VaultNoteEditorHost } from "~/shared/application/vault-note-editor";
import { useVaultHost } from "~/shared/application/vault-provider";

const extraNodeViews = { appLink: AppLinkView, session: SessionNodeView };
type EnhancedEditorProps = {
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
};
const EnhancedEditorInner = forwardRef<NoteEditorRef, EnhancedEditorProps>(
  (props, ref) =>
    props.contentOverride !== undefined ? (
      <PreviewEditor
        {...props}
        editorRef={ref}
        contentOverride={props.contentOverride}
      />
    ) : (
      <BoundEnhancedEditor {...props} editorRef={ref} />
    ),
);
function BoundEnhancedEditor({
  sessionId,
  enhancedNoteId,
  readOnly,
  fileHandlerConfig,
  onNavigateToTitle,
  onViewReady,
  onViewDisposed,
  titleTrailerElement,
  editorRef,
}: EnhancedEditorProps & Pick<VaultNoteEditorHost, "editorRef">) {
  const { scope, error } = useVaultHost();
  const target = useEnhancedDocumentTarget(sessionId, enhancedNoteId);
  const mentionConfig = useMentionConfig();
  const navigateMention = useMentionNavigation();
  const options = useMemo<VaultNoteEditorHost["editorOptions"]>(
    () => ({
      className: "session-note-editor enhanced-summary-editor",
      mentionConfig,
      onMentionNavigate: navigateMention,
      sessionMentionDropConfig,
      onNavigateToTitle,
      onLinkOpen: openEditorLink,
      fileHandlerConfig,
      extraNodeViews,
      onViewReady,
      onViewDisposed,
      titleTrailerElement,
      syncContentWhenFocused: false,
    }),
    [
      mentionConfig,
      navigateMention,
      onNavigateToTitle,
      fileHandlerConfig,
      onViewReady,
      onViewDisposed,
      titleTrailerElement,
    ],
  );
  const failed = useCallback(
    (cause: Error) =>
      sonnerToast.error(`Summary is NOT being saved: ${cause.message}`, {
        id: `summary-save-failed:${scope?.client.scope}:${sessionId}:${enhancedNoteId}`,
      }),
    [scope, sessionId, enhancedNoteId],
  );
  if (!scope || target === undefined)
    return (
      <p role={error ? "alert" : "status"}>
        {error?.message ?? "Loading content…"}
      </p>
    );
  if (!target)
    return (
      <p role="alert">
        This document cannot be identified safely. Editing and regeneration are
        unavailable.
      </p>
    );
  return (
    <VaultDocumentEditor
      scope={scope}
      sessionId={sessionId}
      target={target}
      readOnly={readOnly}
      editorRef={editorRef}
      editorOptions={options}
      showSaveControls
      layout="document"
      onSaveError={failed}
    />
  );
}
function PreviewEditor({
  sessionId,
  sessionTitle,
  contentOverride,
  editorRef,
  onNavigateToTitle,
  onViewReady,
  onViewDisposed,
  titleTrailerElement,
}: EnhancedEditorProps &
  Pick<VaultNoteEditorHost, "editorRef"> & { contentOverride: JSONContent }) {
  const owner = useVaultReadScope();
  const mentionConfig = useMentionConfig();
  const content = useMemo(
    () => ensureFirstLineTitle(contentOverride, sessionTitle),
    [contentOverride, sessionTitle],
  );
  if (!owner.current()) return null;
  return (
    <VaultAttachmentProvider scope={owner.scope} sessionId={sessionId}>
      <NoteEditor
        ref={editorRef}
        readOnly
        className="session-note-editor enhanced-summary-editor"
        initialContent={content}
        attachmentPresentation={owner.scope.client.attachmentPresentation}
        placeholderComponent={documentTitlePlaceholder}
        mentionConfig={mentionConfig}
        onNavigateToTitle={onNavigateToTitle}
        onLinkOpen={openEditorLink}
        extraNodeViews={extraNodeViews}
        onViewReady={onViewReady}
        onViewDisposed={onViewDisposed}
        syncContentWhenFocused
        titleTrailerElement={titleTrailerElement}
      />
    </VaultAttachmentProvider>
  );
}
export const EnhancedEditor = memo(EnhancedEditorInner);
