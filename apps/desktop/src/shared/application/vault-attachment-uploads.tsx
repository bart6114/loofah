import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { useSyncExternalStore, type RefObject } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { Spinner } from "@hypr/ui/components/ui/spinner";

import { deviceFileName } from "./device-attachment-source";
import {
  useVaultFileUpload,
  useVaultFileUploadStates,
} from "./use-vault-file-upload";
import type { VaultScope } from "./vault-scope";

import { useNativeFileDrop } from "~/shared/hooks/useNativeFileDrop";

export function VaultAttachmentUploads({
  scope,
  sessionId,
  targetRef,
}: {
  scope: VaultScope;
  sessionId: string;
  targetRef: RefObject<HTMLElement | null>;
}) {
  const { t } = useLingui();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const allowed = scope.client.canEdit && connection.state === "connected";
  const upload = useVaultFileUpload(scope, sessionId);
  const { states, remove } = useVaultFileUploadStates(scope, sessionId);
  const add = (paths: string[]) => {
    for (const path of paths)
      void upload({
        kind: "path",
        path,
        name: deviceFileName(path),
      }).catch(() => {});
  };
  useNativeFileDrop(targetRef, { enabled: allowed, onDrop: add });
  const picker = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: () =>
        open({ multiple: true, directory: false, title: t`Add attachments` }),
      onSuccess: (paths) => {
        if (paths) add(Array.isArray(paths) ? paths : [paths]);
      },
    },
    scope.queryClient,
  );
  return (
    <div className="flex flex-col gap-2">
      <Button
        size="sm"
        variant="outline"
        disabled={!allowed || picker.isPending}
        onClick={() => picker.mutate()}
      >
        {picker.isPending ? t`Choosing…` : t`Add attachments`}
      </Button>
      {picker.isError && (
        <p role="alert">
          <Trans>Couldn’t choose attachments</Trans>
        </p>
      )}
      {states.map((attempt) => (
        <div
          key={attempt.clientId}
          className="flex flex-wrap items-center gap-2 text-sm"
        >
          <span className="min-w-0 flex-1 truncate">
            {attempt.candidate.name}
          </span>
          {attempt.status === "pending" ? (
            <span role="status">
              <Spinner className="size-3" />
              <span className="sr-only">
                <Trans>Loading…</Trans>
              </span>
            </span>
          ) : (
            <>
              <span role="alert" className="w-full text-xs">
                {attempt.error instanceof Error
                  ? attempt.error.message
                  : t`Couldn’t add 1 attachment`}
              </span>
              <Button
                size="sm"
                variant="outline"
                disabled={!allowed}
                aria-label={t`Retry ${attempt.candidate.name}`}
                onClick={() =>
                  void upload(attempt.candidate, attempt.clientId).catch(
                    () => {},
                  )
                }
              >
                <Trans>Retry</Trans>
              </Button>
            </>
          )}
          <Button
            size="sm"
            variant="ghost"
            aria-label={t`Remove ${attempt.candidate.name}`}
            onClick={() => remove(attempt.clientId)}
          >
            <Trans>Remove</Trans>
          </Button>
        </div>
      ))}
    </div>
  );
}
