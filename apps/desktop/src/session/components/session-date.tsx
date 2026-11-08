import { Trans, useLingui } from "@lingui/react/macro";
import { useForm, useStore } from "@tanstack/react-form";
import { onlineManager, useMutation, useQuery } from "@tanstack/react-query";
import { CheckIcon, XIcon } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { Input } from "@hypr/ui/components/ui/input";
import { format, safeFormat, safeParseDate } from "@hypr/utils";

import { connectionClosed } from "~/shared/application/errors";
import type { MetadataSnapshot } from "~/shared/application/protocol.gen";
import { useVaultHost } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

const terminal = new Set([
  "access_denied",
  "login_required",
  "closed",
  "incompatible",
]);

export function SessionDate({ sessionId }: { sessionId: string }) {
  const { scope } = useVaultHost();
  const [host, setHost] = useState(scope);
  useEffect(() => setHost(scope), [scope]);
  return scope && host === scope ? (
    <ScopedSessionDate key={sessionId} scope={scope} sessionId={sessionId} />
  ) : null;
}

function ScopedSessionDate({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const { t } = useLingui();
  const [editing, setEditing] = useState(false);
  const owner = useRef({
    scope,
    sessionId,
    mounted: false,
    retired: terminal.has(scope.getSnapshot().state),
  });
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const online = useSyncExternalStore(
    (changed) => onlineManager.subscribe(changed),
    () => onlineManager.isOnline(),
  );
  useEffect(() => {
    const captured = owner.current;
    captured.mounted = true;
    const stop = scope.subscribe(() => {
      if (terminal.has(scope.getSnapshot().state)) captured.retired = true;
    });
    return () => {
      captured.mounted = false;
      stop();
    };
  }, [scope]);
  const current = () =>
    owner.current.mounted &&
    !owner.current.retired &&
    owner.current.scope === scope &&
    owner.current.sessionId === sessionId;
  const available = () =>
    current() &&
    scope.getSnapshot().state === "connected" &&
    scope.client.canEdit &&
    scope.client.features.includes("metadata_edit") &&
    (scope.client.networkMode === "always" || onlineManager.isOnline());
  const readable = () =>
    current() &&
    scope.getSnapshot().state === "connected" &&
    (scope.client.networkMode === "always" || onlineManager.isOnline());
  const options = scope.metadataOptions(sessionId);
  const metadata = useQuery(
    {
      ...options,
      enabled:
        Boolean(sessionId) &&
        connection.state === "connected" &&
        !owner.current.retired,
      notifyOnChangeProps: "all",
    },
    scope.queryClient,
  );
  const readFresh = async (): Promise<MetadataSnapshot> => {
    if (!available()) throw connectionClosed();
    await scope.queryClient.cancelQueries({
      queryKey: options.queryKey,
      exact: true,
    });
    if (!available()) throw connectionClosed();
    const snapshot = await scope.queryClient.fetchQuery({
      ...options,
      staleTime: 0,
      retry: false,
    });
    if (!available()) throw connectionClosed();
    return scope.queryClient.getQueryData(options.queryKey) ?? snapshot;
  };
  const write = useMutation(
    {
      networkMode: "always",
      retry: false,
      mutationFn: async (input: { createdAt: string; revision: string }) => {
        if (!available()) throw connectionClosed();
        const outcome = await scope.run(() =>
          scope.client.saveMetadata(sessionId, {
            patch: { created_at: input.createdAt },
            expected_revision: input.revision,
          }),
        );
        // A delayed write reply may predate a newer date; only a canonical read confirms the label.
        const readCount =
          scope.queryClient.getQueryState(options.queryKey)?.dataUpdateCount ??
          0;
        if (available()) await readFresh().catch(() => {});
        return { outcome, readCount };
      },
      onError: () => {
        if (available())
          void scope.queryClient.invalidateQueries({
            queryKey: options.queryKey,
            exact: true,
          });
      },
    },
    scope.queryClient,
  );
  const review = useMutation(
    { networkMode: "always", retry: false, mutationFn: readFresh },
    scope.queryClient,
  );
  const form = useForm({
    defaultValues: { createdAt: "", revision: "", baselineCreatedAt: "" },
    validators: {
      onChange: ({ value }) =>
        !value.createdAt.trim()
          ? { fields: { createdAt: t`Date and time are required` } }
          : !toIsoString(value.createdAt)
            ? { fields: { createdAt: t`Enter a valid date and time` } }
            : undefined,
    },
    onSubmit: async ({ value }): Promise<void> => {
      if (!available() || write.isPending || review.isPending || needsReview)
        return;
      const createdAt = toIsoString(value.createdAt);
      if (!createdAt) return;
      if (value.createdAt === toDatetimeLocalValue(value.baselineCreatedAt)) {
        setEditing(false);
        return;
      }
      setEditing(false);
      await write
        .mutateAsync({ createdAt, revision: value.revision })
        .catch(() => {});
    },
  });
  const values = useStore(form.store, (state) => state.values);
  const submitting = useStore(form.store, (state) => state.isSubmitting);
  const conflict = write.data?.outcome.kind === "conflict";
  const awaitingCanonical =
    write.data?.outcome.kind === "saved" &&
    (scope.queryClient.getQueryState(options.queryKey)?.dataUpdateCount ?? 0) <=
      write.data.readCount;
  const stale =
    editing && metadata.data && values.revision !== metadata.data.revision;
  const needsReview = Boolean(
    write.error || conflict || awaitingCanonical || stale || metadata.error,
  );
  const showEditor =
    editing ||
    Boolean(write.error) ||
    conflict ||
    (awaitingCanonical && metadata.isError);
  const disabled =
    owner.current.retired ||
    connection.state !== "connected" ||
    !scope.client.canEdit ||
    !scope.client.features.includes("metadata_edit") ||
    (!online && scope.client.networkMode !== "always") ||
    write.isPending ||
    review.isPending ||
    submitting;
  const cancel = () => {
    if (write.isPending || review.isPending || form.state.isSubmitting) return;
    write.reset();
    review.reset();
    form.reset();
    setEditing(false);
  };
  const submit = () => {
    if (disabled || needsReview || form.state.isSubmitting) return;
    void form.handleSubmit().catch(() => {});
  };
  const commitOnBlur = () => {
    if (disabled || needsReview || form.state.isSubmitting) return;
    if (
      toIsoString(form.state.values.createdAt) &&
      form.state.values.createdAt !==
        toDatetimeLocalValue(form.state.values.baselineCreatedAt)
    )
      submit();
    else cancel();
  };
  const reviewSaved = () => {
    if (!available() || review.isPending || write.isPending) return;
    void review
      .mutateAsync()
      .then((snapshot) => {
        if (!available()) return;
        const picked = toIsoString(form.state.values.createdAt);
        form.setFieldValue("revision", snapshot.revision);
        form.setFieldValue("baselineCreatedAt", snapshot.meta.created_at);
        write.reset();
        setEditing(
          picked !== safeParseDate(snapshot.meta.created_at)?.toISOString(),
        );
      })
      .catch(() => {});
  };
  if (owner.current.retired || terminal.has(connection.state)) return null;
  const picked = write.variables?.createdAt ?? toIsoString(values.createdAt);
  const displayDate =
    write.isPending || submitting || awaitingCanonical
      ? picked
      : metadata.data?.meta.created_at;
  return (
    <div className="flex w-fit flex-col gap-1">
      {showEditor ? (
        <>
          <form.Field name="createdAt">
            {(field) => (
              <div
                className="flex h-6 items-center gap-0"
                onBlur={(event) => {
                  if (!event.currentTarget.contains(event.relatedTarget))
                    commitOnBlur();
                }}
              >
                <Input
                  autoFocus
                  type="datetime-local"
                  aria-label={t`Note date and time`}
                  className="h-6 w-fit border-0 px-0 py-0 text-xs shadow-none focus-visible:ring-0"
                  value={field.state.value}
                  readOnly={disabled}
                  onChange={(event) => field.handleChange(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      submit();
                    }
                    if (event.key === "Escape") {
                      event.preventDefault();
                      cancel();
                    }
                  }}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive size-6 shrink-0 rounded-full"
                  disabled={write.isPending || review.isPending || submitting}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={cancel}
                  aria-label={t`Cancel date edit`}
                >
                  <XIcon size={14} />
                </Button>
                <form.Subscribe selector={(state) => state.canSubmit}>
                  {(canSubmit) => (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="text-muted-foreground hover:bg-brand/10 hover:text-brand size-6 shrink-0 rounded-full"
                      onMouseDown={(event) => event.preventDefault()}
                      onClick={submit}
                      disabled={!canSubmit || disabled || needsReview}
                      aria-label={t`Save date`}
                    >
                      <CheckIcon size={14} />
                    </Button>
                  )}
                </form.Subscribe>
              </div>
            )}
          </form.Field>
          <form.Field name="createdAt">
            {(field) =>
              field.state.meta.errors[0] ? (
                <p className="text-destructive text-xs">
                  {field.state.meta.errors[0]}
                </p>
              ) : null
            }
          </form.Field>
          {needsReview && (
            <div className="flex flex-col gap-1 text-xs">
              <p role="alert">
                {conflict || stale
                  ? t`The saved date changed. Review it before saving your picked date.`
                  : awaitingCanonical
                    ? t`The date was saved, but its current value could not be confirmed.`
                    : t`Could not confirm the date change. Review the saved date before trying again.`}
              </p>
              {!metadata.isFetching && !metadata.error && metadata.data && (
                <p>
                  <Trans>Current saved date:</Trans>{" "}
                  {safeFormat(
                    metadata.data.meta.created_at,
                    "MMM d, yyyy h:mm a",
                    t`Unknown date`,
                  )}
                </p>
              )}
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={disabled}
                onMouseDown={(event) => event.preventDefault()}
                onClick={reviewSaved}
              >
                <Trans>Review saved date</Trans>
              </Button>
            </div>
          )}
          {review.error && (
            <p role="alert" className="text-destructive text-xs">
              {review.error.message}
            </p>
          )}
        </>
      ) : (
        <>
          <button
            type="button"
            aria-label={t`Edit date`}
            title={t`Edit date`}
            disabled={
              disabled ||
              awaitingCanonical ||
              !metadata.data ||
              Boolean(metadata.error)
            }
            onClick={() => {
              if (!available() || !metadata.data) return;
              write.reset();
              review.reset();
              // Keep the rendered defaults stable so a rerender cannot reset the captured revision fields.
              form.reset(
                {
                  createdAt: toDatetimeLocalValue(
                    metadata.data.meta.created_at,
                  ),
                  revision: metadata.data.revision,
                  baselineCreatedAt: metadata.data.meta.created_at,
                },
                { keepDefaultValues: true },
              );
              setEditing(true);
            }}
            className="text-muted-foreground hover:text-foreground flex h-6 w-fit items-center text-xs transition-none"
          >
            {safeFormat(displayDate, "MMM d, yyyy h:mm a", t`Unknown date`)}
          </button>
          {(write.isPending || awaitingCanonical) && (
            <p role="status" className="text-muted-foreground text-xs">
              <Trans>Saving date…</Trans>
            </p>
          )}
          {metadata.error && (
            <>
              <p role="alert" className="text-destructive text-xs">
                {metadata.error.message}
              </p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={!readable()}
                onClick={() => {
                  if (readable()) void metadata.refetch();
                }}
              >
                <Trans>Reload saved date</Trans>
              </Button>
            </>
          )}
        </>
      )}
    </div>
  );
}

function toDatetimeLocalValue(value: unknown): string {
  const date = safeParseDate(value);
  return date ? format(date, "yyyy-MM-dd'T'HH:mm") : "";
}

function toIsoString(value: string): string | null {
  return safeParseDate(value)?.toISOString() ?? null;
}
