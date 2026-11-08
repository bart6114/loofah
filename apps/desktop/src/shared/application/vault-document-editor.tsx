import { Trans } from "@lingui/react/macro";
import { QueryClientProvider, useQuery } from "@tanstack/react-query";
import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";

import { DeviceRecovery } from "./device-recovery";
import { documentDraftId } from "./document-draft";
import {
  documentEditorBaseline,
  documentEditorDraftId,
  documentEditorDrafts,
  documentEditorRevision,
  type DocumentEditorBaseline,
} from "./document-editor-draft";
import { NativeDraftStore } from "./draft-store";
import type { DocumentTarget } from "./protocol.gen";
import {
  VaultEditorFields,
  type VaultNoteEditorHost,
} from "./vault-note-editor";
import type { VaultScope } from "./vault-scope";

import { enhancedNoteDraftKey } from "~/session/pending-note-drafts";
import type { NoteDraftRecovery } from "~/types/tauri.gen";

const terminal = new Set([
  "closed",
  "access_denied",
  "login_required",
  "incompatible",
]);
const noop = () => {};

export function VaultDocumentEditor(
  props: {
    scope: VaultScope;
    sessionId: string;
    target: DocumentTarget;
    readOnly?: boolean;
    onActiveDraft?: (id: string) => void;
    onSelectSession?: (id: string) => void;
  } & VaultNoteEditorHost,
) {
  const [owner, setOwner] = useState(props.scope);
  useEffect(() => setOwner(props.scope), [props.scope]);
  return owner === props.scope ? (
    <QueryClientProvider client={props.scope.queryClient}>
      <BoundDocumentEditor
        key={documentEditorDraftId(props.sessionId, props.target)}
        {...props}
      />
    </QueryClientProvider>
  ) : null;
}
function BoundDocumentEditor({
  scope,
  sessionId,
  target: input,
  readOnly = false,
  onActiveDraft = noop,
  onSelectSession = noop,
  ...host
}: Parameters<typeof VaultDocumentEditor>[0]) {
  const [target] = useState(() => Object.freeze(structuredClone(input)));
  const [lifetime] = useState(() => ({
    retired: terminal.has(scope.getSnapshot().state),
  }));
  const subscribe = useCallback(
    (changed: () => void) =>
      scope.subscribe(() => {
        if (terminal.has(scope.getSnapshot().state)) lifetime.retired = true;
        changed();
      }),
    [scope, lifetime],
  );
  const connection = useSyncExternalStore(subscribe, scope.getSnapshot);
  if (terminal.has(connection.state)) lifetime.retired = true;
  const readable =
    !lifetime.retired &&
    (connection.state === "connected" || connection.state === "reconnecting");
  const supported = scope.client.features.includes("document_editor_read");
  const options = useMemo(
    () => scope.documentEditorOptions(sessionId, target),
    [scope, sessionId, target],
  );
  const query = useQuery(
    { ...options, enabled: Boolean(sessionId) && supported && readable },
    scope.queryClient,
  );
  const store = useMemo(
    () => new NativeDraftStore(scope.client.scope),
    [scope.client.scope],
  );
  const id = documentEditorDraftId(sessionId, target);
  const recovery = useQuery(
    {
      queryKey: ["device-draft", scope.client.scope, id],
      queryFn: () => store.load(id),
      networkMode: "always",
    },
    scope.queryClient,
  );
  const legacy = useQuery(
    {
      queryKey: [
        "device-draft",
        scope.client.scope,
        documentDraftId(sessionId, target),
      ],
      queryFn: () => store.load(documentDraftId(sessionId, target)),
      networkMode: "always",
    },
    scope.queryClient,
  );
  const baseline = useMemo(() => {
    try {
      if (query.data && !query.error)
        return { value: documentEditorBaseline(sessionId, target, query.data) };
      const existing = documentEditorDrafts.get(scope.client.scope, id);
      if (existing) return { value: existing.getSnapshot().baseline };
      const saved = recovery.data;
      if (saved?.documentEditor && saved.editorTasks) {
        return {
          value: {
            note: saved.baseline,
            metadata: saved.documentEditor.metadata,
            tasks: saved.editorTasks.baseline,
          },
        };
      }
      return {};
    } catch (error) {
      return {
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  }, [query.data, query.error, scope, sessionId, target, id, recovery.data]);
  if (lifetime.retired) return <DeviceRecovery scope={scope.client.scope} />;
  if (recovery.isPending || legacy.isPending)
    return (
      <p>
        <Trans>Loading content…</Trans>
      </p>
    );
  if (recovery.error || legacy.error || baseline.error)
    return (
      <div role="alert">
        <p>{(recovery.error ?? legacy.error ?? baseline.error)?.message}</p>
        <DeviceRecovery scope={scope.client.scope} />
      </div>
    );
  if (legacy.data) return <DeviceRecovery scope={scope.client.scope} />;
  if (!supported)
    return (
      <div role="alert">
        <p>
          This server does not support the document editor. Upgrade the server,
          then reconnect.
        </p>
        <DeviceRecovery scope={scope.client.scope} />
      </div>
    );
  if (!baseline.value)
    return (
      <p role={query.error ? "alert" : "status"}>
        {query.error?.message ?? <Trans>Loading content…</Trans>}
      </p>
    );
  return (
    <DocumentFields
      {...host}
      scope={scope}
      sessionId={sessionId}
      target={target}
      baseline={baseline.value}
      snapshotSource={query.data}
      queryKey={options.queryKey}
      available={
        !!query.data &&
        !query.error &&
        !readOnly &&
        scope.client.features.includes("document_editor_write")
      }
      noteError={query.error}
      recovery={recovery.data ?? null}
      recoveryError={recovery.error}
      store={store}
      onActiveDraft={onActiveDraft}
      onSelectSession={onSelectSession}
    />
  );
}
function DocumentFields({
  scope,
  sessionId,
  target,
  baseline,
  snapshotSource,
  ...props
}: VaultNoteEditorHost & {
  scope: VaultScope;
  sessionId: string;
  target: DocumentTarget;
  baseline: DocumentEditorBaseline;
  snapshotSource: unknown;
  queryKey: readonly unknown[];
  available: boolean;
  noteError: Error | null;
  recovery: NoteDraftRecovery | null;
  recoveryError: Error | null;
  store: NativeDraftStore;
  onActiveDraft: (id: string) => void;
  onSelectSession: (id: string) => void;
}) {
  const [loaded] = useState(() => {
    try {
      const existing = documentEditorDrafts.get(
        scope.client.scope,
        documentEditorDraftId(sessionId, target),
      );
      const value =
        existing ??
        (props.recovery
          ? documentEditorDrafts.restore(
              scope.client,
              sessionId,
              target,
              props.recovery,
            )
          : documentEditorDrafts.open(
              scope.client,
              sessionId,
              target,
              baseline,
            ));
      value.rebind(scope.client);
      value.receive(baseline);
      return { draft: value };
    } catch (error) {
      return {
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  });
  const draft = loaded.draft;
  const retain = useCallback(
    () =>
      draft
        ? documentEditorDrafts.retainDraft(
            scope.client.scope,
            documentEditorDraftId(sessionId, target),
            draft,
          )
        : () => {},
    [scope, sessionId, target, draft],
  );
  const settled = useCallback(() => {
    documentEditorDrafts.sweepDraft(
      scope.client.scope,
      documentEditorDraftId(sessionId, target),
    );
  }, [scope, sessionId, target]);
  const confirm = useCallback(
    async (_saved: DocumentEditorBaseline) => {
      await scope.confirmDocumentEditor(sessionId, target);
    },
    [scope, sessionId, target],
  );
  if (!draft)
    return (
      <div role="alert">
        <p>{loaded.error?.message}</p>
        <DeviceRecovery scope={scope.client.scope} />
      </div>
    );
  return (
    <VaultEditorFields
      {...props}
      scope={scope}
      sessionId={sessionId}
      note={baseline}
      snapshotSource={snapshotSource}
      draft={draft}
      source={draft.source}
      recoveryId={documentEditorDraftId(sessionId, target)}
      viewKey={enhancedNoteDraftKey(
        scope.client.scope,
        sessionId,
        documentEditorDraftId(sessionId, target),
      )}
      retain={retain}
      onSettled={settled}
      confirmSnapshot={confirm}
      revision={documentEditorRevision}
      allowFileUploads={
        props.available &&
        scope.getSnapshot().state === "connected" &&
        scope.client.canEdit
      }
    />
  );
}
