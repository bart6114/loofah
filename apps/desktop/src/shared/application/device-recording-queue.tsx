import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";

import { Button } from "@hypr/ui/components/ui/button";

import type { createDeviceRecordingMonitor } from "./device-recordings";

import type {
  CaptureIdentity,
  CaptureUploadProgress,
  DeviceRecordingInfo,
} from "~/types/tauri.gen";

export function DeviceRecordingQueue({
  profileId,
  client,
  resumeUpload,
}: {
  profileId: string;
  client: ReturnType<typeof createDeviceRecordingMonitor>;
  resumeUpload?: (identity: CaptureIdentity) => Promise<CaptureUploadProgress>;
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
          key={JSON.stringify([
            recording.identity.scope,
            recording.identity.capture_id,
          ])}
          recording={recording}
          client={client}
          resumeUpload={resumeUpload}
        />
      ))}
    </section>
  );
}

function DeviceRecordingRow({
  recording,
  client,
  resumeUpload,
}: {
  recording: DeviceRecordingInfo;
  client: ReturnType<typeof createDeviceRecordingMonitor>;
  resumeUpload?: (identity: CaptureIdentity) => Promise<CaptureUploadProgress>;
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
        ) : !verified && recording.sealed ? (
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
      <p role="status">
        {verified && !active ? (
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
      {(recording.failure ||
        recording.upload?.error ||
        stop.error ||
        resume.error) && (
        <p role="alert" className="text-destructive">
          {stop.error?.message ??
            resume.error?.message ??
            recording.failure ??
            recording.upload?.error?.message}
        </p>
      )}
    </div>
  );
}
