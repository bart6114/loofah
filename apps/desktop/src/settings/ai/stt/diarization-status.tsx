import { Trans } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { arch, platform } from "@tauri-apps/plugin-os";
import { Check, Loader2 } from "lucide-react";

import type { LocalModel } from "@hypr/plugin-local-stt";
import { cn } from "@hypr/utils";

import { useNotifications } from "~/contexts/notifications";
import { useLocalModelDownload } from "~/stt/useLocalSttModel";

const DIARIZER_MODEL: LocalModel = "diarizer-fluid-community";

// Diarization is always on when its model is present: this row is a passive
// status readout (downloads happen automatically alongside STT models), not a
// selectable model or a toggle.
export function DiarizationStatus() {
  const targetArch = useQuery({
    queryKey: ["target-arch"],
    queryFn: () => arch(),
    staleTime: Infinity,
  });

  if (platform() !== "windows" && targetArch.data !== "aarch64") {
    return null;
  }

  return <DiarizationStatusRow />;
}

function DiarizationStatusRow() {
  const { activeDownloads } = useNotifications();
  const { isDownloaded, showProgress, hasError, errorMessage, handleDownload } =
    useLocalModelDownload(DIARIZER_MODEL);

  const downloadInfo = activeDownloads.find((d) => d.model === DIARIZER_MODEL);
  const isDownloading = !isDownloaded && (showProgress || !!downloadInfo);
  const isFailed = !isDownloaded && !isDownloading && hasError;

  return (
    <div
      className={cn([
        "flex items-center justify-between gap-3",
        "rounded-lg border border-dashed px-3 py-2",
      ])}
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-sm font-medium">
          <Trans>Speaker detection</Trans>
        </span>
        <span className="text-muted-foreground text-xs">
          <Trans>
            Included automatically with on-device transcription — no setup
            needed.
          </Trans>
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-2 text-[11px]">
        {isDownloaded ? (
          <span className="text-muted-foreground flex items-center gap-1">
            <Check className="text-brand size-3.5" />
            <Trans>Ready</Trans>
          </span>
        ) : isDownloading ? (
          <span
            className={cn([
              "rounded-full px-2 py-0.5 font-medium",
              "flex items-center gap-1",
              "from-muted to-accent text-muted-foreground bg-linear-to-t",
            ])}
          >
            <Loader2 className="size-3 animate-spin" />
            {downloadInfo ? (
              <span>{Math.round(downloadInfo.progress)}%</span>
            ) : (
              <Trans>Starting</Trans>
            )}
          </span>
        ) : isFailed ? (
          <>
            <span
              className="text-destructive"
              title={errorMessage ?? undefined}
            >
              <Trans>Download failed</Trans>
            </span>
            <button
              className={cn([
                "rounded-full px-2 py-0.5 font-medium",
                "transition-all duration-150",
                "from-muted to-accent text-foreground bg-linear-to-t shadow-xs hover:shadow-md",
              ])}
              onClick={handleDownload}
            >
              <Trans>Retry</Trans>
            </button>
          </>
        ) : (
          <span className="text-muted-foreground">
            <Trans>Not downloaded</Trans>
          </span>
        )}
      </div>
    </div>
  );
}
