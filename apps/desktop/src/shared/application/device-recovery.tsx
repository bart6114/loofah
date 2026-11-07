import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useMemo, useState } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@hypr/ui/components/ui/dialog";

import { NativeDraftStore } from "./draft-store";

import type { NoteDraftRecovery, RecoveryEntry } from "~/types/tauri.gen";

export function DeviceRecovery({ scope }: { scope?: string }) {
  return <RecoveryDialog key={JSON.stringify(scope ?? null)} scope={scope} />;
}

function RecoveryDialog({ scope }: { scope?: string }) {
  const store = useMemo(
    () => (scope === undefined ? null : new NativeDraftStore(scope)),
    [scope],
  );
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState<{
    scope: string;
    id: string;
  } | null>(null);
  const inventory = useQuery({
    queryKey: ["device_recovery", scope ?? null, "inventory"],
    queryFn: async ({ signal }) => {
      const result = store
        ? [{ scope: store.scope, entries: await store.inventory() }]
        : await NativeDraftStore.allInventory();
      signal.throwIfAborted();
      return result;
    },
    enabled: expanded,
    networkMode: "always",
    staleTime: 0,
    retry: false,
  });
  const recovery = useQuery({
    queryKey: ["device_recovery", selected?.scope, "record", selected?.id],
    queryFn: async ({ signal }) => {
      const result = await new NativeDraftStore(selected!.scope).recovery(
        selected!.id,
      );
      signal.throwIfAborted();
      return result;
    },
    enabled: expanded && selected !== null,
    networkMode: "always",
    staleTime: 0,
    retry: false,
  });
  const copy = useMutation({
    networkMode: "always",
    mutationFn: (value: NoteDraftRecovery) => writeText(value.draft.markdown),
  });
  const copyAll = useMutation({
    networkMode: "always",
    mutationFn: (value: NoteDraftRecovery) =>
      writeText(JSON.stringify(value, null, 2)),
  });
  return (
    <Dialog open={expanded} onOpenChange={setExpanded}>
      <DialogTrigger asChild>
        <Button variant="outline" size="sm">
          <Trans>Device recovery drafts</Trans>
        </Button>
      </DialogTrigger>
      <DialogContent className="flex max-h-[85vh] w-[94vw] max-w-2xl flex-col overflow-hidden">
        <DialogHeader>
          <DialogTitle>
            <Trans>Device recovery drafts</Trans>
          </DialogTitle>
          <DialogDescription>
            {scope === undefined ? (
              <Trans>
                Recovery copies saved on this device, including signed-out
                connections and vaults you can no longer access. Copy text to
                recover it; this view does not save to any vault.
              </Trans>
            ) : (
              <Trans>
                Recovery copies for this vault from closed windows and deleted
                notes or outputs. Copy text to recover it; this view does not
                save to the vault.
              </Trans>
            )}
          </DialogDescription>
        </DialogHeader>
        <Button
          variant="ghost"
          size="sm"
          disabled={inventory.isFetching}
          onClick={() => {
            void inventory.refetch();
            if (selected) void recovery.refetch();
          }}
        >
          <Trans>Reload drafts</Trans>
        </Button>
        <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto text-xs">
          {inventory.isPending && (
            <p>
              <Trans>Loading drafts…</Trans>
            </p>
          )}
          {inventory.error && <p role="alert">{inventory.error.message}</p>}
          {inventory.data?.every((group) => group.entries.length === 0) && (
            <p>
              <Trans>No saved recovery drafts.</Trans>
            </p>
          )}
          <div className="flex max-h-32 shrink-0 flex-col gap-1 overflow-y-auto">
            {inventory.data?.flatMap((group) =>
              group.entries.map((entry) => (
                <button
                  key={JSON.stringify([group.scope, entry.id])}
                  className="hover:bg-muted aria-pressed:bg-muted flex flex-col rounded border px-2 py-1 text-left"
                  aria-pressed={
                    selected?.scope === group.scope && selected?.id === entry.id
                  }
                  onClick={() => {
                    setSelected({ scope: group.scope, id: entry.id });
                    copy.reset();
                    copyAll.reset();
                  }}
                >
                  <RecoveryLabel entry={entry} />
                  {scope === undefined && (
                    <RecoveryLocation scope={group.scope} />
                  )}
                  <span className="text-muted-foreground break-all">
                    {recoveryResource(entry.sessionId)}
                  </span>
                  {entry.uncertain && (
                    <span>
                      <Trans>Save confirmation was not received</Trans>
                    </span>
                  )}
                </button>
              )),
            )}
          </div>
          {selected && recovery.isPending && (
            <p>
              <Trans>Loading draft…</Trans>
            </p>
          )}
          {recovery.error && <p role="alert">{recovery.error.message}</p>}
          {selected && recovery.isSuccess && !recovery.data && (
            <p>
              <Trans>This recovery copy has already been cleared.</Trans>
            </p>
          )}
          {recovery.data && (
            <>
              <label className="flex flex-col gap-1">
                <Trans>Recovered title</Trans>
                <input
                  className="bg-background rounded border px-2 py-1"
                  readOnly
                  value={recovery.data.draft.title}
                />
              </label>
              <label className="flex flex-col gap-1">
                <Trans>Recovered Markdown</Trans>
                <textarea
                  className="bg-background min-h-28 shrink-0 rounded border p-2 font-mono"
                  readOnly
                  value={recovery.data.draft.markdown}
                />
              </label>
              {recovery.data.editorTasks && (
                <p>
                  <Trans>Copy all recovery data to keep task changes.</Trans>
                </p>
              )}
            </>
          )}
          {copy.error && <p role="alert">{copy.error.message}</p>}
          {copyAll.error && <p role="alert">{copyAll.error.message}</p>}
        </div>
        {recovery.data && (
          <Button
            variant="outline"
            size="sm"
            disabled={copy.isPending}
            onClick={() => copy.mutate(recovery.data!)}
          >
            <Trans>Copy saved text</Trans>
          </Button>
        )}
        {recovery.data?.editorTasks && (
          <Button
            variant="outline"
            size="sm"
            disabled={copyAll.isPending}
            onClick={() => copyAll.mutate(recovery.data!)}
          >
            <Trans>Copy all recovery data</Trans>
          </Button>
        )}
      </DialogContent>
    </Dialog>
  );
}

