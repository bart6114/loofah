import { Trans } from "@lingui/react/macro";

import { useTimelineSnapshot } from "./owner";

export function TimelineStreamControl({ streamKey }: { streamKey: string }) {
  const { owner, state, available } = useTimelineSnapshot();
  const stream = state.streams.get(streamKey);
  if (!available || !stream) return null;
  const loaded = stream.pages.reduce(
    (count, page) => count + page.items.length,
    0,
  );
  const total = stream.pages[0]?.total_count;
  const next = stream.pages[stream.pages.length - 1]?.next_cursor;
  return (
    <div
      className="text-muted-foreground flex h-9 items-center justify-center gap-2 px-3 text-[11px]"
      aria-live="polite"
    >
      {stream.pending ? (
        <span role="status">
          <Trans>Loading notes…</Trans>
        </span>
      ) : stream.error ? (
        <>
          <span role="alert" className="truncate" title={stream.error.message}>
            {stream.error.message}
          </span>
          <button type="button" onClick={() => owner.retry(streamKey)}>
            <Trans>Retry</Trans>
          </button>
        </>
      ) : (
        <>
          {total !== undefined && (
            <span>
              <Trans>
                {loaded} of {total} loaded
              </Trans>
            </span>
          )}
          {next && (
            <button type="button" onClick={() => owner.more(streamKey)}>
              {streamKey === "past" ? (
                <Trans>Load earlier</Trans>
              ) : streamKey === "future" ? (
                <Trans>Load later</Trans>
              ) : (
                <Trans>Load more</Trans>
              )}
            </button>
          )}
        </>
      )}
    </div>
  );
}
