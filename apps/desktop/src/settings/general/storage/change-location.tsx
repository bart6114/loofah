import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { homeDir } from "@tauri-apps/api/path";
import { open as selectFolder } from "@tauri-apps/plugin-dialog";
import { FolderIcon } from "lucide-react";
import { useState } from "react";

import { commands as openerCommands } from "@hypr/plugin-opener2";
import { commands as settingsCommands } from "@hypr/plugin-settings";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn } from "@hypr/utils";

import { displayPath } from "./path-utils";

import { scheduleAutomaticRelaunch } from "~/shared/relaunch";
import { commands as tauriCommands } from "~/types/tauri.gen";

const VAULT_BASE_QUERY_KEY = ["vault-base-path"] as const;

const NEW_VAULT_FOLDER_NAME = "Loofah";

type VaultAction =
  | { kind: "move"; path: string }
  | { kind: "copy"; path: string }
  | { kind: "switch"; path: string };

type EmptyFolderChoice = "move" | "copy" | "fresh";

export function ChangeLocationRow() {
  const { t } = useLingui();
  const queryClient = useQueryClient();
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [emptyChoice, setEmptyChoice] = useState<EmptyFolderChoice>("move");

  const { data: home } = useQuery({ queryKey: ["home-dir"], queryFn: homeDir });

  const { data: vaultBase } = useQuery({
    queryKey: VAULT_BASE_QUERY_KEY,
    queryFn: async () => {
      const result = await settingsCommands.vaultBase();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
  });

  const { data: destinationKind } = useQuery({
    queryKey: ["vault-destination-kind", pendingPath],
    enabled: pendingPath !== null,
    queryFn: async () => {
      const result = await settingsCommands.classifyVaultDir(pendingPath!);
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });

  const changeMutation = useMutation({
    mutationFn: async (action: VaultAction) => {
      const result =
        action.kind === "switch"
          ? await settingsCommands.setVaultBase(action.path)
          : await tauriCommands.relocateVault(
              action.path,
              action.kind === "copy",
            );
      if (result.status === "error") {
        throw new Error(result.error);
      }
    },
    onSuccess: async () => {
      setPendingPath(null);
      await queryClient.invalidateQueries({ queryKey: VAULT_BASE_QUERY_KEY });
      await scheduleAutomaticRelaunch();
    },
    onError: (error) => {
      sonnerToast.error(error.message);
    },
  });

  const openPickerForPath = (path: string) => {
    changeMutation.reset();
    setEmptyChoice("move");
    setPendingPath(path);
  };

  const handleChange = async () => {
    const selected = await selectFolder({
      title: t`Choose storage location`,
      directory: true,
      multiple: false,
      defaultPath: vaultBase ?? undefined,
    });

    if (selected && selected !== vaultBase) {
      openPickerForPath(selected);
    }
  };

  const handleOpenPath = () => {
    if (vaultBase) {
      openerCommands.openPath(vaultBase, null);
    }
  };

  const closeDialog = () => {
    if (changeMutation.isPending) return;
    setPendingPath(null);
  };

  const pending = changeMutation.isPending;
  const pendingKind = changeMutation.variables?.kind;

  // In a folder with unrelated files, every action targets a fresh subfolder
  // instead -- the vault always ends up in a directory of its own.
  const choiceTarget =
    pendingPath === null
      ? null
      : destinationKind === "other"
        ? `${pendingPath}/${NEW_VAULT_FOLDER_NAME}`
        : pendingPath;

  return (
    <>
      <div className="flex flex-col gap-3">
        <div className="grid grid-cols-[minmax(0,1fr)_9rem] items-center gap-3">
          <div className="border-border bg-muted flex min-w-0 items-center gap-3 rounded-lg border px-4 py-3">
            <FolderIcon className="text-muted-foreground size-4 shrink-0" />
            <button
              onClick={handleOpenPath}
              className="text-muted-foreground min-w-0 flex-1 truncate text-left text-sm hover:underline"
            >
              {displayPath(vaultBase, home)}
            </button>
          </div>
          <Button
            variant="outline"
            className="h-9 w-full justify-center"
            onClick={handleChange}
            disabled={pending}
          >
            <Trans>Change</Trans>
          </Button>
        </div>
      </div>

      <Dialog
        open={pendingPath !== null}
        onOpenChange={(open) => {
          if (!open) closeDialog();
        }}
      >
        {pendingPath !== null && (
          <DialogContent>
            {destinationKind === undefined && (
              <DialogHeader>
                <DialogTitle>
                  <Trans>Change storage location?</Trans>
                </DialogTitle>
                <DialogDescription className="truncate">
                  {displayPath(pendingPath, home)}
                </DialogDescription>
              </DialogHeader>
            )}

            {(destinationKind === "empty_or_missing" ||
              destinationKind === "other") && (
              <>
                {destinationKind === "empty_or_missing" ? (
                  <DialogHeader>
                    <DialogTitle>
                      <Trans>Use this folder for your vault?</Trans>
                    </DialogTitle>
                    <DialogDescription className="truncate">
                      {displayPath(pendingPath, home)}
                    </DialogDescription>
                  </DialogHeader>
                ) : (
                  <DialogHeader>
                    <DialogTitle>
                      <Trans>This folder already has files in it</Trans>
                    </DialogTitle>
                    <DialogDescription>
                      <Trans>
                        Your vault lives directly in the folder you pick, so
                        we'll create one inside:
                      </Trans>
                    </DialogDescription>
                  </DialogHeader>
                )}

                {destinationKind === "other" && (
                  <p className="text-muted-foreground truncate text-sm">
                    {displayPath(pendingPath, home)}/{NEW_VAULT_FOLDER_NAME}
                  </p>
                )}

                <div role="radiogroup" className="flex flex-col gap-2">
                  <ChoiceRow
                    selected={emptyChoice === "move"}
                    disabled={pending}
                    onSelect={() => setEmptyChoice("move")}
                    label={<Trans>Move my vault here</Trans>}
                    detail={
                      <Trans>
                        Everything relocates; the old location is cleaned up.
                      </Trans>
                    }
                  />
                  <ChoiceRow
                    selected={emptyChoice === "copy"}
                    disabled={pending}
                    onSelect={() => setEmptyChoice("copy")}
                    label={<Trans>Copy my vault here</Trans>}
                    detail={
                      <Trans>
                        A duplicate; the original stays where it is.
                      </Trans>
                    }
                  />
                  <ChoiceRow
                    selected={emptyChoice === "fresh"}
                    disabled={pending}
                    onSelect={() => setEmptyChoice("fresh")}
                    label={<Trans>Start a new empty vault here</Trans>}
                    detail={
                      <Trans>
                        Your current vault stays untouched — switch back
                        anytime.
                      </Trans>
                    }
                  />
                </div>

                <RestartNote />
                <MutationError error={changeMutation.error} />

                <DialogFooter>
                  <CancelButton onClick={closeDialog} disabled={pending} />
                  <Button
                    onClick={() =>
                      changeMutation.mutate(
                        emptyChoice === "fresh"
                          ? { kind: "switch", path: choiceTarget! }
                          : { kind: emptyChoice, path: choiceTarget! },
                      )
                    }
                    disabled={pending}
                  >
                    {pending
                      ? pendingLabel(t, pendingKind)
                      : emptyChoice === "move"
                        ? t`Move my vault`
                        : emptyChoice === "copy"
                          ? t`Copy my vault`
                          : t`Start new vault`}
                  </Button>
                </DialogFooter>
              </>
            )}

            {destinationKind === "vault" && (
              <>
                <DialogHeader>
                  <DialogTitle>
                    <Trans>Switch to this vault?</Trans>
                  </DialogTitle>
                  <DialogDescription className="truncate">
                    {displayPath(pendingPath, home)}
                  </DialogDescription>
                </DialogHeader>

                <p className="text-muted-foreground text-sm">
                  <Trans>
                    Your current vault stays where it is — nothing is moved or
                    deleted. You can switch back the same way.
                  </Trans>
                </p>

                <RestartNote />
                <MutationError error={changeMutation.error} />

                <DialogFooter>
                  <CancelButton onClick={closeDialog} disabled={pending} />
                  <Button
                    onClick={() =>
                      changeMutation.mutate({
                        kind: "switch",
                        path: pendingPath,
                      })
                    }
                    disabled={pending}
                  >
                    {pending ? t`Switching...` : t`Switch`}
                  </Button>
                </DialogFooter>
              </>
            )}
          </DialogContent>
        )}
      </Dialog>
    </>
  );
}

function ChoiceRow({
  selected,
  disabled,
  onSelect,
  label,
  detail,
}: {
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
  label: React.ReactNode;
  detail: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      onClick={onSelect}
      className={cn([
        "rounded-lg border px-3 py-2 text-left",
        selected ? "border-brand bg-brand/5" : "border-border hover:bg-muted",
      ])}
    >
      <span className="text-foreground block text-sm font-medium">{label}</span>
      <span className="text-muted-foreground block text-xs">{detail}</span>
    </button>
  );
}

function RestartNote() {
  return (
    <p className="text-muted-foreground text-xs">
      <Trans>The app restarts to apply.</Trans>
    </p>
  );
}

function MutationError({ error }: { error: Error | null }) {
  if (!error) return null;
  return <p className="text-destructive text-sm">{error.message}</p>;
}

function CancelButton({
  onClick,
  disabled,
}: {
  onClick: () => void;
  disabled: boolean;
}) {
  return (
    <Button variant="ghost" onClick={onClick} disabled={disabled}>
      <Trans>Cancel</Trans>
    </Button>
  );
}

function pendingLabel(
  t: ReturnType<typeof useLingui>["t"],
  kind: VaultAction["kind"] | undefined,
) {
  if (kind === "copy") return t`Copying...`;
  if (kind === "switch") return t`Switching...`;
  return t`Moving...`;
}
