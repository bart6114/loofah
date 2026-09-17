import { useForm } from "@tanstack/react-form";
import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import { ConflictReview } from "./sync-conflicts";

import {
  commands,
  events,
  type SyncAction,
  type SyncStatus,
} from "~/types/tauri.gen";

const subscriptions = new WeakMap<
  QueryClient,
  { count: number; unlisten: Promise<() => void> }
>();
const identity = (status: SyncStatus) => status;
const enabled = (status: SyncStatus) => status.enabled;

function useSyncQuery<T>(select: (status: SyncStatus) => T) {
  const client = useQueryClient();
  useEffect(() => {
    let subscription = subscriptions.get(client);
    if (!subscription) {
      subscription = {
        count: 0,
        unlisten: events.syncStatusChanged.listen(({ payload }) =>
          client.setQueryData(["sync-status"], payload.status),
        ),
      };
      subscriptions.set(client, subscription);
    }
    subscription.count++;
    return () => {
      if (--subscription.count === 0) {
        subscriptions.delete(client);
        void subscription.unlisten.then((unlisten) => unlisten());
      }
    };
  }, [client]);
  return useQuery({
    queryKey: ["sync-status"],
    queryFn: () => commands.syncStatus(),
    select,
  });
}
export function useSyncStatus() {
  return useSyncQuery(identity);
}
export function useSyncEnabled() {
  return useSyncQuery(enabled);
}

