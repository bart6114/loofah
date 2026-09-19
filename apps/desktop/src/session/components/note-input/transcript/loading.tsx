import { Trans } from "@lingui/react/macro";

import { Spinner } from "@hypr/ui/components/ui/spinner";

export function TranscriptLoadingState() {
  return (
    <div
      role="status"
      className="text-muted-foreground flex min-h-24 flex-1 items-center justify-center gap-2 text-sm"
    >
      <Spinner size={18} />
      <span>
        <Trans>Loading transcript...</Trans>
      </span>
    </div>
  );
}

export function TranscriptLoadError({ retry }: { retry: () => void }) {
  return (
    <div
      role="alert"
      className="text-muted-foreground flex items-center justify-center gap-2 py-4 text-sm"
    >
      <span>
        <Trans>Couldn't load transcript.</Trans>
      </span>
      <button className="text-foreground underline" onClick={retry}>
        <Trans>Retry</Trans>
      </button>
    </div>
  );
}
