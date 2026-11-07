import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import {
  QueryClientProvider,
  useInfiniteQuery,
  useMutation,
  useQuery,
} from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { Button } from "@hypr/ui/components/ui/button";

import { DeviceRecovery } from "./device-recovery";
import { documentDraftClient, documentDraftId } from "./document-draft";
import { NativeDraftStore } from "./draft-store";
import {
  noteDrafts as drafts,
  type NoteDraft,
  type NoteDraftClient,
} from "./note-draft";
import { noteEditorDrafts } from "./note-editor-draft";
import type { DocumentTarget } from "./protocol.gen";
import { useDraftPersistence } from "./use-draft-persistence";
import { VaultAttachmentsPane } from "./vault-attachments-pane";
import type { VaultClient } from "./vault-client";
import { VaultNoteEditor } from "./vault-note-editor";
import { VaultProcessing } from "./vault-processing";
import { VaultClientProvider, useVaultHost } from "./vault-provider";
import type { VaultScope } from "./vault-scope";

import { RecordingPlayer } from "~/audio-player/recording";
import type { NoteDraftRecovery, NoteSnapshot } from "~/types/tauri.gen";

export function VaultNotesWorkspace({
  client,
  onCatalogChanged,
}: {
  client: VaultClient;
  onCatalogChanged?: () => void;
}) {
  return (
    <VaultClientProvider
      client={client}
      onChange={(change) => {
        if (change.kind === "catalog_changed") onCatalogChanged?.();
      }}
    >
      <WorkspaceHost />
    </VaultClientProvider>
  );
}

function WorkspaceHost() {
  const { scope } = useVaultHost();
  if (!scope)
    return (
      <p>
        <Trans>Connecting…</Trans>
      </p>
    );
  return (
    <QueryClientProvider client={scope.queryClient}>
      <Notes scope={scope} />
    </QueryClientProvider>
  );
}

