import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  NoteEditor,
  type JSONContent,
  type MentionNavigation,
} from "@hypr/editor/note";
import { Button } from "@hypr/ui/components/ui/button";

import { NativeDraftStore } from "./draft-store";
import {
  editorRevision,
  editorText,
  noteEditorDraftId,
  noteEditorDrafts,
  type NoteEditorDraft,
} from "./note-editor-draft";
import type { NoteEditorSnapshot } from "./protocol.gen";
import { useDraftPersistence } from "./use-draft-persistence";
import type { VaultScope } from "./vault-scope";

import {
  captureNoteEditorContent,
  noteEditorContent,
} from "~/editor-bridge/note-editor-content";
import { openEditorLink } from "~/editor-bridge/open-editor-link";
import { documentTitlePlaceholder } from "~/session/title-content";
import type { NoteDraftRecovery } from "~/types/tauri.gen";

export function VaultNoteEditor({
  scope,
  sessionId,
  onActiveDraft,
  onSelectSession,
  legacyEditor,
  hasLegacyDraft,
}: {
  scope: VaultScope;
  sessionId: string;
  onActiveDraft: (id: string) => void;
  onSelectSession: (id: string) => void;
  legacyEditor: ReactNode;
  hasLegacyDraft: boolean;
}) {
  const id = noteEditorDraftId(sessionId);
  const store = useMemo(
    () => new NativeDraftStore(scope.client.scope),
    [scope.client.scope],
  );
  const options = useMemo(
    () => scope.noteEditorOptions(sessionId),
    [scope, sessionId],
  );
  const note = useQuery(options);
  const recovery = useQuery({
    queryKey: ["device-draft", scope.client.scope, id],
    queryFn: () => store.load(id),
    networkMode: "always",
  });
  const legacy = useQuery({
    queryKey: ["device-draft", scope.client.scope, sessionId],
    queryFn: () => store.load(sessionId),
    networkMode: "always",
  });
  const snapshot =
    note.data ??
    noteEditorDrafts.get(scope.client.scope, id)?.getSnapshot().baseline ??
    (recovery.data?.editorTasks
      ? {
          note: recovery.data.baseline,
          tasks: recovery.data.editorTasks.baseline,
        }
      : undefined);
  if (recovery.isPending || legacy.isPending)
    return (
      <p>
        <Trans>Loading content…</Trans>
      </p>
    );
  if (legacy.error) return <p role="alert">{legacy.error.message}</p>;
  return (
    <EditorMode
      legacy={hasLegacyDraft || !!legacy.data}
      legacyEditor={legacyEditor}
    >
      {snapshot ? (
        <EditorDraft
          scope={scope}
          sessionId={sessionId}
          note={snapshot}
          queryKey={options.queryKey}
          available={!!note.data && !note.error && !recovery.error}
          noteError={note.error}
          recovery={recovery.data ?? null}
          recoveryError={recovery.error}
          store={store}
          onActiveDraft={onActiveDraft}
          onSelectSession={onSelectSession}
        />
      ) : (
        <p role={note.error || recovery.error ? "alert" : undefined}>
          {(note.error ?? recovery.error)?.message ?? (
            <Trans>Loading content…</Trans>
          )}
        </p>
      )}
    </EditorMode>
  );
}

function EditorMode({
  legacy,
  legacyEditor,
  children,
}: {
  legacy: boolean;
  legacyEditor: ReactNode;
  children: ReactNode;
}) {
  // Finish an existing source-editor recovery before changing its save contract.
  const [keepLegacy] = useState(legacy);
  return keepLegacy ? legacyEditor : children;
}

