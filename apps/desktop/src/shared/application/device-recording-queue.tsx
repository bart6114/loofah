import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import type { createDeviceRecordingMonitor } from "./device-recordings";

import type {
  CaptureIdentity,
  CaptureScope,
  CaptureUploadProgress,
  DeviceRecordingInfo,
} from "~/types/tauri.gen";

export function DeviceRecordingQueue({
  profileId,
  client,
  resumeUpload,
  retire,
  selectedScope,
}: {
  profileId: string;
  client: ReturnType<typeof createDeviceRecordingMonitor>;
  resumeUpload?: (identity: CaptureIdentity) => Promise<CaptureUploadProgress>;
  retire?: (
    identity: CaptureIdentity,
    requestId: string,
  ) => Promise<DeviceRecordingInfo>;
  selectedScope?: CaptureScope | null;
}) {
  const cache = useQueryClient();
  const queryKey = useMemo(
    () => ["device", "recordings", profileId],
    [profileId],
  );
  const inventory = useQuery({
    queryKey,
    queryFn: () => client.recoveryInventory(),
    networkMode: "always",
    staleTime: Infinity,
    retry: false,
    refetchOnMount: "always",
  });
  const subscription = useMutation({
    mutationFn: (signal: AbortSignal) =>
      client.subscribe(
        () => {
          void cache.invalidateQueries({ queryKey });
        },
        { signal },
      ),
    networkMode: "always",
  });
  const listen = subscription.mutateAsync;
  useEffect(() => {
    const abort = new AbortController();
    let close: (() => void) | undefined;
    void listen(abort.signal)
      .then((unsubscribe) => {
        if (abort.signal.aborted) unsubscribe();
        else close = unsubscribe;
      })
      .catch(() => {});
    return () => {
      abort.abort();
      close?.();
    };
  }, [client, listen]);
  const refresh = useMutation({
    mutationFn: () => client.recoveryInventory(true),
    onSuccess: (value) => cache.setQueryData(queryKey, value),
    networkMode: "always",
  });
  const recordings =
    inventory.data?.filter((item) => item.profile_id === profileId) ?? [];
  if (
    !recordings.length &&
    !inventory.error &&
    !subscription.error &&
    !refresh.error
  )
    return null;
  return (
    <section
      aria-label="Device recordings"
      className="flex flex-col gap-2 rounded border p-3"
    >
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-medium">
          <Trans>Recordings on this device</Trans>
        </h2>
        <Button
          size="sm"
          variant="ghost"
          disabled={refresh.isPending}
          onClick={() => refresh.mutate()}
        >
          <Trans>Refresh</Trans>
        </Button>
      </div>
      {(inventory.error || subscription.error || refresh.error) && (
        <p role="alert" className="text-destructive text-xs">
          {(inventory.error ?? subscription.error ?? refresh.error)?.message}
        </p>
      )}
      {recordings.map((recording) => (
        <DeviceRecordingRow
          key={JSON.stringify(recording.identity)}
          recording={recording}
          client={client}
          resumeUpload={resumeUpload}
          retire={retire}
          processingVisible={
            !!selectedScope &&
            selectedScope.server === recording.identity.scope.server &&
            selectedScope.member_id === recording.identity.scope.member_id &&
            selectedScope.vault_id === recording.identity.scope.vault_id
          }
        />
      ))}
    </section>
  );
}

