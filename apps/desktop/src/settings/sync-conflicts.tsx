import { useMutation, useQueryClient } from "@tanstack/react-query";

import { Button } from "@hypr/ui/components/ui/button";

import { componentLabels, deviceLabel, SyncPreviewText } from "./sync-display";

import {
  commands,
  type SyncAction,
  type SyncConflict,
  type SyncStatus,
} from "~/types/tauri.gen";

export function ConflictReview({
  conflict,
  status,
}: {
  conflict: SyncConflict;
  status?: SyncStatus;
}) {
  const client = useQueryClient();
  const run = async (action: SyncAction) => {
    const result = await commands.syncAction(action);
    if (result.status === "error") throw new Error(result.error);
    return result.data;
  };
  const preview = useMutation({
    mutationFn: async () => {
      const saved = await run({
        preview: { entity: conflict.entity, revision: conflict.local },
      });
      const cloud = await run({
        preview: { entity: conflict.entity, revision: conflict.cloud },
      });
      if (!saved?.preview || !cloud?.preview)
        throw new Error(
          "Could not load both versions. Try again before choosing one.",
        );
      return { saved: saved.preview, cloud: cloud.preview };
    },
  });
  const action = useMutation({
    mutationFn: run,
    onSuccess: (result) => {
      if (!result?.cancelled)
        return client.invalidateQueries({ queryKey: ["sync-status"] });
    },
  });
  const title =
    preview.data?.saved.title ||
    preview.data?.cloud.title ||
    (conflict.entity.kind === "session"
      ? "Session with conflicting changes"
      : `Your vault’s ${conflict.entity.kind}`);
  return (
    <section className="flex flex-col gap-3 rounded border p-4">
      <h2 className="font-semibold">{title}</h2>
      <p>
        Two versions changed separately. Compare them and choose which to use
        across your Macs. Both versions remain in history.
      </p>
      {conflict.entity.kind !== "session" && (
        <p>This choice replaces all {conflict.entity.kind} in this vault.</p>
      )}
      {(!preview.data || action.error) && (
        <Button
          variant="outline"
          disabled={preview.isPending}
          onClick={() => {
            action.reset();
            preview.mutate();
          }}
        >
          {preview.isPending
            ? "Loading both versions…"
            : preview.data
              ? "Reload both versions"
              : "Review versions"}
        </Button>
      )}
      {preview.data && (
        <div className="grid gap-4 lg:grid-cols-2">
          {[
            {
              label: "Saved conflicting version",
              revision: conflict.local,
              value: preview.data.saved,
            },
            {
              label: "Current cloud version",
              revision: conflict.cloud,
              value: preview.data.cloud,
            },
          ].map(({ label, revision, value }) => (
            <section
              key={revision}
              className="flex min-w-0 flex-col gap-3 rounded border p-3"
            >
              <h3 className="font-medium">{label}</h3>
              <p className="text-sm">
                Synced {new Date(value.created_at).toLocaleString()} ·{" "}
                {deviceLabel(value.device_id, status)}
              </p>
              {value.captured_at && (
                <p className="text-sm">
                  Saved {new Date(value.captured_at).toLocaleString()}
                </p>
              )}
              <p className="text-sm">
                {value.deleted
                  ? "This version deletes the item."
                  : `Includes: ${componentLabels(value.files) || "No content"}`}
              </p>
              <p className="text-sm">
                Changes compared with this Mac’s last synced version:{" "}
                {componentLabels(value.changed) || "None"}
              </p>
              <SyncPreviewText
                kind={conflict.entity.kind}
                text={value.text}
                deleted={value.deleted}
              />
              <Button
                variant="outline"
                disabled={action.isPending}
                onClick={() =>
                  action.mutate({
                    export: { entity: conflict.entity, revision },
                  })
                }
              >
                Export {label.toLowerCase()}…
              </Button>
              <Button
                disabled={action.isPending}
                onClick={() =>
                  action.mutate({
                    restore: { entity: conflict.entity, revision },
                  })
                }
              >
                {revision === conflict.local
                  ? "Use saved version"
                  : "Use cloud version"}
              </Button>
            </section>
          ))}
        </div>
      )}
      {action.isSuccess &&
        typeof action.variables === "object" &&
        "export" in action.variables && (
          <p role="status">
            {action.data?.cancelled
              ? "Export cancelled."
              : "Version exported to the folder you selected."}
          </p>
        )}
      {(action.error || preview.error) && (
        <p role="alert" className="text-destructive">
          {action.error?.message ?? preview.error?.message}
        </p>
      )}
    </section>
  );
}