function RecoveryLabel({ entry }: { entry: RecoveryEntry }) {
  if (entry.title) return <span>{entry.title}</span>;
  try {
    const resource: unknown = JSON.parse(entry.sessionId);
    if (Array.isArray(resource)) {
      if (resource[0] === "summary")
        return (
          <span>
            <Trans>Summary draft</Trans>
          </span>
        );
      if (resource[0] === "template_output")
        return (
          <span>
            <Trans>Template output draft</Trans>
          </span>
        );
    }
  } catch {}
  return (
    <span>
      <Trans>Untitled note draft</Trans>
    </span>
  );
}

function recoveryResource(id: string): string {
  try {
    const resource: unknown = JSON.parse(id);
    if (
      Array.isArray(resource) &&
      ["summary", "template_output", "note_editor"].includes(resource[0]) &&
      typeof resource[1] === "string"
    )
      return resource[1];
  } catch {}
  return id;
}

function RecoveryLocation({ scope }: { scope: string }) {
  try {
    const source: unknown = JSON.parse(scope);
    if (
      Array.isArray(source) &&
      source.length === 4 &&
      source[0] === "remote" &&
      source.slice(1).every((value) => typeof value === "string")
    ) {
      return (
        <span className="text-muted-foreground break-all">
          {source[1]} · {source[2]} · {source[3]}
        </span>
      );
    }
  } catch {}
  return (
    <span className="text-muted-foreground break-all">
      <Trans>Device vault</Trans> · {scope}
    </span>
  );
}