function Notes({ scope }: { scope: VaultScope }) {
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const activeDraft = useRef<string | null>(null);
  const onActiveDraft = useCallback((id: string) => {
    activeDraft.current = id;
  }, []);
  const [search, setSearch] = useState("");
  const searchForm = useForm({
    defaultValues: { query: "" },
    onSubmit: ({ value }) => setSearch(value.query.trim()),
  });
  const connected = connection.state === "connected";
  const denied = [
    "access_denied",
    "login_required",
    "incompatible",
    "closed",
  ].includes(connection.state);
  const sessions = useInfiniteQuery({
    queryKey: ["vault", scope.client.scope, "session_page", "infinite"],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      scope.run(() => scope.client.sessions(100, pageParam, { signal })),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    enabled: !denied,
  });
  const create = useMutation({
    mutationFn: async (requestId: string) => {
      const result = await scope.run(() =>
        scope.client.createSession({ title: "", markdown: "" }, { requestId }),
      );
      await scope.confirmNote(result.meta.id, result.note);
      void scope.queryClient.invalidateQueries({
        queryKey: scope.noteOptions(result.meta.id).queryKey,
        exact: true,
      });
      return result;
    },
    onSuccess: (session) => setSessionId(session.meta.id),
  });
  const copy = useMutation({
    networkMode: "always",
    mutationFn: async () => {
      if (sessionId) {
        const id = activeDraft.current ?? sessionId;
        const draft =
          drafts.get(scope.client.scope, id) ??
          noteEditorDrafts.get(scope.client.scope, id);
        if (draft) await writeText(draft.recovery().draft.markdown);
      }
    },
  });
  useEffect(() => {
    if (denied) {
      drafts.clearSaved(scope.client.scope);
      noteEditorDrafts.clearSaved(scope.client.scope);
    }
  }, [denied, scope]);
  const rows = sessions.data?.pages.flatMap((page) => page.sessions) ?? [];
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
        <p className="text-muted-foreground text-xs" role="status">
          {connected ? (
            <Trans>Connected</Trans>
          ) : (
            <Trans>Disconnected. Unsaved text is kept on this device.</Trans>
          )}
        </p>
        <Button
          size="sm"
          disabled={!connected || !scope.client.canEdit || create.isPending}
          onClick={() => create.mutate(crypto.randomUUID())}
        >
          <Trans>New note</Trans>
        </Button>
      </div>
      {connection.error && (
        <p role="alert" className="text-destructive text-xs">
          {connection.error.message}
        </p>
      )}
      {create.error && (
        <div role="alert" className="text-destructive text-xs">
          {create.error.message}
          <Button
            size="sm"
            variant="ghost"
            disabled={!connected || create.isPending}
            onClick={() => create.variables && create.mutate(create.variables)}
          >
            <Trans>Retry creation</Trans>
          </Button>
        </div>
      )}
      {!denied && (
        <DeviceRecovery key={scope.client.scope} scope={scope.client.scope} />
      )}
      {denied ? (
        <div className="flex items-center gap-3">
          <p>
            <Trans>This connection is no longer authorized.</Trans>
          </p>
          <Button variant="outline" onClick={() => copy.mutate()}>
            <Trans>Copy unsaved text</Trans>
          </Button>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 gap-4">
          <aside className="flex w-48 shrink-0 flex-col gap-1 overflow-y-auto border-r pr-3">
            {scope.client.features.includes("search") && (
              <form
                className="mb-2 flex flex-col gap-2"
                onSubmit={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  void searchForm.handleSubmit();
                }}
              >
                <searchForm.Field name="query">
                  {(field) => (
                    <label className="flex flex-col gap-1 text-xs">
                      <Trans>Search notes</Trans>
                      <input
                        className="bg-background rounded border px-2 py-1"
                        maxLength={4096}
                        value={field.state.value}
                        onChange={(event) =>
                          field.handleChange(event.target.value)
                        }
                      />
                    </label>
                  )}
                </searchForm.Field>
                <div className="flex gap-2">
                  <Button
                    type="submit"
                    size="sm"
                    variant="outline"
                    disabled={!connected}
                  >
                    <Trans>Search</Trans>
                  </Button>
                  {search && (
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={() => {
                        searchForm.reset();
                        setSearch("");
                      }}
                    >
                      <Trans>Clear</Trans>
                    </Button>
                  )}
                </div>
              </form>
            )}
            {search ? (
              <SearchResults
                scope={scope}
                query={search}
                selected={sessionId}
                onSelect={setSessionId}
              />
            ) : (
              <>
                {sessions.error && (
                  <p role="alert" className="text-destructive text-xs">
                    {sessions.error.message}
                  </p>
                )}
                {rows.map((session) => (
                  <button
                    key={session.id}
                    className="hover:bg-muted aria-pressed:bg-muted rounded px-2 py-1 text-left text-sm"
                    aria-pressed={session.id === sessionId}
                    onClick={() => setSessionId(session.id)}
                  >
                    {session.title || <Trans>Untitled</Trans>}
                  </button>
                ))}
                {sessions.hasNextPage && (
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={sessions.isFetchingNextPage}
                    onClick={() => void sessions.fetchNextPage()}
                  >
                    <Trans>Load more</Trans>
                  </Button>
                )}
              </>
            )}
          </aside>
          {sessionId ? (
            <SessionPane
              key={`${scope.client.scope}:${sessionId}`}
              scope={scope}
              sessionId={sessionId}
              onActiveDraft={onActiveDraft}
              onSelectSession={setSessionId}
            />
          ) : (
            <p className="text-muted-foreground p-4 text-sm">
              <Trans>Select a note or create one.</Trans>
            </p>
          )}
        </div>
      )}
      {copy.error && (
        <p role="alert" className="text-destructive text-xs">
          {copy.error.message}
        </p>
      )}
    </div>
  );
}