function DeviceRecordingRow({
  recording,
  client,
  resumeUpload,
  retire,
  processingVisible,
}: {
  recording: DeviceRecordingInfo;
  client: ReturnType<typeof createDeviceRecordingMonitor>;
  resumeUpload?: (identity: CaptureIdentity) => Promise<CaptureUploadProgress>;
  retire?: (
    identity: CaptureIdentity,
    requestId: string,
  ) => Promise<DeviceRecordingInfo>;
  processingVisible: boolean;
}) {
  const cache = useQueryClient();
  const stop = useMutation({
    mutationFn: () => client.stop(recording.identity),
    onSuccess: () =>
      cache.invalidateQueries({ queryKey: ["device", "recordings"] }),
    networkMode: "always",
  });
  const resume = useMutation({
    mutationFn: () => {
      if (!resumeUpload)
        throw new Error(
          "Reconnect to the original server account to resume this upload.",
        );
      return resumeUpload(recording.identity);
    },
    onSuccess: () =>
      cache.invalidateQueries({ queryKey: ["device", "recordings"] }),
    networkMode: "always",
  });
  const finish = useMutation({
    mutationFn: () => client.finishRecovery(recording.identity),
    onSuccess: () =>
      cache.invalidateQueries({ queryKey: ["device", "recordings"] }),
    networkMode: "always",
    retry: false,
  });
  const exportAudio = useMutation({
    mutationFn: () => client.exportAudio(recording.identity),
    networkMode: "always",
    retry: false,
  });
  const cleanupRequest = useRef<string | null>(null);
  const cleanup = useMutation({
    mutationFn: () => {
      if (!retire || !processingVisible)
        throw new Error(
          "Reconnect to the original editable vault before freeing device audio.",
        );
      const requestId =
        recording.retirement?.request.cleanup_request_id ??
        (cleanupRequest.current ??= crypto.randomUUID());
      return retire(recording.identity, requestId);
    },
    onSettled: () =>
      cache.invalidateQueries({ queryKey: ["device", "recordings"] }),
    networkMode: "always",
    retry: false,
  });
  const retirement = cleanup.data?.retirement ?? recording.retirement;
  const retired = retirement?.state === "retired";
  const rawFenced = retired || retirement?.state === "pending";
  const processingSettled =
    !recording.processing?.status ||
    [
      "succeeded",
      "empty",
      "cancelled",
      "conflict",
      "interrupted",
      "failed",
    ].includes(recording.processing.status?.state ?? "");
  const active = [
    "preparing",
    "starting",
    "capturing",
    "stopping",
    "pending",
  ].includes(recording.phase ?? "");
  const verified = !!recording.publication;
  return (
    <div className="flex flex-col gap-1 text-xs">
      <div className="flex items-center justify-between gap-2">
        <span>
          {recording.first_audio_ms === null ? (
            <Trans>Recording</Trans>
          ) : (
            new Date(recording.first_audio_ms).toLocaleString()
          )}
        </span>
        {active ? (
          <Button
            size="sm"
            variant="outline"
            disabled={stop.isPending}
            onClick={() => stop.mutate()}
          >
            <Trans>Stop recording</Trans>
          </Button>
        ) : !rawFenced && !verified && recording.sealed ? (
          <Button
            size="sm"
            variant="outline"
            disabled={resume.isPending || !resumeUpload}
            onClick={() => resume.mutate()}
          >
            <Trans>Resume upload</Trans>
          </Button>
        ) : null}
      </div>
      <p className="text-muted-foreground break-all">
        {recording.identity.scope.server} · {recording.identity.scope.vault_id}
      </p>
      {!active && !rawFenced && recording.frames > 0 && (
        <div className="flex flex-wrap gap-2">
          {!recording.sealed && recording.observation_durable && (
            <Button
              size="sm"
              variant="outline"
              disabled={finish.isPending}
              onClick={() => finish.mutate()}
            >
              <Trans>Prepare saved audio for upload</Trans>
            </Button>
          )}
          <Button
            size="sm"
            variant="outline"
            disabled={exportAudio.isPending}
            onClick={() => exportAudio.mutate()}
          >
            <Trans>Export saved audio</Trans>
          </Button>
        </div>
      )}
      {!active &&
        !retired &&
        recording.sealed &&
        verified &&
        processingVisible &&
        retire && (
          <div className="flex flex-col items-start gap-1">
            <Button
              size="sm"
              variant="outline"
              disabled={cleanup.isPending || !processingSettled}
              onClick={() => cleanup.mutate()}
            >
              {cleanup.isPending ? (
                <Trans>Verifying server copy…</Trans>
              ) : retirement ? (
                <Trans>Retry cleanup</Trans>
              ) : (
                <Trans>Free device audio</Trans>
              )}
            </Button>
            <p className="text-muted-foreground">
              <Trans>
                Verify the server copy, then remove this device's saved audio.
              </Trans>
            </p>
          </div>
        )}
      {exportAudio.data && !rawFenced && (
        <p role="status">
          <Trans>
            Audio exported. The original recording is retained on this device.
          </Trans>
        </p>
      )}
      <p role="status">
        {retired ? (
          <Trans>
            Server copy verified. Saved audio has been removed from this device.
          </Trans>
        ) : retirement?.state === "pending" ? (
          <Trans>
            Cleanup was interrupted. Reconnect to the original vault and retry
            verification.
          </Trans>
        ) : retirement?.state === "declared" ? (
          <Trans>
            Cleanup is requested. Device audio is retained until verification
            finishes.
          </Trans>
        ) : verified && !active ? (
          <Trans>Verified on the server. The device copy is retained.</Trans>
        ) : recording.phase === "capturing" ? (
          <Trans>Recording on this device.</Trans>
        ) : recording.phase === "preparing" ||
          recording.phase === "starting" ? (
          <Trans>Starting recording…</Trans>
        ) : active ? (
          <Trans>Finishing recording and saving device audio…</Trans>
        ) : recording.sealed ? (
          <Trans>Audio is saved on this device and awaiting upload.</Trans>
        ) : recording.frames > 0 ? (
          <Trans>
            Recording interrupted. Saved audio is retained for recovery.
          </Trans>
        ) : (
          <Trans>Recording did not start.</Trans>
        )}
      </p>
      {processingVisible && recording.processing && (
        <DeviceProcessingDetails recording={recording} client={client} />
      )}
      {(recording.failure ||
        recording.upload?.error ||
        stop.error ||
        finish.error ||
        exportAudio.error ||
        resume.error ||
        cleanup.error) && (
        <p role="alert" className="text-destructive">
          {cleanup.error?.message ??
            stop.error?.message ??
            finish.error?.message ??
            exportAudio.error?.message ??
            resume.error?.message ??
            recording.failure ??
            recording.upload?.error?.message}
        </p>
      )}
    </div>
  );
}

