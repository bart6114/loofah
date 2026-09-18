import { useForm } from "@tanstack/react-form";
import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { cn } from "@hypr/utils";

import { ConflictReview } from "./sync-conflicts";
import { deviceLabel, SyncConfirmation } from "./sync-display";

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
  const [revoking, setRevoking] = useState<string | null>(null);
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
  const ready = ["paused", "connected"].includes(value.phase);
  const setup =
    !value.hasStarted &&
    !["recovery_required", "authorization_required", "needs_unlock"].includes(
      value.phase,
    );
  const setupStep = ["disconnected", "authorizing"].includes(value.phase)
    ? 0
    : ready
      ? 2
      : 1;
  const phaseLabel =
    value.phase === "paused" && !value.hasStarted
      ? "Ready to sync"
      : value.phase === "connected"
        ? value.error
          ? "Sync needs attention"
          : value.conflicts.length
            ? "Review conflicting versions"
            : value.activeTransfers || value.pendingWork
              ? "Syncing…"
              : value.upToDate
                ? "Up to date"
                : "Checking for changes…"
        : ((
            {
              disconnected: "Not connected",
              authorizing: "Waiting for browser approval",
              authorization_required: "Sign in again",
              save_recovery_kit: "Protect your vault",
              confirm_recovery_kit: "Confirm your recovery kit",
              import_recovery_kit: "Unlock this vault on your Mac",
              paused: "Sync paused",
              pairing: "Connecting your devices",
              recovery_required: "Compare this Mac with the cloud",
              needs_unlock: "Unlock the credential store to continue",
            } as Record<string, string>
          )[value.phase] ?? value.phase);
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
          Keep this vault in sync across your own devices. Use a copied test
          vault during staging. This does not share files with other people.
        </p>
      </div>
      {setup && (
        <ol aria-label="Sync setup" className="flex flex-wrap gap-4 text-sm">
          {["Sign in", "Secure this Mac", "Start syncing"].map(
            (step, index) => (
              <li
                key={step}
                aria-current={index === setupStep ? "step" : undefined}
                className={cn([
                  index === setupStep
                    ? "font-semibold"
                    : "text-muted-foreground",
                ])}
              >
                {index < setupStep ? "✓" : `${index + 1}.`} {step}
              </li>
            ),
          )}
        </ol>
      )}
      <section className="flex flex-col gap-3">
        {value.accountEmail && <p>Signed in as {value.accountEmail}</p>}
        <p className="text-sm break-all">Vault folder: {value.vaultPath}</p>
        <p className="text-muted-foreground text-sm">
          Includes sessions, their recordings and attachments, plus your vault’s
          people, tags and global tasks.
        </p>
        <p role="status" className="font-semibold">
          {phaseLabel}
        </p>
        {value.notice && <p role="status">{value.notice}</p>}
        {value.phase === "paused" && !value.hasStarted && (
          <p>
            This Mac is ready. Start syncing to compare this folder with the
            cloud and upload your changes. You will choose between any
            conflicting versions.
          </p>
        )}
        {(value.error || action.error) && (
          <p role="alert" className="text-destructive">
            {action.error?.message ?? value.error}
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          {value.phase === "needs_unlock" && (
            <>
              <p>
                Unlock the selected credential store, then resume. For an
                encrypted-file store, use the CLI with an unlock file. Your
                existing keys are preserved.
              </p>
              {button("Resume after unlocking", "resume")}
            </>
          )}
          {value.phase === "recovery_required" && (
            <>
              <p>
                Cloud sync has changed. Your local files and previous sync state
                are safe. Compare this vault with the cloud before continuing;
                you will choose between any conflicting versions.
              </p>
              {button("Compare with cloud", "reconcile_recovery")}
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
              {value.userCode && button("Copy code", "copy_code")}
              {value.browserUrl &&
                button("Open browser again", "reopen_browser")}
              {button("Cancel", "cancel")}
            </>
          )}
          {value.phase === "save_recovery_kit" && (
            <div className="flex flex-col gap-3">
              <p>
                Save your recovery kit outside this vault, somewhere safe. It
                unlocks your encrypted files if you lose access to your devices.
                Resetting your password cannot replace it.
              </p>
              {button("Save recovery kit…", "save_recovery_kit")}
            </div>
          )}
          {value.phase === "confirm_recovery_kit" &&
            button("Choose the kit you just saved…", "import_recovery_kit")}
          {value.phase === "import_recovery_kit" && (
            <div className="flex flex-col gap-3">
              <p>
                You are signed in. To unlock your existing encrypted vault,
                choose either method:
              </p>
              <div className="flex flex-wrap gap-2">
                {button("Pair with a trusted device", "pair_new_mac")}
                {button("Import recovery kit…", "import_recovery_kit")}
              </div>
              <p className="text-muted-foreground text-sm">
                Pairing needs another device already connected to this account.
                Otherwise select your saved .loofah-key file.
              </p>
            </div>
          )}
          {value.phase === "pairing" && (
            <>
              {value.pairingRole === "approver" ? (
                <p>
                  Approving your new device. Keep both clients open while the
                  keys transfer securely.
                </p>
              ) : (
                <p>
                  On your trusted device, open Settings → Sync → Approve a new
                  device, and enter{" "}
                  <strong>{value.pairingCode ?? "Connecting…"}</strong>. You can
                  also run loof-staging sync pair approve with this code on a
                  trusted CLI. Both devices must use the same account. The code
                  expires after ten minutes.
                </p>
              )}
              {value.pairingRole !== "approver" &&
                value.pairingCode &&
                button("Copy code", "copy_code")}
              {button("Cancel pairing", "cancel")}
            </>
          )}
          {value.phase === "paused" &&
            button(
              value.hasStarted ? "Resume syncing" : "Start syncing",
              "resume",
            )}
          {value.phase === "connected" && button("Pause syncing", "pause")}
          {!["disconnected", "authorizing"].includes(value.phase) &&
            button("Disconnect", "disconnect")}
        </div>
        {value.phase === "confirm_recovery_kit" && (
          <p>
            Select the .loofah-key file you just saved. This checks that your
            recovery kit works before this Mac can sync.
          </p>
        )}
        {ready && value.pendingWork > 0 && (
          <p>{value.pendingWork} changes waiting to sync</p>
        )}
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
            {value.conflicts.length} items have conflicting versions to review.
            Both versions are preserved.
          </p>
        )}
      </section>
      {value.conflicts.map((conflict) => (
        <ConflictReview
          key={JSON.stringify([
            conflict.entity,
            conflict.local,
            conflict.cloud,
          ])}
          conflict={conflict}
          status={value}
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
          <h2 className="font-semibold">Approve a new device</h2>
          <p>
            Enter the code shown in Sync settings or the CLI on your new device.
            This shares this vault’s encryption keys with that device.
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
            Approve device
          </Button>
        </form>
      )}
      {!["disconnected", "authorizing"].includes(value.phase) && (
        <section className="flex flex-col gap-3">
          <h2 className="font-semibold">Devices</h2>
          <div>{button("Refresh devices", "refresh_devices")}</div>
          {value.devices.length === 0 && (
            <p>
              {ready
                ? "No devices are listed yet. Refresh to check your connected devices."
                : "Finish securing this Mac to add it to your devices."}
            </p>
          )}
          {value.devices.map((device) => (
            <DeviceRow
              key={`${device.id}:${device.name}`}
              device={device}
              status={value}
              pending={action.isPending}
              rename={(name) =>
                action.mutateAsync({ rename_device: { id: device.id, name } })
              }
              revoke={() => setRevoking(device.id)}
            />
          ))}
          <p className="text-muted-foreground text-sm">
            Disconnecting removes this account session from this Mac and keeps
            your local files. Revoking a device signs out all its bound
            sessions.
          </p>
        </section>
      )}
      {revoking && (
        <SyncConfirmation
          title={`Remove access for ${deviceLabel(revoking, value)}?`}
          confirmLabel="Revoke access"
          pending={action.isPending}
          onClose={() => setRevoking(null)}
          onConfirm={() =>
            action.mutate(
              { revoke_device: { id: revoking } },
              { onSuccess: () => setRevoking(null) },
            )
          }
        >
          <p>
            This signs that device out and stops its sync. Its local files stay
            on that device. Connecting it again will require your recovery kit
            or approval from a trusted device.
          </p>
          {action.error && <p role="alert">{action.error.message}</p>}
        </SyncConfirmation>
      )}
    </div>
  );
}