function EditorDraft(props: {
  scope: VaultScope;
  sessionId: string;
  note: NoteEditorSnapshot;
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
      const draft =
        noteEditorDrafts.get(
          props.scope.client.scope,
          noteEditorDraftId(props.sessionId),
        ) ??
        (props.recovery
          ? noteEditorDrafts.restore(
              props.scope.client,
              props.sessionId,
              props.recovery,
            )
          : noteEditorDrafts.open(
              props.scope.client,
              props.sessionId,
              props.note,
            ));
      draft.rebind(props.scope.client);
      draft.receive(props.note);
      return { draft };
    } catch (error) {
      return {
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  });
  if (!loaded.draft)
    return (
      <div role="alert">
        <p>{loaded.error?.message}</p>
        <pre className="whitespace-pre-wrap">
          {props.recovery?.draft.markdown ?? props.note.note.markdown}
        </pre>
      </div>
    );
  return <EditorFields {...props} draft={loaded.draft} />;
}

function EditorFields({
  scope,
  sessionId,
  note,
  queryKey,
  available,
  noteError,
  recovery,
  recoveryError,
  store,
  draft,
  onActiveDraft,
  onSelectSession,
}: {
  scope: VaultScope;
  sessionId: string;
  note: NoteEditorSnapshot;
  queryKey: readonly unknown[];
  available: boolean;
  noteError: Error | null;
  recovery: NoteDraftRecovery | null;
  recoveryError: Error | null;
  store: NativeDraftStore;
  draft: NoteEditorDraft;
  onActiveDraft: (id: string) => void;
  onSelectSession: (id: string) => void;
}) {
  const confirm = useCallback(
    (saved: NoteEditorSnapshot) => scope.confirmNoteEditor(sessionId, saved),
    [scope, sessionId],
  );
  const { snapshot, connection, editable, persist, save, resolve } =
    useDraftPersistence({
      scope,
      draft,
      queryKey,
      confirm,
      available,
      recovery,
      store,
      recoveryId: noteEditorDraftId(sessionId),
      autosave: false,
      onSettled: () => {
        if (
          ["access_denied", "login_required", "closed"].includes(
            scope.getSnapshot().state,
          )
        )
          noteEditorDrafts.clearSaved(scope.client.scope);
        else noteEditorDrafts.sweep(scope.client.scope);
      },
    });
  const model = useMemo(() => {
    try {
      return { value: noteEditorContent(sessionId, snapshot.draft) };
    } catch (error) {
      return {
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  }, [sessionId, snapshot.draft]);
  const [editorEpoch, setEditorEpoch] = useState(0);
  const generation = useRef(0);
  const active = useRef(false);
  const editing = useRef(false);
  editing.current = editable && !resolve.isPending && !!model.value;
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  useEffect(() => {
    onActiveDraft(noteEditorDraftId(sessionId));
  }, [onActiveDraft, sessionId]);
  useEffect(() => {
    if (scope.queryClient.getQueryData(queryKey) === note) draft.receive(note);
  }, [draft, note, scope, queryKey]);
  const wasReady = useRef(false);
  const ready = editable && !resolve.isPending;
  const { mutate: saveNow, mutateAsync: saveAsync, isPending: saving } = save;
  useEffect(() => {
    const resumed = ready && !wasReady.current;
    wasReady.current = ready;
    const current = draft.getSnapshot();
    if (resumed && current.dirty && !current.conflict && !saving) saveNow();
  }, [ready, draft, saving, saveNow]);
  const open = useMutation({ mutationFn: openEditorLink });
  const { mutate: openNow } = open;
  const openHref = useCallback(
    (href: string) => {
      openNow(href);
    },
    [openNow],
  );
  const navigateMention = useCallback<MentionNavigation>(
    ({ id, type }) => {
      if (type === "session" && id) onSelectSession(id);
    },
    [onSelectSession],
  );
  const capture = useCallback(
    (getContent: () => JSONContent) => {
      if (
        !active.current ||
        !editing.current ||
        generation.current !== editorEpoch
      )
        return;
      draft.capture(() =>
        captureNoteEditorContent(sessionId, getContent(), model.value!.tasks),
      );
    },
    [draft, sessionId, model.value, editorEpoch],
  );
  const change = useCallback(
    async (content: JSONContent) => {
      if (
        !active.current ||
        !editing.current ||
        generation.current !== editorEpoch
      )
        throw new Error("Editing is paused");
      draft.edit(
        captureNoteEditorContent(sessionId, content, model.value!.tasks),
      );
      await saveAsync();
      return () => {
        const current = draft.getSnapshot();
        if (current.dirty) throw new Error("Newer edits are still pending");
        return noteEditorContent(sessionId, editorText(current.baseline))
          .content;
      };
    },
    [draft, sessionId, model.value, saveAsync, editorEpoch],
  );
  const chooseServer = () => {
    if (!snapshot.conflict) return;
    editing.current = false;
    generation.current += 1;
    const next = generation.current;
    resolve.mutate(
      { keep: false, revision: editorRevision(snapshot.conflict) },
      { onSettled: () => setEditorEpoch(next) },
    );
  };
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="min-h-0 flex-1 overflow-y-auto">
        {noteError && <p role="alert">{noteError.message}</p>}
        {model.value ? (
          <NoteEditor
            key={editorEpoch}
            initialContent={model.value.content}
            className="min-h-32 px-3 py-2"
            handleChange={change}
            onDraftChange={capture}
            attachmentPresentation={scope.client.attachmentPresentation}
            onMentionNavigate={navigateMention}
            onLinkOpen={openHref}
            placeholderComponent={documentTitlePlaceholder}
            readOnly={!editing.current}
            syncContentWhenFocused
          />
        ) : (
          <div role="alert">
            <p>{model.error?.message}</p>
            <pre className="whitespace-pre-wrap">{snapshot.draft.markdown}</pre>
          </div>
        )}
        {snapshot.conflict && !save.isPending && (
          <div className="rounded border p-3 text-xs">
            <p>
              <Trans>
                This content changed in another client. Review the server
                version before choosing.
              </Trans>
            </p>
            <pre className="my-2 max-h-40 overflow-auto whitespace-pre-wrap">
              {snapshot.conflict.note.title}
              {"\n\n"}
              {snapshot.conflict.note.markdown}
              {"\n\n"}
              {snapshot.conflict.tasks.tasks
                .map(
                  (task) =>
                    `${task.status.replace(/_/g, " ")}: ${task.text}${task.due_at ? ` (${task.due_at})` : ""}`,
                )
                .join("\n")}
            </pre>
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={resolve.isPending}
                onClick={chooseServer}
              >
                <Trans>Use server version</Trans>
              </Button>
              <Button
                size="sm"
                disabled={
                  !available ||
                  connection.state !== "connected" ||
                  !scope.client.canEdit ||
                  resolve.isPending
                }
                onClick={() =>
                  resolve.mutate({
                    keep: true,
                    revision: editorRevision(snapshot.conflict!),
                  })
                }
              >
                <Trans>Save my version</Trans>
              </Button>
            </div>
          </div>
        )}
        {[save.error, resolve.error, open.error]
          .filter(Boolean)
          .map((error, i) => (
            <p key={i} role="alert">
              {error!.message}
            </p>
          ))}
        {(persist.error || recoveryError) && (
          <p role="alert">
            <Trans>Device recovery copy:</Trans>{" "}
            {(persist.error ?? recoveryError)?.message}
          </p>
        )}
      </div>
      <div className="flex items-center justify-between">
        <p role="status" className="text-muted-foreground text-xs">
          {snapshot.dirty ? (
            <Trans>Unsaved changes</Trans>
          ) : (
            <Trans>Saved</Trans>
          )}
        </p>
        <Button
          size="sm"
          disabled={!editing.current || !snapshot.dirty || save.isPending}
          onClick={() => save.mutate()}
        >
          <Trans>Save</Trans>
        </Button>
      </div>
    </div>
  );
}
