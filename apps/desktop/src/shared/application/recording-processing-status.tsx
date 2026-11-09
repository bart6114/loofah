import { Trans } from "@lingui/react/macro";

import type { LocalCaptureStatus } from "~/types/tauri.gen";

export function recordingStatus(value: LocalCaptureStatus) {
  switch (value.state) {
    case "capturing":
      return <Trans>Recording</Trans>;
    case "waiting_for_persistence":
      return <Trans>Saving recording</Trans>;
    case "transcribing":
      return <Trans>Transcribing recording</Trans>;
    case "summarizing":
      return <Trans>Creating summary</Trans>;
    case "cancelling":
      return <Trans>Cancelling recording processing</Trans>;
    case "cancelled":
      return <Trans>Recording processing cancelled</Trans>;
    case "closed":
      return <Trans>Recording processing closed by review</Trans>;
    case "succeeded":
      return <Trans>Recording processing complete</Trans>;
    case "skipped":
      return <Trans>Recording processing skipped</Trans>;
    case "not_started":
      return <Trans>Recording did not start</Trans>;
    case "review":
      return <Trans>Recording processing needs review</Trans>;
    case "conflict":
      return (
        <Trans>
          Recording processing stopped because the saved content changed
        </Trans>
      );
  }
}
