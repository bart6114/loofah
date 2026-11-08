import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { Button } from "@hypr/ui/components/ui/button";

import { DeviceRecordingQueue } from "./device-recording-queue";
import {
  createDeviceRecordingClient,
  createDeviceRecordingMonitor,
} from "./device-recordings";
import { useVaultReadScope } from "./use-vault-read-scope";

import type {
  CaptureScope,
  DeviceCaptureStartRequest,
} from "~/types/tauri.gen";

export type DeviceRecordingDestination = {
  scopeKey: string;
  scope: CaptureScope;
  connectionId: string;
};

type DeviceRecordingOwner = {
  scopeKey: string;
  client: ReturnType<typeof createDeviceRecordingClient>;
};
const RecordingContext = createContext<DeviceRecordingOwner | null>(null);

export function DeviceRecordingHost({
  profileId,
  destination,
  children,
}: {
  profileId: string;
  destination: DeviceRecordingDestination | null;
  children: ReactNode;
}) {
  const [monitor, setMonitor] = useState<{
    profileId: string;
    client: ReturnType<typeof createDeviceRecordingMonitor>;
  } | null>(null);
  const [owner, setOwner] = useState<
    (DeviceRecordingOwner & { destination: DeviceRecordingDestination }) | null
  >(null);
  useEffect(() => {
    const client = createDeviceRecordingMonitor();
    setMonitor({ profileId, client });
    return () => client.dispose();
  }, [profileId]);
  useEffect(() => {
    if (!destination) {
      setOwner(null);
      return;
    }
    const client = createDeviceRecordingClient(destination);
    setOwner({ scopeKey: destination.scopeKey, client, destination });
    return () => client.dispose();
  }, [destination]);
  const current = destination === owner?.destination ? owner : null;
  return (
    <RecordingContext.Provider value={current}>
      {monitor?.profileId === profileId && (
        <DeviceRecordingQueue
          profileId={profileId}
          client={monitor.client}
          resumeUpload={current?.client.resumeUpload}
        />
      )}
      {children}
    </RecordingContext.Provider>
  );
}

export function DeviceRecordButton({ sessionId }: { sessionId: string }) {
  const recording = useContext(RecordingContext);
  const owner = useVaultReadScope();
  if (!recording || recording.scopeKey !== owner.scope.client.scope)
    return null;
  return (
    <AdmittedRecordButton
      key={JSON.stringify([recording.scopeKey, sessionId])}
      sessionId={sessionId}
      recording={recording}
    />
  );
}

function AdmittedRecordButton({
  sessionId,
  recording,
}: {
  sessionId: string;
  recording: DeviceRecordingOwner;
}) {
  const owner = useVaultReadScope();
  const cache = useQueryClient();
  const { scope } = owner;
  const connected = owner.current() && owner.connection.state === "connected";
  const permitted =
    connected &&
    scope.client.canEdit &&
    scope.client.features.includes("capture_publication");
  const audio = useQuery(
    { ...scope.audioMetadataOptions(sessionId), enabled: permitted },
    scope.queryClient,
  );
  const transcript = useQuery(
    { ...scope.transcriptOverviewOptions(sessionId), enabled: permitted },
    scope.queryClient,
  );
  const intent = useRef<Omit<DeviceCaptureStartRequest, "scope"> | null>(null);
  const start = useMutation({
    networkMode: "always",
    retry: false,
    mutationFn: async () => {
      if (
        !owner.current() ||
        scope.getSnapshot().state !== "connected" ||
        !permitted
      )
        throw new Error("Reconnect to this vault before starting a recording.");
      if (
        !audio.isSuccess ||
        !transcript.isSuccess ||
        audio.data ||
        transcript.data.has_words
      )
        throw new Error("Create a new note for this recording.");
      const request = (intent.current ??= {
        request_id: crypto.randomUUID(),
        target: {
          kind: "existing_session",
          session_id: sessionId,
          expected_audio_sha256: null,
        },
        format: {
          encoding: "pcm_f32_le",
          sample_rate: 16000,
          channels: ["microphone", "system_audio"],
        },
      });
      try {
        return await recording.client.start(request);
      } finally {
        void cache.invalidateQueries({ queryKey: ["device", "recordings"] });
      }
    },
  });
  const canStart =
    permitted &&
    audio.isSuccess &&
    !audio.data &&
    transcript.isSuccess &&
    !transcript.data.has_words;
  return (
    <div className="flex flex-col items-start gap-1">
      <Button
        size="sm"
        variant="outline"
        disabled={!canStart || start.isPending || start.isSuccess}
        onClick={() => start.mutate()}
      >
        {start.isPending ? (
          <Trans>Starting recording…</Trans>
        ) : (
          <Trans>Record audio</Trans>
        )}
      </Button>
      {(audio.error || transcript.error || start.error) && (
        <p role="alert" className="text-destructive text-xs">
          {start.error?.message ??
            audio.error?.message ??
            transcript.error?.message}
        </p>
      )}
    </div>
  );
}
