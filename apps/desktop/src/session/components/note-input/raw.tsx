import { Trans } from "@lingui/react/macro";
import { QueryClientProvider } from "@tanstack/react-query";
import type { EditorView } from "prosemirror-view";
import {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useMemo,
  useRef,
} from "react";

import {
  type FileHandlerConfig,
  type NoteEditorRef,
  useMentionNavigation,
} from "@hypr/editor/note";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn } from "@hypr/utils";

import { AppLinkView } from "~/editor-bridge/app-link-view";
import { useMentionConfig } from "~/editor-bridge/mention-config";
import { openEditorLink } from "~/editor-bridge/open-editor-link";
import { sessionMentionDropConfig } from "~/editor-bridge/session-mention-drop";
import { SessionNodeView } from "~/editor-bridge/session-view";
import { useAttachmentResolver } from "~/session/hooks/useAttachmentResolver";
import { DeviceRecovery } from "~/shared/application/device-recovery";
import {
  VaultNoteEditor,
  type VaultNoteEditorHost,
} from "~/shared/application/vault-note-editor";
import { useVaultHost } from "~/shared/application/vault-provider";

const extraNodeViews = { appLink: AppLinkView, session: SessionNodeView };
const noop = () => {};

type RawEditorProps = {
  sessionId: string;
  rawMd: string;
  sessionTitle: string;
  className?: string;
  onNavigateToTitle?: (pixelWidth?: number) => void;
  showFormatToolbar?: boolean;
  fileHandlerConfig?: FileHandlerConfig;
  onViewReady?: (view: EditorView) => void;
  onViewDisposed?: (view: EditorView) => void;
  titleTrailerElement?: HTMLElement;
};

export const RawEditor = forwardRef<NoteEditorRef, RawEditorProps>(
  (props, ref) => <BoundRawEditor key={props.sessionId} {...props} ref={ref} />,
);

const BoundRawEditor = forwardRef<NoteEditorRef, RawEditorProps>(
  (
    {
      sessionId,
      className,
      onNavigateToTitle,
      showFormatToolbar = true,
      fileHandlerConfig,
      onViewReady,
      onViewDisposed,
      titleTrailerElement,
    },
    ref,
  ) => {
    const { scope, error, retry } = useVaultHost();
    const inner = useRef<NoteEditorRef>(null);
    const pending = useRef<Array<(editor: NoteEditorRef) => void>>([]);
    const invoke = useCallback((command: (editor: NoteEditorRef) => void) => {
      if (inner.current?.view) command(inner.current);
      else pending.current.push(command);
    }, []);
    useImperativeHandle(
      ref,
      () => ({
        get view() {
          return inner.current?.view ?? null;
        },
        commands: {
          focus: () => invoke((editor) => editor.commands.focus()),
          focusAtStart: () =>
            invoke((editor) => editor.commands.focusAtStart()),
          focusAtTrailingEmptyLine: () =>
            invoke((editor) => editor.commands.focusAtTrailingEmptyLine()),
          focusAtPixelWidth: (width) =>
            invoke((editor) => editor.commands.focusAtPixelWidth(width)),
          insertAtStartAndFocus: (content) =>
            invoke((editor) => editor.commands.insertAtStartAndFocus(content)),
          replaceContent: (content) =>
            invoke((editor) => editor.commands.replaceContent(content)),
          setSearch: (query, caseSensitive) =>
            invoke((editor) => editor.commands.setSearch(query, caseSensitive)),
          replace: (params) =>
            invoke((editor) => editor.commands.replace(params)),
        },
        flushPendingChanges: () => inner.current?.flushPendingChanges(),
      }),
      [invoke],
    );
    const ready = useCallback(
      (view: EditorView) => {
        onViewReady?.(view);
        queueMicrotask(() => {
          const editor = inner.current;
          if (editor?.view === view)
            pending.current.splice(0).forEach((command) => command(editor));
        });
      },
      [onViewReady],
    );
    const resolveAttachment = useAttachmentResolver(sessionId);
    const mentionConfig = useMentionConfig();
    const navigateMention = useMentionNavigation();
    const options = useMemo<VaultNoteEditorHost["editorOptions"]>(
      () => ({
        className: cn(["session-note-editor", className]),
        resolveAttachment,
        mentionConfig,
        onMentionNavigate: navigateMention,
        sessionMentionDropConfig,
        onNavigateToTitle,
        onLinkOpen: openEditorLink,
        fileHandlerConfig,
        extraNodeViews,
        showFormatToolbar,
        onViewReady: ready,
        onViewDisposed,
        titleTrailerElement,
        syncContentWhenFocused: false,
      }),
      [
        className,
        resolveAttachment,
        mentionConfig,
        navigateMention,
        onNavigateToTitle,
        fileHandlerConfig,
        showFormatToolbar,
        ready,
        onViewDisposed,
        titleTrailerElement,
      ],
    );
    const failed = useCallback(
      (cause: Error) => {
        sonnerToast.error(`Note is NOT being saved: ${cause.message}`, {
          id: `note-save-failed:${scope?.client.scope}:${sessionId}`,
        });
      },
      [scope, sessionId],
    );
    if (!scope)
      return (
        <div role={error ? "alert" : "status"}>
          {error ? (
            <>
              <p>{error.message}</p>
              <button onClick={retry}>
                <Trans>Retry</Trans>
              </button>
            </>
          ) : (
            <Trans>Loading content…</Trans>
          )}
        </div>
      );
    return (
      <QueryClientProvider client={scope.queryClient}>
        <VaultNoteEditor
          key={`${scope.client.scope}:${sessionId}`}
          scope={scope}
          sessionId={sessionId}
          onActiveDraft={noop}
          onSelectSession={noop}
          hasLegacyDraft={false}
          legacyEditor={<DeviceRecovery scope={scope.client.scope} />}
          editorRef={inner}
          editorOptions={options}
          showSaveControls={false}
          layout="document"
          onSaveError={failed}
        />
      </QueryClientProvider>
    );
  },
);
