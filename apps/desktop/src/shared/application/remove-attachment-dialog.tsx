import { Trans, useLingui } from "@lingui/react/macro";
import {
  useMutation,
  useQuery,
  type UseMutationResult,
} from "@tanstack/react-query";
import { useState, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { ApplicationRequestError } from "./errors";
import type { FileMetadata } from "./protocol.gen";
import type { VaultScope } from "./vault-scope";

import { formatFileSize } from "~/shared/format-file-size";

export function RemoveAttachmentDialog({
  scope,
  sessionId,
  attachmentId,
  onClose,
  onRemoved,
}: {
  scope: VaultScope;
  sessionId: string;
  attachmentId: string;
  onClose: () => void;
  onRemoved: () => void;
}) {
  const { t } = useLingui();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const allowed = connection.state === "connected" && scope.client.canEdit;
  const query = useQuery(
    {
      ...scope.attachmentMetadataOptions(sessionId, attachmentId),
      enabled: allowed,
      staleTime: 0,
      gcTime: 0,
    },
    scope.queryClient,
  );
  const mutation = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: (input: { expectedSha256: string; requestId: string }) =>
        scope.run(() =>
          scope.client.removeAttachment(
            sessionId,
            attachmentId,
            { expected_sha256: input.expectedSha256 },
            { requestId: input.requestId },
          ),
        ),
      onSuccess: () => {
        void scope.queryClient.invalidateQueries({
          queryKey: ["vault", scope.client.scope, "attachment_list", sessionId],
        });
        void scope.queryClient.invalidateQueries({
          queryKey: [
            "vault",
            scope.client.scope,
            "attachment_metadata",
            sessionId,
            attachmentId,
          ],
        });
        onRemoved();
      },
    },
    scope.queryClient,
  );
  const uncertain =
    mutation.error &&
    (!(mutation.error instanceof ApplicationRequestError) ||
      mutation.error.detail.mutation_outcome !== "not_committed");
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !mutation.isPending) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            <Trans>Remove attachment?</Trans>
          </DialogTitle>
          <DialogDescription>{t`“${attachmentId}” will be moved to the vault Trash. Links to this file in notes or summaries may stop working.`}</DialogDescription>
        </DialogHeader>
        {mutation.isError && (
          <p role="alert">
            {uncertain ? (
              <Trans>
                Removal could not be confirmed. Check the attachment and vault
                Trash before trying again.
              </Trans>
            ) : (
              <Trans>
                The attachment was not removed. Close this dialog and review it
                again.
              </Trans>
            )}
          </p>
        )}

        {!allowed ? (
          <>
            <p role="alert">
              <Trans>
                Attachment access is unavailable. Reconnect or sign in again.
              </Trans>
            </p>
            <Button
              variant="outline"
              disabled={mutation.isPending}
              onClick={onClose}
            >
              <Trans>Close</Trans>
            </Button>
          </>
        ) : query.data ? (
          <>
            {query.isError && (
              <p role="alert">
                <Trans>
                  Couldn’t check the attachment. Close this dialog and review it
                  again.
                </Trans>
              </p>
            )}
            <ReviewedRemoval
              initial={query.data}
              current={query.data}
              fetching={query.isFetching || query.isError}
              onClose={onClose}
              mutation={mutation}
            />
          </>
        ) : (
          <>
            {query.isPending ? (
              <p role="status">
                <Trans>Checking attachment…</Trans>
              </p>
            ) : query.isError ? (
              <div role="alert">
                <p>
                  <Trans>Couldn’t check the attachment.</Trans>
                </p>
                <Button
                  variant="outline"
                  disabled={query.isFetching}
                  onClick={() => void query.refetch()}
                >
                  <Trans>Try again</Trans>
                </Button>
              </div>
            ) : (
              <p>
                <Trans>This attachment is no longer available.</Trans>
              </p>
            )}
            <Button
              variant="outline"
              disabled={mutation.isPending}
              onClick={onClose}
            >
              <Trans>Close</Trans>
            </Button>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function ReviewedRemoval({
  initial,
  current,
  fetching,
  onClose,
  mutation,
}: {
  initial: FileMetadata;
  current: FileMetadata;
  fetching: boolean;
  onClose: () => void;
  mutation: UseMutationResult<
    boolean,
    Error,
    { expectedSha256: string; requestId: string }
  >;
}) {
  // A refresh must not silently change which bytes the confirmation applies to.
  const [reviewed] = useState(initial);
  const changed = current.sha256 !== reviewed.sha256;
  return (
    <>
      <p className="text-muted-foreground text-sm">
        {formatFileSize(reviewed.sizeBytes)} · {reviewed.contentType}
      </p>
      {changed && (
        <p role="alert">
          <Trans>
            The attachment changed. Close this dialog and review it again.
          </Trans>
        </p>
      )}
      <DialogFooter>
        <Button
          variant="outline"
          disabled={mutation.isPending}
          onClick={onClose}
        >
          <Trans>Cancel</Trans>
        </Button>
        <Button
          variant="destructive"
          disabled={
            mutation.isPending ||
            mutation.isError ||
            mutation.isSuccess ||
            changed ||
            fetching
          }
          onClick={() =>
            mutation.mutate({
              expectedSha256: reviewed.sha256,
              requestId: crypto.randomUUID(),
            })
          }
        >
          {mutation.isPending ? (
            <Trans>Removing…</Trans>
          ) : (
            <Trans>Move to Trash</Trans>
          )}
        </Button>
      </DialogFooter>
    </>
  );
}
