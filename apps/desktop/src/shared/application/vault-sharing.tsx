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

import { ApplicationRequestError } from "./errors";
import type { VaultInfo, VaultRole } from "./protocol.gen";
import type { NativeRemoteConnection } from "./remote";

function catalogKey(connection: NativeRemoteConnection, memberId: string) {
  return ["server-preview", connection.info.server, memberId];
}

export function CreateSharedVault({
  connection,
  memberId,
  onCreated,
}: {
  connection: NativeRemoteConnection;
  memberId: string;
  onCreated: (vault: VaultInfo) => void;
}) {
  const [open, setOpen] = useState(false);
  const queryClient = useQueryClient();
  const create = useMutation({
    mutationFn: ({ name, requestId }: { name: string; requestId: string }) =>
      connection.request({ kind: "create_vault", input: { name } }, "vault", {
        requestId,
      }),
    onSuccess: async (vault) => {
      await queryClient.invalidateQueries({
        queryKey: catalogKey(connection, memberId),
      });
      onCreated(vault);
      setOpen(false);
      form.reset();
    },
  });
  const uncertain =
    !!create.error &&
    (!(create.error instanceof ApplicationRequestError) ||
      create.error.detail.mutation_outcome === "unknown");
  const form = useForm({
    defaultValues: { name: "" },
    onSubmit: async ({ value }) => {
      if (!uncertain)
        await create.mutateAsync({
          name: value.name,
          requestId: crypto.randomUUID(),
        });
    },
  });
  return (
    <>
      <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
        <Trans>Create shared vault</Trans>
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              <Trans>Create shared vault</Trans>
            </DialogTitle>
            <DialogDescription>
              <Trans>
                Your personal vault stays private. Share this new vault with
                existing company members.
              </Trans>
            </DialogDescription>
          </DialogHeader>
          <form
            className="flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void form.handleSubmit().catch(() => {});
            }}
          >
            <form.Field name="name">
              {(field) => (
                <label className="flex flex-col gap-1 text-sm">
                  <Trans>Vault name</Trans>
                  <input
                    className="bg-background rounded border px-3 py-2"
                    required
                    maxLength={120}
                    readOnly={create.isPending || uncertain}
                    value={field.state.value}
                    onChange={(event) => field.handleChange(event.target.value)}
                  />
                </label>
              )}
            </form.Field>
            {create.error && (
              <p role="alert" className="text-destructive text-sm">
                {create.error.message}
              </p>
            )}
            {uncertain ? (
              <>
                <p className="text-muted-foreground text-xs">
                  <Trans>
                    The server may have created this vault. Retry the same
                    request to recover its result.
                  </Trans>
                </p>
                <Button
                  type="button"
                  disabled={create.isPending}
                  onClick={() =>
                    create.variables && create.mutate(create.variables)
                  }
                >
                  <Trans>Retry creation</Trans>
                </Button>
              </>
            ) : (
              <Button type="submit" disabled={create.isPending}>
                <Trans>Create vault</Trans>
              </Button>
            )}
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}