function SessionPane({
  scope,
  sessionId,
  onActiveDraft,
  onSelectSession,
}: {
  scope: VaultScope;
  sessionId: string;
  onActiveDraft: (id: string) => void;
  onSelectSession: (id: string) => void;
}) {
  const { t } = useLingui();
  const [view, setView] = useState<
    "note" | "documents" | "transcripts" | "tasks" | "attachments"
  >("note");
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      {scope.client.features.includes("audio_playback") && (
        <RecordingPlayer scope={scope} sessionId={sessionId} />
      )}
      {scope.client.features.includes("content_read") && (
        <div className="flex gap-2" role="group" aria-label={t`Note views`}>
          <Button
            aria-pressed={view === "note"}
            size="sm"
            variant="ghost"
            onClick={() => setView("note")}
          >
            <Trans>Note</Trans>
          </Button>
          <Button
            aria-pressed={view === "documents"}
            size="sm"
            variant="ghost"
            onClick={() => setView("documents")}
          >
            <Trans>Outputs</Trans>
          </Button>
          <Button
            aria-pressed={view === "transcripts"}
            size="sm"
            variant="ghost"
            onClick={() => setView("transcripts")}
          >
            <Trans>Transcript</Trans>
          </Button>
          <Button
            aria-pressed={view === "tasks"}
            size="sm"
            variant="ghost"
            onClick={() => setView("tasks")}
          >
            <Trans>Tasks</Trans>
          </Button>
          {scope.client.features.includes("attachment_list") && (
            <Button
              aria-pressed={view === "attachments"}
              size="sm"
              variant="ghost"
              onClick={() => setView("attachments")}
            >
              <Trans>Attachments</Trans>
            </Button>
          )}
        </div>
      )}
      {view === "note" ? (
        scope.client.features.includes("note_editor_read") &&
        scope.client.features.includes("note_editor_write") ? (
          <VaultNoteEditor
            scope={scope}
            sessionId={sessionId}
            onActiveDraft={onActiveDraft}
            onSelectSession={onSelectSession}
            hasLegacyDraft={
              !!drafts.get(scope.client.scope, sessionId)?.getSnapshot().dirty
            }
            legacyEditor={
              <NotePane
                scope={scope}
                sessionId={sessionId}
                onActiveDraft={onActiveDraft}
              />
            }
          />
        ) : (
          <NotePane
            scope={scope}
            sessionId={sessionId}
            onActiveDraft={onActiveDraft}
          />
        )
      ) : view === "documents" ? (
        <div className="flex min-h-0 flex-1 flex-col gap-3">
          <DocumentsPane
            scope={scope}
            sessionId={sessionId}
            onActiveDraft={onActiveDraft}
          />
        </div>
      ) : view === "attachments" ? (
        <VaultAttachmentsPane
          key={sessionId}
          scope={scope}
          sessionId={sessionId}
        />
      ) : (
        <ContentPane scope={scope} sessionId={sessionId} view={view} />
      )}
    </div>
  );
}

