import {
  useInfiniteQuery,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { useState } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { useSyncStatus } from "./sync";
import {
  componentLabels,
  deviceLabel,
  SyncConfirmation,
  SyncPreviewText,
} from "./sync-display";

import { useTabs } from "~/store/zustand/tabs";
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
  beforeRestore,
  afterRestore,
}: {
  entity: SyncEntity;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  beforeRestore?: () => Promise<void>;
  afterRestore?: () => Promise<void>;
}) {
  const client = useQueryClient();
  const status = useSyncStatus();
  const openNew = useTabs((state) => state.openNew);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const ready =
    status.data &&
    (["paused", "connected"].includes(status.data.phase) ||
      (status.data.phase === "pairing" &&
        status.data.pairingRole === "approver"));
  const openSettings = () => {
    onOpenChange(false);
    openNew({ type: "settings", state: { tab: "sync" } });
  };
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
    enabled: open && Boolean(ready),
  });
  const preview = useMutation({
    mutationFn: (revision: string) =>
      perform({ preview: { entity, revision } }),
  });
  const action = useMutation({
    mutationFn: async (value: SyncAction) => {
      if (typeof value === "object" && "restore" in value) {
        await beforeRestore?.();
      }
      return perform(value);
    },
    onSuccess: async (result, value) => {
      if (result?.cancelled) return;
      if (typeof value === "object" && "purge_version" in value) {
        preview.reset();
        setConfirmDelete(false);
      }
      await client.invalidateQueries({ queryKey: ["sync-history", entity] });
      if (typeof value === "object" && "restore" in value) {
        await afterRestore?.();
      }
    },
  });
  const selected = preview.data?.preview;
  const successful = action.isSuccess && !action.data?.cancelled;
  const successText =
    successful && typeof action.variables === "object"
      ? "export" in action.variables
        ? "Version exported to the folder you selected."
        : "restore" in action.variables
          ? "Version restored. Your previous changes remain in history."
          : "purge_version" in action.variables
            ? "Version deleted from history."
            : null
      : null;
  const versions =
    history.data?.pages.flatMap((page) => page?.versions ?? []) ?? [];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Version history</DialogTitle>
        </DialogHeader>
        <DialogDescription>
          Browse versions saved to the cloud, export a copy, or restore an
          earlier version.
        </DialogDescription>
        {status.isPending && <p>Checking sync setup…</p>}
        {status.error && (
          <p role="alert">
            Could not check sync setup. Open Sync settings to retry.
          </p>
        )}
        {status.data && !ready && (
          <div className="flex flex-col gap-3">
            <p>
              {[
                "disconnected",
                "authorizing",
                "authorization_required",
              ].includes(status.data.phase)
                ? "Sign in and finish setting up Sync on this Mac to view cloud versions."
                : "Finish securing this Mac in Sync settings to unlock your cloud versions."}
            </p>
            <Button onClick={openSettings}>Open Sync settings</Button>
          </div>
        )}
        {status.error && (
          <Button onClick={openSettings}>Open Sync settings</Button>
        )}
        {ready && history.isPending && <p>Loading versions…</p>}
        {ready && history.isSuccess && versions.length === 0 && (
          <div className="flex flex-col gap-3">
            <p>
              No cloud versions yet. A version appears after this session syncs
              for the first time.
            </p>
            <Button variant="outline" onClick={openSettings}>
              Open Sync settings
            </Button>
          </div>
        )}
        {successText && <p role="status">{successText}</p>}
        {action.data?.cancelled && <p role="status">Export cancelled.</p>}
        {(history.error || preview.error || action.error) && (
          <p role="alert" className="text-destructive">
            {history.error?.message ??
              preview.error?.message ??
              action.error?.message}
          </p>
        )}
        <div className="flex flex-col gap-2">
          {ready &&
            versions.map((version) => (
              <Button
                key={version.id}
                variant="outline"
                onClick={() => {
                  action.reset();
                  preview.mutate(version.id);
                }}
                disabled={preview.isPending || action.isPending}
              >
                Synced {new Date(version.created_at).toLocaleString()} ·{" "}
                {deviceLabel(version.device_id, status.data)}
                {version.current === 1 ? " · Current" : ""}
                {version.pinned === 1 ? " · Conflict" : ""}
              </Button>
            ))}
        </div>
        {ready && history.hasNextPage && (
          <Button
            variant="outline"
            disabled={history.isFetchingNextPage}
            onClick={() => void history.fetchNextPage()}
          >
            Load older versions
          </Button>
        )}
        {ready && selected && (
          <section className="flex flex-col gap-3 rounded border p-3">
            <h3 className="font-semibold">
              {selected.title || "Selected version"}
            </h3>
            <p>
              Synced {new Date(selected.created_at).toLocaleString()} ·{" "}
              {deviceLabel(selected.device_id, status.data)}
            </p>
            {selected.captured_at && (
              <p>Captured {new Date(selected.captured_at).toLocaleString()}</p>
            )}
            {selected.missing_references.length > 0 && (
              <p role="status">
                People or tags that are no longer in this vault:{" "}
                {selected.missing_references.join("; ")}. Restoring this session
                does not restore these people or tags.
              </p>
            )}
            <p className="text-sm">
              {selected.deleted
                ? "Deleted session"
                : `Includes: ${componentLabels(selected.files)}`}
            </p>
            <p className="text-sm">
              Changes compared with this Mac’s last synchronized version:{" "}
              {componentLabels(selected.changed) || "None"}
            </p>
            <SyncPreviewText
              kind={entity.kind}
              text={selected.text}
              deleted={selected.deleted}
            />
            <p className="text-muted-foreground text-sm">
              {entity.kind === "session"
                ? "Restoring replaces the whole session, including its recording, transcript and attachments, and saves a new cloud version. Your current edits are saved in history first. People, tags and global tasks stay unchanged."
                : `Restoring replaces all ${entity.kind} in this vault. Your current version is saved in history first.`}
            </p>
            <div className="flex flex-wrap gap-2">
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
                onClick={() => setConfirmDelete(true)}
              >
                Delete this version from history…
              </Button>
            )}
          </section>
        )}
        {ready && confirmDelete && selected && (
          <SyncConfirmation
            title="Delete this version from history?"
            confirmLabel="Delete version"
            pending={action.isPending}
            onClose={() => setConfirmDelete(false)}
            onConfirm={() =>
              action.mutate({ purge_version: { revision: preview.variables! } })
            }
          >
            <p>
              The version synced{" "}
              {new Date(selected.created_at).toLocaleString()} will no longer be
              available to preview or restore. This frees storage from your
              allowance. Your current version and unresolved conflicts stay
              protected.
            </p>
            {action.error && <p role="alert">{action.error.message}</p>}
          </SyncConfirmation>
        )}
      </DialogContent>
    </Dialog>
  );
}