export function VaultSharing({
  connection,
  memberId,
  vault,
}: {
  connection: NativeRemoteConnection;
  memberId: string;
  vault: VaultInfo;
}) {
  const [open, setOpen] = useState(false);
  if (vault.kind !== "shared") return null;
  return (
    <>
      <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
        <Trans>Vault members</Trans>
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{vault.name}</DialogTitle>
            <DialogDescription>
              <Trans>
                Only existing company members can join. Readers can view;
                editors can change content; owners also manage access.
              </Trans>
            </DialogDescription>
          </DialogHeader>
          {open && (
            <SharingForm
              key={vault.id}
              connection={connection}
              memberId={memberId}
              vault={vault}
              onDeleted={() => setOpen(false)}
            />
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}

function SharingForm({
  connection,
  memberId,
  vault,
  onDeleted,
}: {
  connection: NativeRemoteConnection;
  memberId: string;
  vault: VaultInfo;
  onDeleted: () => void;
}) {
  const queryClient = useQueryClient();
  const prefix = catalogKey(connection, memberId);
  const members = useQuery({
    queryKey: [...prefix, "memberships", vault.id],
    queryFn: ({ signal }) =>
      connection.request(
        { kind: "memberships", vault_id: vault.id },
        "memberships",
        { signal },
      ),
    retry: false,
  });
  const owner = vault.role === "owner";
  const directory = useQuery({
    queryKey: [...prefix, "directory"],
    queryFn: ({ signal }) =>
      connection.request({ kind: "directory" }, "directory", { signal }),
    enabled: owner,
    retry: false,
  });
  const refresh = () => queryClient.invalidateQueries({ queryKey: prefix });
  const change = useMutation({
    mutationFn: async ({
      id,
      role,
    }: {
      id: string;
      role: VaultRole | null;
    }) => {
      if (!owner) throw new Error("Only vault owners can change access");
      if (role)
        return connection.request(
          {
            kind: "set_membership",
            vault_id: vault.id,
            member_id: id,
            input: { role, expected_revision: vault.revision },
          },
          "vault_mutation",
        );
      return connection.request(
        {
          kind: "remove_membership",
          vault_id: vault.id,
          member_id: id,
          expected_revision: vault.revision,
        },
        "vault_mutation",
      );
    },
    onSuccess: refresh,
  });
  const rename = useMutation({
    mutationFn: async (name: string) => {
      if (!owner) throw new Error("Only vault owners can rename a vault");
      return connection.request(
        {
          kind: "rename_vault",
          vault_id: vault.id,
          input: { name, expected_revision: vault.revision },
        },
        "vault_mutation",
      );
    },
    onSuccess: refresh,
  });
  const remove = useMutation({
    mutationFn: async () => {
      if (!owner) throw new Error("Only vault owners can delete a vault");
      return connection.request(
        {
          kind: "delete_vault",
          vault_id: vault.id,
          expected_revision: vault.revision,
        },
        "vault_mutation",
      );
    },
    onSuccess: async () => {
      await refresh();
      onDeleted();
    },
  });
  const reload = useMutation({
    mutationFn: async () => {
      await refresh();
      change.reset();
      rename.reset();
      remove.reset();
    },
  });
  const error =
    change.error ??
    rename.error ??
    remove.error ??
    members.error ??
    directory.error;
  const blocked =
    error instanceof ApplicationRequestError &&
    (error.detail.code === "conflict" ||
      error.detail.mutation_outcome === "unknown");
  const denied = [change.error, rename.error, remove.error, members.error].some(
    (error) =>
      error instanceof ApplicationRequestError &&
      ["unauthenticated", "forbidden"].includes(error.detail.code),
  );
  const busy =
    change.isPending ||
    rename.isPending ||
    remove.isPending ||
    reload.isPending ||
    blocked;
  const form = useForm({
    defaultValues: { member: "", role: "reader" as VaultRole },
    onSubmit: async ({ value }) => {
      if (owner && value.member && !busy)
        await change.mutateAsync({ id: value.member, role: value.role });
    },
  });
  const renameForm = useForm({
    defaultValues: { name: vault.name },
    onSubmit: async ({ value }) => {
      if (!busy) await rename.mutateAsync(value.name);
    },
  });
  const deleteForm = useForm({
    defaultValues: { confirmed: false },
    onSubmit: async ({ value }) => {
      if (value.confirmed && !busy) await remove.mutateAsync();
    },
  });
  const owners =
    members.data?.filter((entry) => entry.role === "owner").length ?? 0;
  const available =
    directory.data?.filter(
      (entry) =>
        !members.data?.some((membership) => membership.member.id === entry.id),
    ) ?? [];
  if (denied)
    return (
      <p role="alert" className="text-destructive text-sm">
        <Trans>
          Access to this vault changed. Close this panel and reload your vaults.
        </Trans>
      </p>
    );
  return (
    <div className="flex flex-col gap-4">
      {error && (
        <div role="alert" className="text-destructive text-sm">
          {error.message}
          <Button
            variant="ghost"
            size="sm"
            disabled={
              reload.isPending ||
              change.isPending ||
              rename.isPending ||
              remove.isPending
            }
            onClick={() => reload.mutate()}
          >
            <Trans>Reload access</Trans>
          </Button>
        </div>
      )}
      {members.isPending ? (
        <p>
          <Trans>Loading members…</Trans>
        </p>
      ) : (
        <ul className="flex flex-col gap-2">
          {members.data?.map((entry) => {
            const lastOwner = entry.role === "owner" && owners <= 1;
            return (
              <li
                className="flex items-center justify-between gap-3 text-sm"
                key={entry.member.id}
              >
                <span>{entry.member.name}</span>
                {owner ? (
                  <div className="flex gap-2">
                    <label>
                      <span className="sr-only">
                        <Trans>Role for {entry.member.name}</Trans>
                      </span>
                      <RoleSelect
                        value={entry.role}
                        disabled={busy || lastOwner}
                        onChange={(role) =>
                          change.mutate({ id: entry.member.id, role })
                        }
                      />
                    </label>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy || lastOwner}
                      onClick={() =>
                        change.mutate({ id: entry.member.id, role: null })
                      }
                    >
                      <Trans>Remove</Trans>
                    </Button>
                  </div>
                ) : (
                  <span>{entry.role}</span>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {owner && (
        <>
          <form
            className="flex flex-col gap-2 border-t pt-3"
            onSubmit={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void form.handleSubmit().catch(() => {});
            }}
          >
            <form.Field name="member">
              {(field) => (
                <label className="flex flex-col gap-1 text-sm">
                  <Trans>Company member</Trans>
                  <select
                    className="bg-background rounded border px-2 py-1"
                    required
                    disabled={busy || !members.data}
                    value={field.state.value}
                    onChange={(event) => field.handleChange(event.target.value)}
                  >
                    <option value="">
                      <Trans>Choose a member</Trans>
                    </option>
                    {available.map((entry) => (
                      <option key={entry.id} value={entry.id}>
                        {entry.name}
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </form.Field>
            <form.Field name="role">
              {(field) => (
                <label className="flex items-center gap-2 text-sm">
                  <Trans>Role</Trans>
                  <RoleSelect
                    value={field.state.value}
                    disabled={busy}
                    onChange={field.handleChange}
                  />
                </label>
              )}
            </form.Field>
            <Button
              type="submit"
              disabled={busy || !members.data || !!members.error}
            >
              <Trans>Add member</Trans>
            </Button>
          </form>
          <form
            className="flex flex-col gap-2 border-t pt-3"
            onSubmit={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void renameForm.handleSubmit().catch(() => {});
            }}
          >
            <renameForm.Field name="name">
              {(field) => (
                <label className="flex flex-col gap-1 text-sm">
                  <Trans>Vault name</Trans>
                  <input
                    className="bg-background rounded border px-3 py-2"
                    required
                    maxLength={120}
                    disabled={busy}
                    value={field.state.value}
                    onChange={(event) => field.handleChange(event.target.value)}
                  />
                </label>
              )}
            </renameForm.Field>
            <Button type="submit" variant="outline" disabled={busy}>
              <Trans>Rename vault</Trans>
            </Button>
          </form>
          <form
            className="flex flex-col gap-2 border-t pt-3"
            onSubmit={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void deleteForm.handleSubmit().catch(() => {});
            }}
          >
            <p className="text-muted-foreground text-xs">
              <Trans>
                Deleting removes everyone's access. Server files are retained
                for operator recovery.
              </Trans>
            </p>
            <deleteForm.Field name="confirmed">
              {(field) => (
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={field.state.value}
                    onChange={(event) =>
                      field.handleChange(event.target.checked)
                    }
                  />
                  <Trans>Delete this shared vault</Trans>
                </label>
              )}
            </deleteForm.Field>
            <deleteForm.Subscribe selector={(state) => state.values.confirmed}>
              {(confirmed) => (
                <Button
                  type="submit"
                  variant="destructive"
                  disabled={busy || !confirmed}
                >
                  <Trans>Delete vault</Trans>
                </Button>
              )}
            </deleteForm.Subscribe>
          </form>
        </>
      )}
    </div>
  );
}

function RoleSelect({
  value,
  disabled,
  onChange,
}: {
  value: VaultRole;
  disabled: boolean;
  onChange: (role: VaultRole) => void;
}) {
  return (
    <select
      className="bg-background rounded border px-2 py-1"
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value as VaultRole)}
    >
      <option value="reader">
        <Trans>Reader</Trans>
      </option>
      <option value="editor">
        <Trans>Editor</Trans>
      </option>
      <option value="owner">
        <Trans>Owner</Trans>
      </option>
    </select>
  );
}
