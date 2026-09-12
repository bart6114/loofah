import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { RefreshCwIcon } from "lucide-react";

import { Button } from "@hypr/ui/components/ui/button";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { commands } from "~/types/tauri.gen";

export function RebuildIndexRow() {
  const { t } = useLingui();

  const rebuildMutation = useMutation({
    mutationFn: async () => {
      const result = await commands.sessionRebuildIndex();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: () => {
      sonnerToast.success(t`Notes refreshed from the files in your folder.`);
    },
    onError: (error: Error) => {
      sonnerToast.error(error.message);
    },
  });

  return (
    <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3">
      <div className="border-border bg-muted flex min-w-0 items-center gap-3 rounded-lg border px-4 py-3">
        <RefreshCwIcon className="text-muted-foreground size-4 shrink-0" />
        <p className="text-muted-foreground min-w-0 flex-1 truncate text-left text-sm">
          <Trans>
            Refresh the app if notes are missing or changes made outside Loofah
            are not showing.
          </Trans>
        </p>
      </div>
      <Button
        variant="outline"
        className="h-9 w-full justify-center"
        onClick={() => rebuildMutation.mutate()}
        disabled={rebuildMutation.isPending}
      >
        {rebuildMutation.isPending ? (
          <Trans>Refreshing...</Trans>
        ) : (
          <Trans>Refresh notes from disk</Trans>
        )}
      </Button>
    </div>
  );
}
