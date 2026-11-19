import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ask } from "@tauri-apps/plugin-dialog";
import { useId } from "react";

import { Switch } from "@hypr/ui/components/ui/switch";

import { commands } from "~/types/tauri.gen";

const queryKey = ["iphone-capture-receiver"];

export function IphoneCapturesSetting() {
  const id = useId();
  const client = useQueryClient();
  const status = useQuery({
    queryKey,
    queryFn: async () => {
      const result = await commands.iphoneCaptureReceiver();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    refetchInterval: 10_000,
    networkMode: "always",
  });
  const mutation = useMutation({
    mutationFn: async (enabled: boolean) => {
      if (!status.data) return;
      const { receiver, vault } = status.data;
      if (
        enabled &&
        receiver &&
        !(await ask(
          `New and pending iPhone captures will be sent to this vault:\n${vault}`,
          {
            title: "Change the receiving Mac?",
            okLabel: "Use this Mac",
            cancelLabel: "Cancel",
          },
        ))
      )
        return;
      const result = await commands.iphoneCaptureSetReceiver(enabled, receiver);
      if (result.status === "error") throw new Error(result.error);
    },
    onSettled: () => client.invalidateQueries({ queryKey }),
    networkMode: "always",
  });
  const message = mutation.error?.message ?? status.error?.message;
  return (
    <div className="rounded-lg border px-4 py-3">
      <div className="flex items-center justify-between gap-4">
        <label htmlFor={id} className="text-sm font-medium">
          Receive iPhone captures on this Mac
        </label>
        <Switch
          id={id}
          checked={status.data?.enabled ?? false}
          disabled={
            !status.data?.available || mutation.isPending || status.isError
          }
          onCheckedChange={mutation.mutate}
        />
      </div>
      {status.data?.available === false && (
        <p className="text-muted-foreground mt-1 text-xs">
          Turn on iCloud Drive on this Mac to receive captures.
        </p>
      )}
      {message && (
        <p className="text-destructive mt-1 text-xs" role="alert">
          {message}
        </p>
      )}
    </div>
  );
}
