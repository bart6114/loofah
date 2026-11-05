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

export function DeviceRecovery({ scope }: { scope: string }) {
  const store = useMemo(() => new NativeDraftStore(scope), [scope]);
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const inventory = useQuery({
    queryKey: ["device_recovery", scope, "inventory"],
    queryFn: async ({ signal }) => {
      const result = await store.inventory();
      signal.throwIfAborted();
      return result;
    },
    enabled: expanded,
    networkMode: "always",
    staleTime: 0,
    retry: false,
  });
  const recovery = useQuery({
    queryKey: ["device_recovery", scope, "record", selected],
    queryFn: async ({ signal }) => {
      const result = await store.recovery(selected!);
      signal.throwIfAborted();
      return result;
    },
    enabled: expanded && selected !== null,
    networkMode: "always",
    staleTime: 0,
    retry: false,
  });
  const copy = useMutation({
    mutationFn: (value: NoteDraftRecovery) => writeText(value.draft.markdown),
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
            <Trans>
              Recovery copies for this vault from closed windows and deleted
              notes or outputs. Copy text to recover it; this view does not save
              to the vault.
            </Trans>
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
          {inventory.data?.length === 0 && (
            <p>
              <Trans>No saved recovery drafts.</Trans>
            </p>
          )}
          <div className="flex max-h-32 shrink-0 flex-col gap-1 overflow-y-auto">
            {inventory.data?.map((entry) => (
              <button
                key={entry.id}
                className="hover:bg-muted aria-pressed:bg-muted flex flex-col rounded border px-2 py-1 text-left"
                aria-pressed={selected === entry.id}
                onClick={() => {
                  setSelected(entry.id);
                  copy.reset();
                }}
              >
                <RecoveryLabel entry={entry} />
                <span className="text-muted-foreground break-all">
                  {recoveryResource(entry.sessionId)}
                </span>
                {entry.uncertain && (
                  <span>
                    <Trans>Save confirmation was not received</Trans>
                  </span>
                )}
              </button>
            ))}
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
            </>
          )}
          {copy.error && <p role="alert">{copy.error.message}</p>}
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
      ["summary", "template_output"].includes(resource[0]) &&
      typeof resource[1] === "string"
    )
      return resource[1];
  } catch {}
  return id;
}
