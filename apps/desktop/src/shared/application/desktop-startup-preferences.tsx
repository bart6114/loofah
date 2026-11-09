import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { applicationResult } from "./errors";

import {
  commands,
  type DesktopConnectionProfile,
  type DesktopStartupSnapshot,
} from "~/types/tauri.gen";

const startupKey = ["device", "desktop-startup"];
const connectionsKey = ["device", "desktop-connections"];

export function DesktopStartupPreferences() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="outline" size="sm" onClick={() => setOpen(true)}>
        <Trans>Startup options</Trans>
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              <Trans>Desktop startup</Trans>
            </DialogTitle>
            <DialogDescription>
              <Trans>
                Choose where Loofah connects the next time it starts.
              </Trans>
            </DialogDescription>
          </DialogHeader>
          {open && <Preferences />}
        </DialogContent>
      </Dialog>
    </>
  );
}

function Preferences() {
  const cache = useQueryClient();
  const startup = useQuery({
    queryKey: startupKey,
    queryFn: async () =>
      applicationResult(await commands.applicationDesktopStartup()),
    networkMode: "always",
    retry: false,
  });
  const connections = useQuery({
    queryKey: connectionsKey,
    queryFn: async () =>
      applicationResult(await commands.applicationDesktopConnections()),
    networkMode: "always",
    retry: false,
  });
  const save = useMutation({
    mutationFn: async ({
      profileId,
      snapshot,
    }: {
      profileId: string;
      snapshot: DesktopStartupSnapshot;
    }) => {
      const profile = connections.data?.find((value) => value.id === profileId);
      if (profileId !== "local" && !profile)
        throw new Error("Choose an available server connection");
      const result = applicationResult(
        await commands.applicationSaveDesktopStartup({
          selection: profile
            ? {
                kind: "remote",
                profile_name: profile.name,
                profile_id: profile.id,
              }
            : { kind: "local" },
          expected_revision: snapshot.revision,
        }),
      );
      if (result.kind === "conflict")
        throw new Error(
          "Startup preferences changed. Reload them before saving again.",
        );
      return result.snapshot;
    },
    onSuccess: () => {
      void cache.invalidateQueries({ queryKey: startupKey });
    },
  });
  const snapshot = startup.data;
  return (
    <div className="flex flex-col gap-5">
      {snapshot && connections.data && (
        <Selection
          key={snapshot.revision}
          snapshot={snapshot}
          connections={connections.data}
          pending={save.isPending}
          onSave={(profileId) => save.mutate({ profileId, snapshot })}
        />
      )}
      {(startup.isPending || connections.isPending) && (
        <p role="status">
          <Trans>Loading startup preferences…</Trans>
        </p>
      )}
      {(startup.error || connections.error || save.error) && (
        <p role="alert" className="text-destructive text-sm">
          {(startup.error ?? connections.error ?? save.error)?.message}
        </p>
      )}
      {(startup.error || save.error) && (
        <Button variant="outline" onClick={() => void startup.refetch()}>
          <Trans>Reload preferences</Trans>
        </Button>
      )}
      {save.data && (
        <p role="status" className="text-sm">
          <Trans>Saved for the next app start.</Trans>
        </p>
      )}
      <AddConnection
        onSaved={() => {
          void cache.invalidateQueries({ queryKey: connectionsKey });
        }}
      />
    </div>
  );
}

function Selection({
  snapshot,
  connections,
  pending,
  onSave,
}: {
  snapshot: DesktopStartupSnapshot;
  connections: DesktopConnectionProfile[];
  pending: boolean;
  onSave: (profileId: string) => void;
}) {
  const selected =
    snapshot.selection.kind === "local"
      ? "local"
      : snapshot.selection.profile_id;
  const form = useForm({
    defaultValues: { profileId: selected },
    onSubmit: ({ value }) => onSave(value.profileId),
  });
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="profileId">
        {(field) => (
          <label className="flex flex-col gap-1 text-sm">
            <Trans>Open Loofah with</Trans>
            <select
              className="bg-background rounded border px-3 py-2"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
              disabled={pending}
            >
              <option value="local">
                <Trans>Standalone on this device</Trans>
              </option>
              {snapshot.selection.kind === "remote" &&
                !connections.some((profile) => profile.id === selected) && (
                  <option value={selected} disabled>
                    <Trans>Unavailable saved server</Trans>
                  </option>
                )}
              {connections.map((profile) => (
                <option value={profile.id} key={profile.id}>
                  {profile.name} · {profile.server}
                </option>
              ))}
            </select>
          </label>
        )}
      </form.Field>
      <p className="text-muted-foreground text-xs">
        <Trans>Standalone works offline and requires no login.</Trans>
      </p>
      <Button type="submit" disabled={pending}>
        <Trans>Save startup choice</Trans>
      </Button>
    </form>
  );
}

function AddConnection({ onSaved }: { onSaved: () => void }) {
  const create = useMutation({
    mutationFn: async (input: { name: string; server: string }) =>
      applicationResult(
        await commands.applicationCreateDesktopConnection(
          input.name,
          input.server,
        ),
      ),
    onSuccess: onSaved,
  });
  const form = useForm({
    defaultValues: { name: "company", server: "" },
    onSubmit: async ({ value }) => {
      await create.mutateAsync(value);
    },
  });
  return (
    <form
      className="flex flex-col gap-2 border-t pt-4"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit().catch(() => {});
      }}
    >
      <form.Field name="name">
        {(field) => (
          <label className="flex flex-col gap-1 text-sm">
            <Trans>Connection name</Trans>
            <input
              className="bg-background rounded border px-3 py-2"
              required
              maxLength={64}
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </label>
        )}
      </form.Field>
      <form.Field name="server">
        {(field) => (
          <label className="flex flex-col gap-1 text-sm">
            <Trans>Server address</Trans>
            <input
              className="bg-background rounded border px-3 py-2"
              required
              type="url"
              placeholder="https://notes.company.example"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </label>
        )}
      </form.Field>
      <Button type="submit" variant="outline" disabled={create.isPending}>
        <Trans>Add server connection</Trans>
      </Button>
      {create.error && (
        <p role="alert" className="text-destructive text-sm">
          {create.error.message}
        </p>
      )}
      {create.data && (
        <p role="status" className="text-sm">
          <Trans>Connection added. Select it above to use it at startup.</Trans>
        </p>
      )}
    </form>
  );
}
