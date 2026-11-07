import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { ApplicationRequestError } from "./errors";
import { VaultNotesWorkspace } from "./notes-workspace";
import type { Me, VaultInfo } from "./protocol.gen";
import { NativeRemoteConnection } from "./remote";
import { connectRemoteVault } from "./vault-client";
import { CreateSharedVault, VaultSharing } from "./vault-sharing";

export default function ServerPreview() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="outline" onClick={() => setOpen(true)}>
        <Trans>Try server notes</Trans>
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="flex h-[90vh] w-[94vw] max-w-5xl flex-col overflow-hidden">
          <DialogHeader>
            <DialogTitle>
              <Trans>Server notes preview</Trans>
            </DialogTitle>
            <DialogDescription>
              <Trans>
                Connect to your server to edit notes and play recordings. Your
                standalone setup stays available.
              </Trans>
            </DialogDescription>
          </DialogHeader>
          {open && <Connection />}
        </DialogContent>
      </Dialog>
    </>
  );
}

function Connection() {
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const connect = useMutation({
    mutationFn: async (server: string) => {
      const connection = await NativeRemoteConnection.connect(server);
      if (
        !connection.info.capabilities.features.includes("notes") ||
        !connection.info.capabilities.features.includes("change_stream")
      ) {
        await connection.disconnect();
        throw new Error(
          "This server does not support the note preview's requests and notifications",
        );
      }
      try {
        return { connection, me: await connection.restore() };
      } catch (error) {
        if (
          error instanceof ApplicationRequestError &&
          error.detail.code === "unauthenticated"
        )
          return { connection, me: null };
        await connection.disconnect();
        throw error;
      }
    },
    onSuccess: ({ connection }) => {
      if (!alive.current) void connection.disconnect().catch(() => {});
    },
  });
  const connection = connect.data?.connection;
  useEffect(
    () => () => {
      if (connection) void connection.disconnect().catch(() => {});
    },
    [connection],
  );
  const login = useMutation({
    mutationFn: async () => {
      if (!connection) throw new Error("Connect to a server first");
      return connection.signIn();
    },
  });
  const leave = useMutation({
    mutationFn: async (signOut: boolean) => {
      if (connection) {
        if (signOut) await connection.signOut();
        else await connection.disconnect();
      }
    },
    onSuccess: () => {
      login.reset();
      connect.reset();
    },
  });
  const form = useForm({
    defaultValues: { server: "" },
    onSubmit: async ({ value }) => {
      await connect.mutateAsync(value.server);
    },
  });
  const me = login.data ?? connect.data?.me;
  if (!connection)
    return (
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          event.stopPropagation();
          void form.handleSubmit().catch(() => {});
        }}
      >
        <form.Field name="server">
          {(field) => (
            <label className="flex flex-col gap-1 text-sm">
              <Trans>Server address</Trans>
              <input
                className="bg-background rounded border px-3 py-2"
                type="url"
                required
                placeholder="https://notes.company.example"
                value={field.state.value}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            </label>
          )}
        </form.Field>
        <Button type="submit" disabled={connect.isPending}>
          {connect.isPending ? (
            <Trans>Connecting…</Trans>
          ) : (
            <Trans>Connect</Trans>
          )}
        </Button>
        {connect.error && (
          <p role="alert" className="text-destructive text-sm">
            {connect.error.message}
          </p>
        )}
      </form>
    );
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="text-muted-foreground">{connection.info.server}</span>
        <div className="flex gap-2">
          <Button
            variant="ghost"
            size="sm"
            disabled={leave.isPending}
            onClick={() => leave.mutate(false)}
          >
            <Trans>Disconnect</Trans>
          </Button>
          {me && (
            <Button
              variant="ghost"
              size="sm"
              disabled={leave.isPending}
              onClick={() => leave.mutate(true)}
            >
              <Trans>Sign out</Trans>
            </Button>
          )}
        </div>
      </div>
      {leave.error && (
        <p role="alert" className="text-destructive text-xs">
          {leave.error.message}
        </p>
      )}
      {me ? (
        <ConnectedVaults connection={connection} me={me} />
      ) : (
        <div className="flex flex-col gap-3">
          <p className="text-sm">
            <Trans>Sign in through your company to open its vaults.</Trans>
          </p>
          <Button disabled={login.isPending} onClick={() => login.mutate()}>
            <Trans>Company sign-in</Trans>
          </Button>
          {login.isPending && (
            <Button
              variant="ghost"
              onClick={() => void connection.cancelSignIn()}
            >
              <Trans>Cancel sign-in</Trans>
            </Button>
          )}
          {login.error && (
            <p role="alert" className="text-destructive text-xs">
              {login.error.message}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function ConnectedVaults({
  connection,
  me,
}: {
  connection: NativeRemoteConnection;
  me: Me;
}) {
  const queryClient = useQueryClient();
  const catalog = useQuery({
    queryKey: [
      "server-preview",
      connection.info.server,
      me.member.id,
      "vaults",
    ],
    queryFn: ({ signal }) =>
      connection.request({ kind: "vaults" }, "vaults", { signal }),
    retry: false,
  });
  const [selected, setSelected] = useState<VaultInfo | null>(null);
  useEffect(
    () => () => {
      const queryKey = ["server-preview", connection.info.server, me.member.id];
      void queryClient.cancelQueries({ queryKey });
      queryClient.removeQueries({ queryKey });
    },
    [queryClient, connection.info.server, me.member.id],
  );

  useEffect(() => {
    if (!catalog.data) return;
    const next = selected
      ? catalog.data.find((vault) => vault.id === selected.id)
      : catalog.data.find((vault) => vault.id === me.personal_vault_id);
    if (next && next !== selected) setSelected(next);
  }, [catalog.data, me.personal_vault_id, selected]);
  const changed = useCallback(() => {
    void queryClient.invalidateQueries({
      queryKey: [
        "server-preview",
        connection.info.server,
        me.member.id,
        "vaults",
      ],
    });
  }, [queryClient, connection.info.server, me.member.id]);
  const selectedId = selected?.id;
  const selectedRole = selected?.role;
  const client = useMemo(
    () =>
      selectedId && selectedRole
        ? connectRemoteVault(connection, me.member.id, {
            id: selectedId,
            role: selectedRole,
          })
        : null,
    [connection, me.member.id, selectedId, selectedRole],
  );
  const unavailable =
    selected &&
    catalog.data &&
    !catalog.data.some((vault) => vault.id === selected.id);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
        <label className="flex items-center gap-2 text-sm">
          <Trans>Vault</Trans>
          <select
            className="bg-background rounded border px-2 py-1"
            value={selected?.id ?? ""}
            onChange={(event) => {
              const vault = catalog.data?.find(
                (vault) => vault.id === event.target.value,
              );
              if (vault) setSelected(vault);
            }}
          >
            {!selected && (
              <option value="">
                <Trans>Choose a vault</Trans>
              </option>
            )}
            {unavailable && (
              <option value={selected.id} disabled>
                <Trans>Unavailable vault</Trans>
              </option>
            )}
            {catalog.data?.map((vault) => (
              <option value={vault.id} key={vault.id}>
                {vault.name}
              </option>
            ))}
          </select>
        </label>
        <div className="flex items-center gap-2">
          <span className="text-muted-foreground text-xs">
            {me.member.name}
          </span>
          {connection.info.capabilities.features.includes(
            "vault_membership",
          ) && (
            <>
              <CreateSharedVault
                connection={connection}
                memberId={me.member.id}
                onCreated={setSelected}
              />
              {selected && !unavailable && (
                <VaultSharing
                  connection={connection}
                  memberId={me.member.id}
                  vault={selected}
                />
              )}
            </>
          )}
        </div>
      </div>
      {catalog.error && (
        <p role="alert" className="text-destructive text-xs">
          {catalog.error.message}
        </p>
      )}
      {client && (
        <VaultNotesWorkspace client={client} onCatalogChanged={changed} />
      )}
    </div>
  );
}
