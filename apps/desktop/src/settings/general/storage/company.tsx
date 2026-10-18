import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { flushNoteEditors } from "@hypr/editor/note";
import { Button } from "@hypr/ui/components/ui/button";

import {
  companyAction,
  companyRequest,
  type CompanyRole,
  type CompanyUser,
  type CompanyVault,
  type PendingCompanyDraft,
  useCompanyStatus,
} from "~/shared/company-vault";
import { flushDatabaseWrites } from "~/shared/write-queue";

export function CompanyStorageSettings() {
  const queryClient = useQueryClient();
  const { data: status } = useCompanyStatus();
  const connected = Boolean(status?.profile);
  const vaults = useQuery({
    queryKey: [
      "company",
      status?.profile?.server_id,
      status?.profile?.user_id,
      "vaults",
    ],
    queryFn: () => companyRequest<CompanyVault[]>("GET", "/v1/vaults"),
    enabled: connected,
    retry: false,
  });
  const login = useMutation({
    mutationFn: async (url: string) => {
      flushNoteEditors();
      await flushDatabaseWrites();
      return companyAction("login", { url });
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  const select = useMutation({
    mutationFn: async (vault_id: string | null) => {
      flushNoteEditors();
      await flushDatabaseWrites();
      return companyAction("select", { vault_id });
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  const create = useMutation({
    mutationFn: (name: string) =>
      companyRequest<CompanyVault>("POST", "/v1/vaults", { name }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  const logout = useMutation({
    mutationFn: async () => {
      flushNoteEditors();
      await flushDatabaseWrites();
      return companyAction("logout");
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  const form = useForm({
    defaultValues: { url: status?.profile?.base_url ?? "", name: "" },
    onSubmit: ({ value }) => {
      login.mutate(value.url);
    },
  });
  const current = vaults.data?.find(
    (vault) => vault.id === status?.profile?.vault_id,
  );

  return (
    <section className="space-y-4 rounded-lg border p-4">
      <div>
        <h3 className="font-medium">
          <Trans>Company server</Trans>
        </h3>
        <p className="text-muted-foreground mt-1 text-xs">
          <Trans>
            Open your personal or shared vault on your company server. Content
            loads when you open it.
          </Trans>
        </p>
      </div>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
        className="flex items-end gap-2"
      >
        <form.Field name="url">
          {(field) => (
            <label className="flex flex-1 flex-col gap-1 text-sm">
              <Trans>Server address</Trans>
              <input
                className="bg-background rounded border px-3 py-2"
                type="url"
                value={field.state.value}
                onChange={(event) => field.handleChange(event.target.value)}
                placeholder="https://notes.example.com"
                required
              />
            </label>
          )}
        </form.Field>
        <Button
          type="submit"
          disabled={login.isPending || status?.recording || status?.processing}
        >
          {login.isPending
            ? "Complete sign-in in your browser…"
            : connected
              ? "Sign in again"
              : "Sign in"}
        </Button>
      </form>
      {connected && (
        <>
          <p className="text-muted-foreground text-xs">
            <Trans>Connected to </Trans>
            {status?.profile?.base_url}
          </p>
          <div className="space-y-2">
            <label className="flex flex-col gap-1 text-sm">
              <Trans>Vault location</Trans>
              <select
                className="bg-background rounded border px-3 py-2"
                value={status?.profile?.vault_id ?? ""}
                disabled={
                  select.isPending ||
                  status?.recording ||
                  status?.processing ||
                  vaults.isPending
                }
                onChange={(event) => select.mutate(event.target.value || null)}
              >
                <option value="">
                  <Trans>Local folder</Trans>
                </option>
                {vaults.data?.map((vault) => (
                  <option key={vault.id} value={vault.id}>
                    {vault.name}
                    {vault.personal ? " (personal)" : ""} ·{" "}
                    {vault.members[status?.profile?.user_id ?? ""]}
                  </option>
                ))}
              </select>
            </label>
            {(status?.recording || status?.processing) && (
              <p className="text-muted-foreground text-xs">
                <Trans>
                  Finish recording, transcription, and summaries before
                  switching vaults.
                </Trans>
              </p>
            )}
          </div>
          <form.Field name="name">
            {(field) => (
              <div className="flex items-end gap-2">
                <label className="flex flex-1 flex-col gap-1 text-sm">
                  <Trans>New shared vault</Trans>
                  <input
                    className="bg-background rounded border px-3 py-2"
                    value={field.state.value}
                    onChange={(event) => field.handleChange(event.target.value)}
                    maxLength={255}
                  />
                </label>
                <Button
                  variant="outline"
                  disabled={!field.state.value.trim() || create.isPending}
                  onClick={() => create.mutate(field.state.value)}
                >
                  <Trans>Create vault</Trans>
                </Button>
              </div>
            )}
          </form.Field>
          {current &&
            !current.personal &&
            current.members[status?.profile?.user_id ?? ""] === "owner" && (
              <VaultMembers vault={current} />
            )}
          <Button
            variant="outline"
            disabled={
              logout.isPending || status?.recording || status?.processing
            }
            onClick={() => logout.mutate()}
          >
            <Trans>Sign out</Trans>
          </Button>
        </>
      )}
      {[login.error, select.error, create.error, vaults.error, logout.error]
        .filter(Boolean)
        .map((error, index) => (
          <p key={index} role="alert" className="text-destructive text-sm">
            {error instanceof Error ? error.message : String(error)}
          </p>
        ))}
      {status?.active && <CompanyRecovery />}
    </section>
  );
}

function VaultMembers({ vault }: { vault: CompanyVault }) {
  const queryClient = useQueryClient();
  const users = useQuery({
    queryKey: ["company", "users"],
    queryFn: () => companyRequest<CompanyUser[]>("GET", "/v1/users"),
  });
  const update = useMutation({
    mutationFn: ({ id, role }: { id: string; role: CompanyRole | null }) =>
      companyRequest(
        role ? "PUT" : "DELETE",
        `/v1/vaults/${vault.id}/members/${id}`,
        role ? { role } : undefined,
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  const rename = useMutation({
    mutationFn: (name: string) =>
      companyRequest("PUT", `/v1/vaults/${vault.id}`, { name }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  const form = useForm({
    defaultValues: {
      member: "",
      role: "editor" as CompanyRole,
      name: vault.name,
    },
    onSubmit: ({ value }) =>
      update.mutate({ id: value.member, role: value.role }),
  });
  return (
    <div className="space-y-3 border-t pt-3">
      <h4 className="text-sm font-medium">
        <Trans>Manage shared vault</Trans>
      </h4>
      <form.Field name="name">
        {(field) => (
          <div className="flex gap-2">
            <input
              aria-label="Vault name"
              className="bg-background flex-1 rounded border px-3 py-2 text-sm"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
            <Button
              variant="outline"
              disabled={rename.isPending || !field.state.value.trim()}
              onClick={() => rename.mutate(field.state.value)}
            >
              <Trans>Rename</Trans>
            </Button>
          </div>
        )}
      </form.Field>
      <ul className="space-y-2">
        {Object.entries(vault.members).map(([id, role]) => (
          <li key={id} className="flex items-center gap-2 text-sm">
            <span className="flex-1">
              {users.data?.find((user) => user.id === id)?.name ||
                users.data?.find((user) => user.id === id)?.email ||
                "Company member"}
            </span>
            <select
              aria-label="Member role"
              className="bg-background rounded border px-2 py-1"
              value={role}
              disabled={update.isPending}
              onChange={(event) =>
                update.mutate({ id, role: event.target.value as CompanyRole })
              }
            >
              {["viewer", "editor", "owner"].map((role) => (
                <option key={role} value={role}>
                  {role}
                </option>
              ))}
            </select>
            <Button
              variant="ghost"
              disabled={update.isPending}
              onClick={() => update.mutate({ id, role: null })}
            >
              <Trans>Remove</Trans>
            </Button>
          </li>
        ))}
      </ul>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
        className="flex gap-2"
      >
        <form.Field name="member">
          {(field) => (
            <select
              aria-label="Company user"
              className="bg-background min-w-0 flex-1 rounded border px-2 py-1 text-sm"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
              required
            >
              <option value="">
                <Trans>Choose a company user</Trans>
              </option>
              {users.data
                ?.filter((user) => !vault.members[user.id])
                .map((user) => (
                  <option key={user.id} value={user.id}>
                    {user.name || user.email}
                  </option>
                ))}
            </select>
          )}
        </form.Field>
        <form.Field name="role">
          {(field) => (
            <select
              aria-label="New member role"
              className="bg-background rounded border px-2 py-1 text-sm"
              value={field.state.value}
              onChange={(event) =>
                field.handleChange(event.target.value as CompanyRole)
              }
            >
              {["viewer", "editor", "owner"].map((role) => (
                <option key={role} value={role}>
                  {role}
                </option>
              ))}
            </select>
          )}
        </form.Field>
        <Button type="submit" disabled={update.isPending}>
          <Trans>Add member</Trans>
        </Button>
      </form>
      <p className="text-muted-foreground text-xs">
        <Trans>Users appear after their first company sign-in.</Trans>
      </p>
      {(update.error || rename.error) && (
        <p role="alert" className="text-destructive text-sm">
          {String(update.error || rename.error)}
        </p>
      )}
    </div>
  );
}

export function CompanyRecovery() {
  const queryClient = useQueryClient();
  const { data: status } = useCompanyStatus();
  const retry = useMutation({
    mutationFn: () => companyAction("retry"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  if (!status?.pending.length && !status?.transfers.length && !status?.error)
    return null;
  return (
    <div className="space-y-3 border-t pt-3">
      <h4 className="text-sm font-medium">
        <Trans>Work waiting to be saved</Trans>
      </h4>
      {status?.error && (
        <p role="status" className="text-sm">
          {status.error.message}
        </p>
      )}
      <p className="text-muted-foreground text-xs">
        <Trans>
          Unsent edits and recordings stay on this device until the server
          confirms they are saved.
        </Trans>
      </p>
      <Button
        variant="outline"
        disabled={retry.isPending}
        onClick={() => retry.mutate()}
      >
        <Trans>Retry saving</Trans>
      </Button>
      {status?.transfers.map((transfer) => (
        <TransferRecovery key={transfer.id} transfer={transfer} />
      ))}
      {status?.pending.map((draft) => (
        <DraftRecovery key={draft.call.mutation_id} draft={draft} />
      ))}
      {retry.error && (
        <p role="alert" className="text-destructive text-sm">
          {String(retry.error)}
        </p>
      )}
    </div>
  );
}

function DraftRecovery({ draft }: { draft: PendingCompanyDraft }) {
  const queryClient = useQueryClient();
  const sessionId = String(draft.call.args.session_id ?? "");
  const preview = useQuery({
    queryKey: ["company", "preview", draft.call.mutation_id],
    queryFn: () =>
      companyAction<{
        data: {
          note_markdown?: string | null;
          markdown?: string | null;
        } | null;
        versions: Record<string, string | null>;
      }>("preview", { id: draft.call.mutation_id }),
    enabled: Boolean(sessionId && draft.error?.code === "conflict"),
    retry: false,
  });
  const resolve = useMutation({
    mutationFn: (
      action: "discard" | "replace" | "save_separate" | "export_draft",
    ) =>
      companyAction(
        action === "save_separate" || action === "export_draft"
          ? action
          : "resolve",
        {
          id: draft.call.mutation_id,
          action,
          expected: preview.data?.versions ?? {},
        },
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  const patch = draft.call.args.patch as { markdown?: string } | undefined;
  const doc = draft.call.args.doc as { markdown?: string } | undefined;
  const markdown = String(
    draft.call.args.markdown ??
      patch?.markdown ??
      doc?.markdown ??
      JSON.stringify(draft.call.args, null, 2),
  );
  return (
    <details className="rounded border p-3">
      <summary className="cursor-pointer text-sm">
        {draft.error?.message || "Unsaved edit"}
      </summary>
      <div className="mt-3 space-y-3">
        <label className="flex flex-col gap-1 text-xs">
          <Trans>Your unsaved text</Trans>
          <textarea
            className="bg-background min-h-24 rounded border p-2"
            readOnly
            value={markdown}
          />
        </label>
        {preview.data && (
          <label className="flex flex-col gap-1 text-xs">
            <Trans>Saved on the server</Trans>
            <textarea
              className="bg-background min-h-24 rounded border p-2"
              readOnly
              value={
                preview.data.data?.note_markdown ??
                preview.data.data?.markdown ??
                ""
              }
            />
          </label>
        )}
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            disabled={resolve.isPending}
            onClick={() => resolve.mutate("save_separate")}
          >
            <Trans>Save as a separate note</Trans>
          </Button>
          <Button
            variant="outline"
            disabled={resolve.isPending}
            onClick={() => resolve.mutate("export_draft")}
          >
            <Trans>Export draft</Trans>
          </Button>
          <Button
            variant="outline"
            disabled={resolve.isPending}
            onClick={() => resolve.mutate("discard")}
          >
            <Trans>Discard draft and reload</Trans>
          </Button>
          {markdown && preview.data?.data && (
            <Button
              disabled={resolve.isPending || !preview.data?.data}
              onClick={() => resolve.mutate("replace")}
            >
              <Trans>Replace the displayed server version</Trans>
            </Button>
          )}
        </div>
        {resolve.error && (
          <p role="alert" className="text-destructive text-sm">
            {String(resolve.error)}
          </p>
        )}
      </div>
    </details>
  );
}

function TransferRecovery({
  transfer,
}: {
  transfer: { id: string; filename: string; error: { message: string } | null };
}) {
  const queryClient = useQueryClient();
  const action = useMutation({
    mutationFn: (operation: "export_transfer" | "discard_transfer") =>
      companyAction(operation, { id: transfer.id }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["company"] }),
  });
  return (
    <details className="rounded border p-3">
      <summary className="cursor-pointer text-sm">
        {transfer.filename}: {transfer.error?.message || "Waiting to upload"}
      </summary>
      <div className="mt-3 flex gap-2">
        <Button
          variant="outline"
          disabled={action.isPending}
          onClick={() => action.mutate("export_transfer")}
        >
          <Trans>Export file</Trans>
        </Button>
        <Button
          variant="outline"
          disabled={action.isPending}
          onClick={() => action.mutate("discard_transfer")}
        >
          <Trans>Discard upload</Trans>
        </Button>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive text-sm">
          {String(action.error)}
        </p>
      )}
    </details>
  );
}