function DeviceProcessingDetails({
  recording,
  client,
}: {
  recording: DeviceRecordingInfo;
  client: ReturnType<typeof createDeviceRecordingMonitor>;
}) {
  const cache = useQueryClient();
  const cancel = useMutation({
    mutationFn: () => client.cancelProcessing(recording.identity),
    onSettled: () =>
      cache.invalidateQueries({ queryKey: ["device", "recordings"] }),
    networkMode: "always",
    retry: false,
  });
  const processing = recording.processing!;
  const status = processing.status;
  const captions = processing.captions;
  const visibleCaptions =
    captions && ["connected", "reconnecting"].includes(captions.connection);
  const terminal =
    status &&
    [
      "succeeded",
      "empty",
      "cancelled",
      "conflict",
      "interrupted",
      "uncertain",
      "failed",
    ].includes(status.state);
  return (
    <section
      aria-label="Recording processing"
      className="flex flex-col gap-2 rounded border p-2"
    >
      <p role="status">
        {processing.declaration.cancel_requested ? (
          <Trans>
            Processing cancellation is saved. Audio capture and upload continue.
          </Trans>
        ) : status?.state === "succeeded" ? (
          <Trans>Server processing finished.</Trans>
        ) : status?.state === "empty" ? (
          <Trans>No speech was found.</Trans>
        ) : status?.state === "cancelled" ? (
          <Trans>Server processing was cancelled.</Trans>
        ) : status?.state === "live" ? (
          <Trans>Transcribing on the server.</Trans>
        ) : status?.state === "repairing" ? (
          <Trans>Completing the transcript from saved audio.</Trans>
        ) : status?.state === "summarizing" ? (
          <Trans>Creating the summary.</Trans>
        ) : status?.state === "waiting_for_publication" ? (
          <Trans>
            Waiting for the complete recording to finish processing.
          </Trans>
        ) : terminal ? (
          <Trans>
            Server processing needs review. The device recording is retained.
          </Trans>
        ) : (
          <Trans>
            Processing was requested and will start after the server receives
            audio.
          </Trans>
        )}
      </p>
      {visibleCaptions && (
        <div aria-label="Recent live captions">
          <p>{captions.words.map((word) => word.text).join(" ")}</p>
          <p className="text-muted-foreground">
            {captions.partials.map((word) => word.text).join(" ")}
          </p>
          {captions.connection === "reconnecting" && (
            <p role="status">
              <Trans>
                Live captions are interrupted. Recording continues on this
                device.
              </Trans>
            </p>
          )}
        </div>
      )}
      {!terminal && !processing.declaration.cancel_requested && (
        <Button
          size="sm"
          variant="outline"
          disabled={cancel.isPending}
          onClick={() => cancel.mutate()}
        >
          <Trans>Cancel processing</Trans>
        </Button>
      )}
      {(processing.error || cancel.error) && (
        <p role="alert">{cancel.error?.message ?? processing.error?.message}</p>
      )}
    </section>
  );
}
