import { Trans, useLingui } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import { recordingStatus } from "./recording-processing-status";
import type { VaultScope } from "./vault-scope";

const PAGE_SIZE = 25;

export function VaultRecordingHistory({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const [owner, setOwner] = useState<{
    scope: VaultScope;
    sessionId: string;
  } | null>(null);
  useEffect(() => setOwner(null), [scope, sessionId]);
  const open = owner?.scope === scope && owner.sessionId === sessionId;
  return (
    <div className="flex flex-col gap-2">
      <Button
        size="sm"
        variant="outline"
        aria-expanded={open}
        disabled={!open && connection.state !== "connected"}
        onClick={() => setOwner(open ? null : { scope, sessionId })}
      >
        <Trans>Processing history</Trans>
      </Button>
      {open && <HistoryPage scope={scope} sessionId={sessionId} />}
    </div>
  );
}

function HistoryPage({
  scope,
  sessionId,
}: {
  scope: VaultScope;
  sessionId: string;
}) {
  const { t } = useLingui();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const connected = connection.state === "connected";
  const [cursor, setCursor] = useState<string | null>(null);
  const page = useQuery(
    {
      queryKey: [
        "recording_processing_history",
        scope.client.scope,
        sessionId,
        cursor,
      ],
      queryFn: ({ signal }) => {
        if (scope.getSnapshot().state !== "connected")
          throw new Error(
            t`Reconnect to the original workspace to read processing history.`,
          );
        return scope.run(() =>
          scope.client.recordingProcessing!.history(
            sessionId,
            cursor,
            PAGE_SIZE,
            { signal },
          ),
        );
      },
      enabled: connected,
      retry: false,
      staleTime: 30_000,
      gcTime: 0,
    },
    scope.queryClient,
  );
  const items =
    page.data?.items
      .slice(0, PAGE_SIZE)
      .filter((item) => item.status.declaration.session_id === sessionId) ?? [];
  return (
    <section
      aria-label={t`Archived recording processing`}
      className="flex flex-col gap-2 text-sm"
    >
      {page.isFetching && (
        <p role="status">
          <Trans>Loading processing history…</Trans>
        </p>
      )}
      {page.error && <p role="alert">{page.error.message}</p>}
      {page.data && !items.length && (
        <p>
          <Trans>No processing history on this page.</Trans>
        </p>
      )}
      {!!items.length && (
        <ul className="flex flex-col gap-2">
          {items.map((item) => (
            <li key={item.status.declaration.request_id}>
              <p>{recordingStatus(item.status)}</p>
              {item.status.message && <p>{item.status.message}</p>}
            </li>
          ))}
        </ul>
      )}
      <div className="flex gap-2">
        {cursor !== null && (
          <Button
            size="sm"
            variant="outline"
            disabled={!connected || page.isFetching}
            onClick={() => setCursor(null)}
          >
            <Trans>First history page</Trans>
          </Button>
        )}
        {page.data?.next_cursor && (
          <Button
            size="sm"
            variant="outline"
            disabled={!connected || page.isFetching}
            onClick={() => setCursor(page.data!.next_cursor)}
          >
            <Trans>Next history page</Trans>
          </Button>
        )}
        {page.isError && (
          <Button
            size="sm"
            variant="outline"
            disabled={!connected || page.isFetching}
            onClick={() => void page.refetch()}
          >
            <Trans>Retry history</Trans>
          </Button>
        )}
      </div>
    </section>
  );
}
