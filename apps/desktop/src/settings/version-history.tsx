import {
  useInfiniteQuery,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { commands, type SyncAction, type SyncEntity } from "~/types/tauri.gen";

async function perform(action: SyncAction) {
  const result = await commands.syncAction(action);
  if (result.status === "error") throw new Error(result.error);
  return result.data;
}

export function VersionHistory({
  entity,
  open,
  onOpenChange,
}: {
  entity: SyncEntity;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const client = useQueryClient();
  const history = useInfiniteQuery({
    queryKey: ["sync-history", entity],
    initialPageParam: null as [number, string] | null,
    queryFn: ({ pageParam }) =>
      perform({ history: { entity, before: pageParam } }),
    getNextPageParam: (page) => {
      const last = page?.versions[page.versions.length - 1];
      return page?.versions.length === 50 && last
        ? ([last.created_at, last.id] as [number, string])
        : undefined;
    },
    enabled: open,
  });
  const preview = useMutation({
    mutationFn: (revision: string) =>
      perform({ preview: { entity, revision } }),
  });
  const action = useMutation({
    mutationFn: perform,
    onSuccess: () =>
      client.invalidateQueries({ queryKey: ["sync-history", entity] }),
  });
  const versions =
    history.data?.pages.flatMap((page) => page?.versions ?? []) ?? [];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Version history</DialogTitle>
        </DialogHeader>
        <p className="text-muted-foreground text-sm">
          {entity.kind === "session"
            ? "Restore replaces this whole session and creates a new cloud version. People, tags and global tasks stay unchanged."
            : `Restore replaces the complete ${entity.kind} registry and creates a new cloud version.`}{" "}
          Local edits are preserved before restoring. Referenced people or tags
          may be missing when restoring older sessions.
        </p>
        {history.isPending && <p>Loading versions…</p>}
        {(history.error || preview.error || action.error) && (
          <p role="alert" className="text-destructive">
            {history.error?.message ??
              preview.error?.message ??
              action.error?.message}
          </p>
        )}
        <div className="flex flex-col gap-2">
          {versions.map((version) => (
            <Button
              key={version.id}
              variant="outline"
              onClick={() => preview.mutate(version.id)}
              disabled={preview.isPending}
            >
              Synced {new Date(version.created_at).toLocaleString()} · Mac{" "}
              {version.device_id.slice(0, 8)}
              {version.current === 1 ? " · Current" : ""}
              {version.pinned === 1 ? " · Conflict" : ""}
            </Button>
          ))}
        </div>
        {history.hasNextPage && (
          <Button
            variant="outline"
            disabled={history.isFetchingNextPage}
            onClick={() => void history.fetchNextPage()}
          >
            Load older versions
          </Button>
        )}
        {preview.data?.preview && (
          <section className="flex flex-col gap-3">
            {preview.data.preview.captured_at && (
              <p>
                Captured{" "}
                {new Date(preview.data.preview.captured_at).toLocaleString()}
              </p>
            )}
            {preview.data.preview.missing_references.length > 0 && (
              <p role="status">
                Missing from the current global records:{" "}
                {preview.data.preview.missing_references.join("; ")}. These
                records remain unchanged by a session restore.
              </p>
            )}
            <p className="text-sm">
              {preview.data.preview.deleted
                ? "Deleted session"
                : `Components: ${preview.data.preview.files.join(", ")}`}
            </p>
            <p className="text-sm">
              Changes compared with this Mac’s last synchronized version:{" "}
              {preview.data.preview.changed.join(", ") || "None"}
            </p>
            <pre className="max-h-64 overflow-auto rounded border p-3 text-sm whitespace-pre-wrap">
              {preview.data.preview.text || "No note text in this version."}
            </pre>
            <div className="flex gap-2">
              <Button
                variant="outline"
                disabled={action.isPending}
                onClick={() =>
                  action.mutate({
                    export: {
                      entity,
                      revision: preview.variables!,
                    },
                  })
                }
              >
                Export this version…
              </Button>
              <Button
                disabled={action.isPending}
                onClick={() =>
                  action.mutate({
                    restore: {
                      entity,
                      revision: preview.variables!,
                    },
                  })
                }
              >
                {entity.kind === "session"
                  ? "Restore whole session"
                  : `Restore ${entity.kind}`}
              </Button>
            </div>
            {versions.some(
              (version) =>
                version.id === preview.variables &&
                version.current !== 1 &&
                version.pinned !== 1,
            ) && (
              <Button
                variant="outline"
                disabled={action.isPending}
                onClick={() =>
                  action.mutate({
                    purge_version: { revision: preview.variables! },
                  })
                }
              >
                Purge this historical version
              </Button>
            )}
            <p className="text-muted-foreground text-sm">
              Purging removes this version from history and releases logical
              storage. Current and unresolved conflict versions are protected.
              Recovery bytes remain retained during staging.
            </p>
            {action.isSuccess && <p role="status">Completed.</p>}
          </section>
        )}
      </DialogContent>
    </Dialog>
  );
}
