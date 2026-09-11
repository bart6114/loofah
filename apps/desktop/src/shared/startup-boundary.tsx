import { useLingui } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { join } from "@tauri-apps/api/path";
import { open as selectFolder } from "@tauri-apps/plugin-dialog";
import { AlertCircleIcon, FolderOpenIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { commands as settingsCommands } from "@hypr/plugin-settings";
import { Button } from "@hypr/ui/components/ui/button";
import { Spinner } from "@hypr/ui/components/ui/spinner";

import { relaunchNow } from "./relaunch";

import {
  commands,
  events,
  type StartupPhase,
  type StartupStatus,
} from "~/types/tauri.gen";

const CLOUD_STORAGE_HINT_DELAY_MS = 5000;
const NEW_VAULT_FOLDER_NAME = "Loofah";

export function StartupBoundary({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<StartupStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const apply = (next: StartupStatus) => {
      if (cancelled) return;
      setStatus((current) =>
        current && current.revision > next.revision ? current : next,
      );
    };

    void events.startupProgress
      .listen(({ payload }) => apply(payload.status))
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((error) => {
        console.error("Failed to listen for startup progress", error);
      });

    void commands
      .getStartupStatus()
      .then(apply)
      .catch((error) => {
        apply({
          revision: Number.MAX_SAFE_INTEGER,
          vaultPath: "",
          isCloudStorage: false,
          phase: { kind: "failed", message: String(error) },
        });
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (status?.phase.kind === "ready") {
    return children;
  }

  return <StartupScreen status={status} />;
}

function StartupScreen({ status }: { status: StartupStatus | null }) {
  const { t } = useLingui();
  const [showCloudHint, setShowCloudHint] = useState(false);
  const phase = status?.phase ?? ({ kind: "openingVault" } as const);
  const failed = phase.kind === "failed";

  useEffect(() => {
    const timeout = setTimeout(
      () => setShowCloudHint(true),
      CLOUD_STORAGE_HINT_DELAY_MS,
    );
    return () => clearTimeout(timeout);
  }, []);

  const retry = useMutation({ mutationFn: relaunchNow });
  const switchVault = useMutation({
    mutationFn: async () => {
      const selected = await selectFolder({
        title: t`Choose storage location`,
        directory: true,
        multiple: false,
        defaultPath: status?.vaultPath || undefined,
      });
      if (!selected) return;

      const classification = await settingsCommands.classifyVaultDir(selected);
      if (classification.status === "error") {
        throw new Error(classification.error);
      }
      const target =
        classification.data === "other"
          ? await join(selected, NEW_VAULT_FOLDER_NAME)
          : selected;
      const result = await settingsCommands.setVaultBase(target);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      await relaunchNow();
    },
  });

  const showRecovery = failed || showCloudHint;
  const error = retry.error ?? switchVault.error;

  return (
    <div
      data-tauri-drag-region
      className="bg-background text-foreground flex h-screen w-screen items-center justify-center px-8"
    >
      <main className="flex w-full max-w-md flex-col items-center text-center">
        {failed ? (
          <div className="bg-destructive/10 text-destructive flex size-12 items-center justify-center rounded-full">
            <AlertCircleIcon className="size-6" />
          </div>
        ) : (
          <Spinner size={32} className="text-primary" aria-label={t`Loading`} />
        )}

        <h1 className="mt-6 text-xl font-semibold tracking-tight">
          {failed ? t`We couldn't open your vault` : t`Opening your vault…`}
        </h1>
        <p className="text-muted-foreground mt-2 text-sm">
          {phaseDescription(t, phase)}
        </p>

        {status?.vaultPath ? (
          <p className="text-muted-foreground/80 mt-3 max-w-full truncate font-mono text-xs">
            {status.vaultPath}
          </p>
        ) : null}

        {showCloudHint && status?.isCloudStorage && !failed ? (
          <div className="border-border bg-muted/50 mt-6 w-full rounded-lg border p-4 text-left">
            <p className="text-sm font-medium">
              {cloudProviderName(status.vaultPath) === "Google Drive"
                ? t`Google Drive may still be downloading files`
                : t`Your cloud provider may still be downloading files`}
            </p>
            <p className="text-muted-foreground mt-1 text-xs">
              {t`Loofah will continue automatically when the vault is available.`}
            </p>
          </div>
        ) : null}

        {showRecovery ? (
          <div className="mt-6 flex items-center gap-2">
            {failed ? (
              <Button
                variant="outline"
                onClick={() => retry.mutate()}
                disabled={retry.isPending || switchVault.isPending}
              >
                {retry.isPending ? t`Restarting…` : t`Try again`}
              </Button>
            ) : null}
            <Button
              variant={failed ? "default" : "outline"}
              onClick={() => switchVault.mutate()}
              disabled={retry.isPending || switchVault.isPending}
            >
              {switchVault.isPending ? (
                <Spinner size={14} className="mr-2" />
              ) : (
                <FolderOpenIcon className="mr-2 size-4" />
              )}
              {switchVault.isPending ? t`Switching…` : t`Switch vault`}
            </Button>
          </div>
        ) : null}

        {error ? (
          <p className="text-destructive mt-4 text-sm">{error.message}</p>
        ) : null}
      </main>
    </div>
  );
}

function phaseDescription(
  t: ReturnType<typeof useLingui>["t"],
  phase: StartupPhase,
) {
  switch (phase.kind) {
    case "openingVault":
      return t`Preparing your workspace`;
    case "scanning":
      return phase.sessions_found > 0
        ? t`Scanning notes — ${phase.sessions_found} found`
        : t`Scanning notes…`;
    case "indexing":
      return phase.total > 0
        ? t`Indexing notes — ${phase.completed} of ${phase.total}`
        : t`Indexing notes…`;
    case "archivingLegacyTemplates":
      return t`Archiving legacy templates…`;
    case "failed":
      return phase.message;
    case "ready":
      return t`Ready`;
  }
}

function cloudProviderName(path: string) {
  if (path.includes("GoogleDrive")) return "Google Drive";
  return null;
}
