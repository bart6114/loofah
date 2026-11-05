import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import {
  QueryClientProvider,
  useInfiniteQuery,
  useMutation,
  useQuery,
} from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { Button } from "@hypr/ui/components/ui/button";

import { NativeDraftStore } from "./draft-store";
import { ApplicationRequestError } from "./errors";
import { NoteDrafts, type NoteDraft } from "./note-draft";
import type { VaultClient } from "./vault-client";
import { VaultScope } from "./vault-scope";

import type { NoteDraftRecovery, NoteSnapshot } from "~/types/tauri.gen";

const drafts = new NoteDrafts();

export function VaultNotesWorkspace({
  client,
  onCatalogChanged,
}: {
  client: VaultClient;
  onCatalogChanged?: () => void;
}) {
  const [scope, setScope] = useState<VaultScope | null>(null);
  useEffect(() => {
    const current = new VaultScope(client);
    drafts.retainScope(client.scope);
    setScope(current);
    const changed = current.onChange((change) => {
      if (change.kind === "catalog_changed") onCatalogChanged?.();
    });
    void current.start().catch(() => {});
    return () => {
      changed();
      current.dispose();
      drafts.releaseScope(client.scope);
    };
  }, [client, onCatalogChanged]);
  if (!scope || scope.client !== client)
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
      return result;
    },
    onSuccess: (session) => setSessionId(session.meta.id),
  });
  const copy = useMutation({
    mutationFn: async () => {
      if (sessionId) {
        const draft = drafts.get(scope.client.scope, sessionId);
        if (draft) await writeText(draft.recovery().draft.markdown);
      }
    },
  });
  useEffect(() => {
    if (denied) drafts.clearSaved(scope.client.scope);
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
          </aside>
          {sessionId ? (
            <NotePane
              key={`${scope.client.scope}:${sessionId}`}
              scope={scope}
              sessionId={sessionId}
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

function NotePane({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const note = useQuery(scope.noteOptions(sessionId));
  const store = useMemo(
    () => new NativeDraftStore(scope.client.scope),
    [scope.client.scope],
  );
  const recovery = useQuery({
    queryKey: ["device-draft", scope.client.scope, sessionId],
    queryFn: () => store.load(sessionId),
    networkMode: "always",
  });
  const snapshot =
    note.data ??
    drafts.get(scope.client.scope, sessionId)?.getSnapshot().baseline ??
    recovery.data?.baseline;
  if (!snapshot || recovery.isPending)
    return (
      <div className="p-4 text-sm">
        {note.error ? (
          <p role="alert">{note.error.message}</p>
        ) : (
          <Trans>Loading note…</Trans>
        )}
      </div>
    );
  return (
    <NoteFields
      scope={scope}
      sessionId={sessionId}
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
  note,
  noteError,
  available,
  recovery,
  recoveryError,
  store,
}: {
  scope: VaultScope;
  sessionId: string;
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
        ? drafts.restore(scope.client, recovery)
        : drafts.open(scope.client, sessionId, note));
    value.rebind(scope.client);
    value.receive(note);
    return value;
  });
  const snapshot = useSyncExternalStore(draft.subscribe, draft.getSnapshot);
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const persist = useMutation({
    mutationFn: async () => {
      const value = draft.recovery();
      if (draft.getSnapshot().dirty || value.uncertain) await store.save(value);
      else await store.remove(sessionId);
    },
  });
  const lastSubmitted = useRef<{ markdown: string; title: string } | null>(
    null,
  );
  const save = useMutation({
    mutationFn: async () => {
      await persist.mutateAsync().catch(() => {});
      if (scope.getSnapshot().state !== "connected")
        throw new ApplicationRequestError({
          code: "unavailable",
          message:
            "Reconnect before saving. Your draft is kept on this device.",
          request_id: null,
          retryable: true,
          current_revision: null,
          mutation_outcome: "not_committed",
        });
      try {
        lastSubmitted.current = draft.getSnapshot().draft;
        const saved = await scope.run(() => draft.save());
        await scope.confirmNote(sessionId, saved);
        return saved;
      } finally {
        await persist.mutateAsync().catch(() => {});
        if (
          ["access_denied", "login_required", "closed"].includes(
            scope.getSnapshot().state,
          )
        )
          drafts.clearSaved(scope.client.scope);
        else drafts.sweep(scope.client.scope);
      }
    },
  });
  const editable =
    available &&
    connection.state === "connected" &&
    scope.client.canEdit &&
    (!snapshot.conflict || save.isPending);
  const form = useForm({
    defaultValues: snapshot.draft,
    onSubmit: async () => {
      await save.mutateAsync();
    },
  });
  const persistRef = useRef(persist.mutateAsync);
  persistRef.current = persist.mutateAsync;
  const saveRef = useRef(save.mutate);
  saveRef.current = save.mutate;
  useEffect(() => {
    if (
      scope.queryClient.getQueryData(scope.noteOptions(sessionId).queryKey) ===
      note
    )
      draft.receive(note);
  }, [draft, note, scope, sessionId]);
  useEffect(() => {
    if (!snapshot.dirty) form.reset(snapshot.draft);
  }, [form, snapshot.baseline.revision, snapshot.dirty, snapshot.draft]);
  useEffect(() => {
    if (!snapshot.dirty) return;
    const timer = setTimeout(() => {
      void persistRef.current().catch(() => {});
    }, 200);
    return () => clearTimeout(timer);
  }, [snapshot]);
  useEffect(() => {
    if (
      !snapshot.dirty ||
      !editable ||
      save.isPending ||
      (save.error && lastSubmitted.current === snapshot.draft)
    )
      return;
    const timer = setTimeout(() => saveRef.current(), 500);
    return () => clearTimeout(timer);
  }, [snapshot, editable, save.isPending, save.error]);
  useEffect(() => {
    if (connection.state === "connected") lastSubmitted.current = null;
  }, [connection.state]);
  useEffect(
    () => () => {
      void persistRef.current().catch(() => {});
    },
    [draft],
  );
  const resolve = useMutation({
    mutationFn: async ({
      keep,
      revision,
    }: {
      keep: boolean;
      revision: string;
    }) => {
      if (keep) draft.keepDraft(revision);
      else draft.useServerVersion(revision);
      const current = draft.getSnapshot();
      form.reset(current.draft);
      if (keep) await save.mutateAsync();
      else {
        await scope.confirmNote(sessionId, current.baseline);
        await persist.mutateAsync();
      }
    },
  });
  return (
    <form
      className="flex min-h-0 min-w-0 flex-1 flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        event.stopPropagation();
        void form.handleSubmit().catch(() => {});
      }}
    >
      {noteError && (
        <p role="alert" className="text-destructive text-xs">
          {noteError.message}
        </p>
      )}
      <form.Field name="title">
        {(field) => (
          <label className="flex flex-col gap-1 text-xs">
            <Trans>Title</Trans>
            <input
              className="bg-background rounded border px-3 py-2 text-sm"
              readOnly={!editable}
              value={field.state.value}
              onChange={(event) => {
                field.handleChange(event.target.value);
                draft.edit(form.getFieldValue("markdown"), event.target.value);
              }}
            />
          </label>
        )}
      </form.Field>
      <form.Field name="markdown">
        {(field) => (
          <label className="flex min-h-0 flex-1 flex-col gap-1 text-xs">
            <Trans>Markdown</Trans>
            <textarea
              className="bg-background min-h-48 flex-1 resize-none rounded border p-3 font-mono text-sm"
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
              This note changed in another client. Review the server version
              before choosing.
            </Trans>
          </p>
          <pre className="my-2 max-h-40 overflow-auto whitespace-pre-wrap">
            {snapshot.conflict.title}
            {"\n\n"}
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
              <Trans>Use server version</Trans>
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
      <div className="flex items-center justify-between">
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
