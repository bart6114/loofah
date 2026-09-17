import { useMutation, useQueryClient } from "@tanstack/react-query";

import { Button } from "@hypr/ui/components/ui/button";

import {
  commands,
  type SyncAction,
  type SyncConflict,
} from "~/types/tauri.gen";

export function ConflictReview({ conflict }: { conflict: SyncConflict }) {
  const client = useQueryClient();
  const run = async (action: SyncAction) => {
    const result = await commands.syncAction(action);
    if (result.status === "error") throw new Error(result.error);
    return result.data;
  };
  const preview = useMutation({
    mutationFn: (revision: string) =>
      run({ preview: { entity: conflict.entity, revision } }),
  });
  const action = useMutation({
    mutationFn: run,
    onSuccess: () => client.invalidateQueries({ queryKey: ["sync-status"] }),
  });
  return (
    <section className="flex flex-col gap-3 rounded border p-4">
      <h2 className="font-semibold">
        Conflict:{" "}
        {conflict.entity.kind === "session"
          ? `Session ${conflict.entity.id}`
          : `${conflict.entity.kind} registry`}
      </h2>
      <p>
        Review both saved versions, then choose which becomes the new cloud
        version. Both remain in history.{" "}
        {conflict.entity.kind !== "session" &&
          "This choice replaces the complete global registry."}
      </p>
      <div className="flex gap-2">
        <Button
          variant="outline"
          onClick={() => preview.mutate(conflict.local)}
        >
          Preview saved edit
        </Button>
        <Button
          variant="outline"
          onClick={() => preview.mutate(conflict.cloud)}
        >
          Preview cloud
        </Button>
      </div>
      {preview.data?.preview && (
        <>
          <p>
            {preview.variables === conflict.local
              ? "Saved divergent version"
              : "Cloud version"}
          </p>
          <pre className="max-h-64 overflow-auto rounded border p-3 text-sm whitespace-pre-wrap">
            {preview.data.preview.text ||
              (preview.data.preview.deleted
                ? "Deleted entity"
                : "No note text")}
          </pre>
          <Button
            variant="outline"
            onClick={() =>
              action.mutate({
                export: {
                  entity: conflict.entity,
                  revision: preview.variables!,
                },
              })
            }
          >
            Export preview…
          </Button>
        </>
      )}
      <div className="flex gap-2">
        <Button
          disabled={action.isPending}
          onClick={() =>
            action.mutate({
              restore: { entity: conflict.entity, revision: conflict.local },
            })
          }
        >
          Use saved edit
        </Button>
        <Button
          disabled={action.isPending}
          onClick={() =>
            action.mutate({
              restore: { entity: conflict.entity, revision: conflict.cloud },
            })
          }
        >
          Use cloud version
        </Button>
      </div>
      {(action.error || preview.error) && (
        <p role="alert">{action.error?.message ?? preview.error?.message}</p>
      )}
    </section>
  );
}