export function SettingsSync() {
  const status = useSyncStatus();
  const client = useQueryClient();
  const action = useMutation({
    mutationFn: async (value: SyncAction) => {
      const result = await commands.syncAction(value);
      if (result.status === "error") throw new Error(result.error);
    },
    onSettled: () => client.invalidateQueries({ queryKey: ["sync-status"] }),
  });
  const pairingForm = useForm({
    defaultValues: { code: "" },
    onSubmit: async ({ value }) => {
      await action.mutateAsync({ approve_mac: { code: value.code.trim() } });
    },
  });
  if (status.isPending) return <p>Loading sync settings…</p>;
  if (status.error) return <p role="alert">{status.error.message}</p>;
  const value: SyncStatus = status.data;
  if (!value.enabled) return <p>Sync is available in Loofah Staging.</p>;
  const button = (label: string, command: SyncAction) => (
    <Button
      variant="outline"
      disabled={
        action.isPending &&
        !(
          typeof command === "string" &&
          ["cancel", "pause", "disconnect"].includes(command)
        )
      }
      onClick={() => action.mutate(command)}
    >
      {label}
    </Button>
  );
  return (
    <div className="flex max-w-2xl flex-col gap-6">
      <div>
        <h1 className="text-xl font-semibold">Sync</h1>
        <p className="text-muted-foreground mt-2">
          Connect a copied test vault to Loofah Staging. Your recovery kit is
          required to decrypt your files on another Mac.
        </p>
      </div>
      <section className="flex flex-col gap-3">
        <p className="text-sm break-all">Selected vault: {value.vaultPath}</p>
        <p role="status">
          {(
            {
              disconnected: "Disconnected",
              authorizing: "Waiting for browser approval",
              authorization_required: "Sign in again",
              save_recovery_kit: "Save your recovery kit",
              confirm_recovery_kit: "Reimport your saved recovery kit",
              import_recovery_kit: "Import your existing recovery kit",
              paused: "Paused",
              connected: "Connected",
              pairing: "Waiting for the other Mac",
              recovery_required: "Reconciliation required",
            } as Record<string, string>
          )[value.phase] ?? value.phase}
        </p>
        {(value.error || action.error) && (
          <p role="alert" className="text-destructive">
            {action.error?.message ?? value.error}
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          {value.phase === "recovery_required" && (
            <>
              <p>
                Reconciliation retains the previous baselines, pending uploads
                and checkpoints in a local archive, then compares this copied
                vault with the current cloud state. Divergent content requires
                an explicit choice.
              </p>
              {button("Reconcile selected vault", "reconcile_recovery")}
            </>
          )}
          {["disconnected", "authorization_required"].includes(value.phase) &&
            button("Connect", "connect")}
          {value.phase === "authorizing" && (
            <>
              {value.userCode ? (
                <p>
                  Approve code <strong>{value.userCode}</strong> in your
                  browser.
                </p>
              ) : (
                <p>Requesting a browser approval code…</p>
              )}
              {button("Cancel", "cancel")}
            </>
          )}
          {value.phase === "save_recovery_kit" &&
            button("Save recovery kit…", "save_recovery_kit")}
          {["confirm_recovery_kit", "import_recovery_kit"].includes(
            value.phase,
          ) && button("Import recovery kit…", "import_recovery_kit")}
          {value.phase === "import_recovery_kit" &&
            button("Pair with a trusted Mac", "pair_new_mac")}
          {value.phase === "pairing" && (
            <>
              <p>
                Enter this code on the trusted Mac:{" "}
                <strong>{value.pairingCode ?? "Connecting…"}</strong>. It
                expires after ten minutes.
              </p>
              {button("Cancel pairing", "cancel")}
            </>
          )}
          {value.phase === "paused" && button("Resume syncing", "resume")}
          {value.phase === "connected" && button("Pause syncing", "pause")}
          {!["disconnected", "authorizing"].includes(value.phase) &&
            button("Disconnect", "disconnect")}
        </div>
        {value.phase === "confirm_recovery_kit" && (
          <p>
            Keep the saved recovery kit somewhere safe. Select that file again
            to confirm it was saved before this Mac enrolls.
          </p>
        )}
        <p>{value.pendingWork} pending operations</p>
        {value.activeTransfers > 0 && (
          <div>
            <progress
              className="w-full"
              max={value.transferBytes || 1}
              value={value.transferredBytes}
            />
            <p>
              {(value.transferredBytes / 1e6).toFixed(1)} of{" "}
              {(value.transferBytes / 1e6).toFixed(1)} MB ·{" "}
              {value.activeTransfers} active transfers
            </p>
          </div>
        )}
        {value.lastSuccess && (
          <p>
            Last successful sync: {new Date(value.lastSuccess).toLocaleString()}
          </p>
        )}
        {value.quotaBytes > 0 && (
          <p>
            {(value.usedBytes / 1e9).toFixed(2)} GB of{" "}
            {(value.quotaBytes / 1e9).toFixed(0)} GB used, including retained
            history.
          </p>
        )}
        {value.conflicts.length > 0 && (
          <p>
            {value.conflicts.length} conflicting entities need your review. Both
            versions are preserved.
          </p>
        )}
      </section>
      {value.conflicts.map((conflict) => (
        <ConflictReview
          key={JSON.stringify(conflict.entity)}
          conflict={conflict}
        />
      ))}
      {["paused", "connected"].includes(value.phase) && (
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void pairingForm.handleSubmit();
          }}
        >
          <h2 className="font-semibold">Approve a new Mac</h2>
          <p>
            Enter the code shown in Sync settings on your new Mac. This shares
            this vault’s encryption keys with that Mac.
          </p>
          <pairingForm.Field name="code">
            {(field) => (
              <label className="flex flex-col gap-2">
                Pairing code
                <input
                  className="rounded border p-2"
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                  onBlur={field.handleBlur}
                  autoComplete="off"
                  spellCheck={false}
                  required
                />
              </label>
            )}
          </pairingForm.Field>
          <Button type="submit" variant="outline" disabled={action.isPending}>
            Approve Mac
          </Button>
        </form>
      )}
      {!["disconnected", "authorizing"].includes(value.phase) && (
        <section className="flex flex-col gap-3">
          <h2 className="font-semibold">Devices</h2>
          <div>{button("Refresh devices", "refresh_devices")}</div>
          {value.devices.map((device) => (
            <div
              key={device.id}
              className="flex items-center justify-between gap-4 text-sm"
            >
              <span className="break-all">
                Mac {device.id.slice(0, 8)} ·{" "}
                {device.revoked_at
                  ? "Revoked"
                  : `Enrolled ${new Date(device.enrolled_at).toLocaleDateString()}`}
              </span>
              {!device.revoked_at &&
                button("Revoke access", { revoke_device: { id: device.id } })}
            </div>
          ))}
          <p className="text-muted-foreground text-sm">
            Disconnecting removes this account session from this Mac and keeps
            your local files. Revoking a device signs out all its bound
            sessions.
          </p>
        </section>
      )}
    </div>
  );
}