function DeviceRow({
  device,
  status,
  pending,
  rename,
  revoke,
}: {
  device: SyncStatus["devices"][number];
  status: SyncStatus;
  pending: boolean;
  rename: (name: string) => Promise<void>;
  revoke: () => void;
}) {
  const form = useForm({
    defaultValues: { name: device.name ?? "" },
    onSubmit: async ({ value }) => {
      await rename(value.name.trim());
    },
  });
  return (
    <div className="flex flex-col gap-2 rounded border p-3 text-sm">
      <p className="font-medium">{deviceLabel(device.id, status)}</p>
      <p>
        {device.revoked_at
          ? "Access removed"
          : `Connected ${new Date(device.enrolled_at).toLocaleDateString()}`}
      </p>
      {!device.revoked_at && (
        <>
          <form
            className="flex flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void form.handleSubmit();
            }}
          >
            <form.Field name="name">
              {(field) => (
                <label className="flex flex-col gap-1">
                  Device name
                  <input
                    className="rounded border p-2"
                    value={field.state.value}
                    onBlur={field.handleBlur}
                    onChange={(event) => field.handleChange(event.target.value)}
                    placeholder="e.g. Work MacBook"
                    maxLength={80}
                    required
                  />
                </label>
              )}
            </form.Field>
            <Button variant="outline" type="submit" disabled={pending}>
              Save name
            </Button>
          </form>
          <Button variant="outline" disabled={pending} onClick={revoke}>
            Revoke access
          </Button>
        </>
      )}
    </div>
  );
}