function DocumentsPane({
  scope,
  sessionId,
  onActiveDraft,
}: {
  scope: VaultScope;
  sessionId: string;
  onActiveDraft: (id: string) => void;
}) {
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const documents = useQuery({
    ...scope.documentsOptions(sessionId),
    enabled: connection.state === "connected",
  });
  const [target, setTarget] = useState<DocumentTarget | null>(null);
  const canEdit = scope.client.features.includes("document_edit");
  if (target) {
    const selected = documents.data?.find((doc) =>
      target.kind === "summary"
        ? doc.kind === "summary"
        : doc.kind === "template_output" && doc.id === target.document_id,
    );
    return (
      <div className="flex min-h-0 flex-1 flex-col gap-3">
        <div className="flex shrink-0 items-center gap-3">
          <Button size="sm" variant="ghost" onClick={() => setTarget(null)}>
            <Trans>Back</Trans>
          </Button>
          <h3 className="truncate text-sm font-medium">
            {selected?.title ||
              (target.kind === "summary" ? (
                <Trans>Summary</Trans>
              ) : (
                <Trans>Output</Trans>
              ))}
          </h3>
        </div>
        {(target.kind === "summary" ||
          selected?.kind === "template_output") && (
          <VaultProcessing
            scope={scope}
            sessionId={sessionId}
            target={target}
          />
        )}
        <NotePane
          key={documentDraftId(sessionId, target)}
          scope={scope}
          sessionId={sessionId}
          target={target}
          onActiveDraft={onActiveDraft}
        />
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <VaultProcessing scope={scope} sessionId={sessionId} />
      <div className="min-h-0 flex-1 overflow-y-auto text-sm" role="document">
        {documents.error && (
          <p role="alert" className="text-destructive">
            {documents.error.message}
          </p>
        )}
        {documents.isPending ? (
          <p className="text-muted-foreground">
            <Trans>Loading content…</Trans>
          </p>
        ) : (
          <>
            {canEdit &&
              !documents.error &&
              !documents.data?.some((doc) => doc.kind === "summary") && (
                <Button
                  size="sm"
                  className="mb-3"
                  disabled={
                    connection.state !== "connected" || !scope.client.canEdit
                  }
                  onClick={() => setTarget({ kind: "summary" })}
                >
                  <Trans>Add summary</Trans>
                </Button>
              )}
            {documents.data?.length === 0 && (
              <p className="text-muted-foreground">
                <Trans>No content yet.</Trans>
              </p>
            )}
            {documents.data?.map((doc) => (
              <section key={doc.id} className="mb-4">
                <div className="mb-2 flex items-center justify-between gap-3">
                  <h3 className="font-medium">{doc.title}</h3>
                  {canEdit && (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={connection.state !== "connected"}
                      onClick={() =>
                        setTarget(
                          doc.kind === "summary"
                            ? { kind: "summary" }
                            : { kind: "template_output", document_id: doc.id },
                        )
                      }
                    >
                      {scope.client.canEdit ? (
                        <Trans>Edit</Trans>
                      ) : (
                        <Trans>View</Trans>
                      )}
                    </Button>
                  )}
                </div>
                <pre className="whitespace-pre-wrap">{doc.markdown}</pre>
              </section>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function ContentPane({
  scope,
  sessionId,
  view,
}: {
  scope: VaultScope;
  sessionId: string;
  view: "transcripts" | "tasks";
}) {
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const enabled = connection.state === "connected";

  const transcripts = useQuery({
    ...scope.transcriptsOptions(sessionId),
    enabled: enabled && view === "transcripts",
  });
  const tasks = useQuery({
    ...scope.tasksOptions(sessionId),
    enabled: enabled && view === "tasks",
  });
  const result = view === "transcripts" ? transcripts : tasks;
  if (result.error)
    return (
      <p role="alert" className="text-destructive text-sm">
        {result.error.message}
      </p>
    );
  if (result.isPending)
    return (
      <p className="text-muted-foreground text-sm">
        <Trans>Loading content…</Trans>
      </p>
    );
  if (result.data?.length === 0)
    return (
      <p className="text-muted-foreground text-sm">
        <Trans>No content yet.</Trans>
      </p>
    );
  return (
    <div className="min-h-0 flex-1 overflow-y-auto text-sm" role="document">
      {view === "transcripts" &&
        transcripts.data?.map((transcript) => (
          <pre key={transcript.id} className="mb-4 whitespace-pre-wrap">
            {(transcript.words ?? []).map((word) => word.text).join(" ")}
          </pre>
        ))}
      {view === "tasks" &&
        tasks.data?.map((task) => (
          <p key={task.id} className="mb-3">
            <span className="text-muted-foreground">{task.status}</span>{" "}
            {task.text}
          </p>
        ))}
    </div>
  );
}

function SearchResults({
  scope,
  query,
  selected,
  onSelect,
}: {
  scope: VaultScope;
  query: string;
  selected: string | null;
  onSelect: (sessionId: string) => void;
}) {
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const result = useQuery({
    ...scope.searchOptions({
      query,
      limit: 50,
      options: {
        fuzzy: false,
        distance: null,
        snippets: true,
        snippet_max_chars: 160,
        phrase_slop: null,
      },
    }),
    enabled: connection.state === "connected",
  });
  return (
    <>
      {result.error && (
        <p role="alert" className="text-destructive text-xs">
          {result.error.message}
        </p>
      )}
      {result.isFetching && (
        <p className="text-muted-foreground text-xs">
          <Trans>Searching…</Trans>
        </p>
      )}
      {result.data?.count === 0 && (
        <p className="text-muted-foreground text-xs">
          <Trans>No matching notes</Trans>
        </p>
      )}
      {result.data?.hits.map((hit) => (
        <button
          key={hit.document.id}
          className="hover:bg-muted aria-pressed:bg-muted rounded px-2 py-1 text-left text-sm"
          aria-pressed={selected === hit.document.id}
          onClick={() => onSelect(hit.document.id)}
        >
          <span className="block">
            {hit.document.title || <Trans>Untitled</Trans>}
          </span>
          {hit.content_snippet && (
            <span className="text-muted-foreground line-clamp-3 text-xs">
              {hit.content_snippet.fragment}
            </span>
          )}
        </button>
      ))}
      {result.data && result.data.count > result.data.hits.length && (
        <p className="text-muted-foreground text-xs">
          <Trans>Refine your search to see more results.</Trans>
        </p>
      )}
    </>
  );
}

function NotePane({
  scope,
  sessionId,
  target,
  onActiveDraft,
}: {
  scope: VaultScope;
  sessionId: string;
  target?: DocumentTarget;
  onActiveDraft: (id: string) => void;
}) {
  const id = target ? documentDraftId(sessionId, target) : sessionId;
  const client = useMemo(
    () =>
      target
        ? documentDraftClient(scope.client, sessionId, target)
        : scope.client,
    [scope.client, sessionId, target],
  );
  const options = useMemo(
    () =>
      target
        ? scope.documentTextOptions(sessionId, target)
        : scope.noteOptions(sessionId),
    [scope, sessionId, target],
  );
  const note = useQuery(options);
  const confirm = useCallback(
    (saved: NoteSnapshot) =>
      target
        ? scope.confirmDocumentText(sessionId, target, saved)
        : scope.confirmNote(sessionId, saved),
    [scope, sessionId, target],
  );
  useEffect(() => {
    onActiveDraft(id);
  }, [id, onActiveDraft]);
  const store = useMemo(
    () => new NativeDraftStore(scope.client.scope),
    [scope.client.scope],
  );
  const recovery = useQuery({
    queryKey: ["device-draft", scope.client.scope, id],
    queryFn: () => store.load(id),
    networkMode: "always",
  });
  const snapshot =
    note.data ??
    drafts.get(scope.client.scope, id)?.getSnapshot().baseline ??
    recovery.data?.baseline;
  if (!snapshot || recovery.isPending)
    return (
      <div className="p-4 text-sm">
        {note.error ? (
          <p role="alert">{note.error.message}</p>
        ) : (
          <Trans>Loading content…</Trans>
        )}
      </div>
    );
  return (
    <NoteFields
      scope={scope}
      sessionId={id}
      client={client}
      queryKey={options.queryKey}
      confirm={confirm}
      readOnlyTitle={!!target}
      note={snapshot}
      noteError={note.error}
      available={!!note.data && !note.error}
      recovery={recovery.data ?? null}
      recoveryError={recovery.error}
      store={store}
    />
  );
}

function NoteFields({
  scope,
  sessionId,
  client,
  queryKey,
  confirm,
  readOnlyTitle,
  note,
  noteError,
  available,
  recovery,
  recoveryError,
  store,
}: {
  scope: VaultScope;
  sessionId: string;
  client: NoteDraftClient;
  queryKey: readonly unknown[];
  confirm: (note: NoteSnapshot) => Promise<void>;
  readOnlyTitle: boolean;
  note: NoteSnapshot;
  noteError: Error | null;
  available: boolean;
  recovery: NoteDraftRecovery | null;
  recoveryError: Error | null;
  store: NativeDraftStore;
}) {
  const [draft] = useState<NoteDraft>(() => {
    const existing = drafts.get(scope.client.scope, sessionId);
    const value =
      existing ??
      (recovery
        ? drafts.restore(client, recovery)
        : drafts.open(client, sessionId, note));
    value.rebind(client);
    value.receive(note);
    return value;
  });
  const { snapshot, connection, editable, persist, save, resolve } =
    useDraftPersistence({
      scope,
      draft,
      queryKey,
      confirm,
      available,
      recovery,
      store,
      recoveryId: sessionId,
      onSettled: () => {
        if (
          ["access_denied", "login_required", "closed"].includes(
            scope.getSnapshot().state,
          )
        )
          drafts.clearSaved(scope.client.scope);
        else drafts.sweep(scope.client.scope);
      },
    });
  const form = useForm({
    defaultValues: snapshot.draft,
    onSubmit: async () => {
      await save.mutateAsync();
    },
  });
  useEffect(() => {
    if (scope.queryClient.getQueryData(queryKey) === note) draft.receive(note);
  }, [draft, note, scope, queryKey]);
  useEffect(() => {
    if (!snapshot.dirty) form.reset(snapshot.draft);
  }, [form, snapshot.baseline.revision, snapshot.dirty, snapshot.draft]);
  return (
    <form
      className="flex min-h-0 min-w-0 flex-1 flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        event.stopPropagation();
        void form.handleSubmit().catch(() => {});
      }}
    >
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
        {noteError && (
          <p role="alert" className="text-destructive text-xs">
            {noteError.message}
          </p>
        )}
        {!readOnlyTitle && (
          <form.Field name="title">
            {(field) => (
              <label className="flex shrink-0 flex-col gap-1 text-xs">
                <Trans>Title</Trans>
                <input
                  className="bg-background rounded border px-3 py-2 text-sm"
                  readOnly={!editable}
                  value={field.state.value}
                  onChange={(event) => {
                    field.handleChange(event.target.value);
                    draft.edit(
                      form.getFieldValue("markdown"),
                      event.target.value,
                    );
                  }}
                />
              </label>
            )}
          </form.Field>
        )}
        <form.Field name="markdown">
          {(field) => (
            <label className="flex min-h-32 flex-1 flex-col gap-1 text-xs">
              <Trans>Markdown</Trans>
              <textarea
                className="bg-background min-h-0 flex-1 resize-none rounded border p-3 font-mono text-sm"
                readOnly={!editable}
                value={field.state.value}
                onChange={(event) => {
                  field.handleChange(event.target.value);
                  draft.edit(event.target.value, form.getFieldValue("title"));
                }}
              />
            </label>
          )}
        </form.Field>
        {snapshot.conflict && !save.isPending && (
          <div className="rounded border p-3 text-xs">
            <p>
              <Trans>
                This content changed elsewhere. Review the saved version before
                choosing.
              </Trans>
            </p>
            <pre className="my-2 max-h-40 overflow-auto whitespace-pre-wrap">
              {snapshot.conflict.title && `${snapshot.conflict.title}\n\n`}
              {snapshot.conflict.markdown}
            </pre>
            <div className="flex gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() =>
                  resolve.mutate({
                    keep: false,
                    revision: snapshot.conflict!.revision,
                  })
                }
              >
                <Trans>Use saved version</Trans>
              </Button>
              <Button
                type="button"
                size="sm"
                disabled={
                  !available ||
                  connection.state !== "connected" ||
                  !scope.client.canEdit
                }
                onClick={() =>
                  resolve.mutate({
                    keep: true,
                    revision: snapshot.conflict!.revision,
                  })
                }
              >
                <Trans>Save my version</Trans>
              </Button>
            </div>
          </div>
        )}
        {save.error && (
          <p role="alert" className="text-destructive text-xs">
            {save.error.message}
          </p>
        )}
        {resolve.error && (
          <p role="alert" className="text-destructive text-xs">
            {resolve.error.message}
          </p>
        )}
        {(persist.error || recoveryError) && (
          <p role="alert" className="text-destructive text-xs">
            <Trans>Device recovery copy:</Trans>{" "}
            {(persist.error ?? recoveryError)?.message}
          </p>
        )}
      </div>
      <div className="flex shrink-0 items-center justify-between">
        <p role="status" className="text-muted-foreground text-xs">
          {snapshot.dirty ? (
            <Trans>Unsaved changes</Trans>
          ) : (
            <Trans>Saved</Trans>
          )}
        </p>
        <Button
          type="submit"
          size="sm"
          disabled={!editable || !snapshot.dirty || save.isPending}
        >
          <Trans>Save</Trans>
        </Button>
      </div>
    </form>
  );
}
