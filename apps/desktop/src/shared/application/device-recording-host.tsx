import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  useCallback,
  useMemo,
  useSyncExternalStore,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { Button } from "@hypr/ui/components/ui/button";

import {
  captureProcessingOptions,
  DeviceProcessingOptions,
  useDeviceProcessingOptions,
} from "./device-processing-options";
import { DeviceRecordingControls } from "./device-recording-controls";
import { DeviceRecordingQueue } from "./device-recording-queue";
import {
  createDeviceRecordingClient,
  createDeviceRecordingMonitor,
} from "./device-recordings";
import { MediaRecoveryPanel } from "./media-recovery-panel";
import { useVaultReadScope } from "./use-vault-read-scope";
import { VaultHostContext } from "./vault-provider";
import type { VaultScope } from "./vault-scope";

import type {
  CaptureAppendRequest,
  CaptureScope,
  DeviceCaptureStartRequest,
} from "~/types/tauri.gen";

export type DeviceRecordingDestination = {
  scopeKey: string;
  scope: CaptureScope;
  connectionId?: string;
};

type DeviceRecordingOwner = {
  scopeKey: string;
  connectionId?: string;
  client: ReturnType<typeof createDeviceRecordingClient>;
};
const RecordingContext = createContext<
  | (DeviceRecordingOwner & { bindAccess: (scope: VaultScope) => () => void })
  | null
>(null);

export function DeviceRecordingAccess() {
  const recording = useContext(RecordingContext);
  const scope = useContext(VaultHostContext)?.scope;
  useEffect(
    () => (scope && recording ? recording.bindAccess(scope) : undefined),
    [scope, recording],
  );
  return scope?.client.media?.pendingSubmissions ? (
    <MediaRecoveryPanel key={scope.client.scope} scope={scope} />
  ) : null;
}

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
    | (DeviceRecordingOwner & {
        profileId: string;
        destination: DeviceRecordingDestination;
      })
    | null
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
    setOwner({
      profileId,
      scopeKey: destination.scopeKey,
      connectionId: destination.connectionId,
      client,
      destination,
    });
    return () => client.dispose();
  }, [destination, profileId]);
  const current =
    destination === owner?.destination && profileId === owner.profileId
      ? owner
      : null;
  const [access, setAccess] = useState<{
    scope: VaultScope;
    client: DeviceRecordingOwner["client"];
  } | null>(null);
  const bindAccess = useCallback(
    (scope: VaultScope, client: DeviceRecordingOwner["client"]) => {
      const next = { scope, client };
      setAccess(next);
      return () => setAccess((current) => (current === next ? null : current));
    },
    [],
  );
  const context = useMemo(
    () =>
      current
        ? {
            ...current,
            bindAccess: (scope: VaultScope) =>
              bindAccess(scope, current.client),
          }
        : null,
    [current, bindAccess],
  );
  const connection = useSyncExternalStore(
    useCallback(
      (changed) => access?.scope.subscribe(changed) ?? (() => {}),
      [access],
    ),
    useCallback(() => access?.scope.getSnapshot().state ?? "closed", [access]),
  );
  const latestAuthorization = useRef({ current, access, profileId });
  latestAuthorization.current = { current, access, profileId };
  const cleanupCurrent = () =>
    latestAuthorization.current.current === current &&
    latestAuthorization.current.access === access &&
    latestAuthorization.current.profileId === profileId &&
    !!current &&
    access?.client === current.client &&
    access.scope.client.scope === current.scopeKey &&
    access.scope.client.nativeConnectionId ===
      current.destination.connectionId &&
    access.scope.getSnapshot().state === "connected" &&
    access.scope.client.canEdit &&
    access.scope.client.features.includes("capture_retirement") &&
    !!current.destination.connectionId;

  return (
    <RecordingContext.Provider value={context}>
      <DeviceRecordingControls
        profileId={profileId}
        recording={current}
        startAllowed={
          !(
            current &&
            access?.client === current.client &&
            access.scope.client.scope === current.scopeKey &&
            access.scope.client.nativeConnectionId ===
              current.destination.connectionId &&
            connection === "connected" &&
            !access.scope.client.canEdit
          )
        }
        monitor={monitor?.profileId === profileId ? monitor.client : null}
      >
        {monitor?.profileId === profileId && (
          <DeviceRecordingQueue
            profileId={profileId}
            client={monitor.client}
            selectedScope={destination?.scope ?? null}
            retire={
              connection === "connected" && cleanupCurrent()
                ? (identity, requestId) =>
                    current!.client.retire(identity, requestId, cleanupCurrent)
                : undefined
            }
            resumeUpload={
              current?.destination.connectionId
                ? current.client.resumeUpload
                : undefined
            }
          />
        )}
        {children}
      </DeviceRecordingControls>
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
  const processing = useDeviceProcessingOptions();
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
  const appendPermitted =
    permitted &&
    !!recording.connectionId &&
    scope.client.nativeConnectionId === recording.connectionId &&
    scope.client.features.includes("capture_append");
  const appendCurrent = () =>
    owner.current() &&
    scope.getSnapshot().state === "connected" &&
    appendPermitted;
  const appendPending = useQuery({
    queryKey: [
      "device",
      "append-preparations",
      recording.connectionId,
      sessionId,
    ],
    queryFn: async () => {
      const requests = (
        await recording.client.pendingAppendRequests(appendCurrent)
      ).filter((request) => request.session_id === sessionId);
      if (requests.length > 1)
        throw new Error(
          "Multiple saved append requests need recovery before recording again.",
        );
      return requests[0] ?? null;
    },
    enabled: appendPermitted && audio.isSuccess && !!audio.data,
    networkMode: "always",
    retry: false,
    staleTime: 0,
  });
  const appendIntent = useRef<Omit<CaptureAppendRequest, "scope"> | null>(null);
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
      if (appendIntent.current || (!intent.current && audio.data)) {
        if (
          !appendCurrent() ||
          (!appendIntent.current && !appendPending.isSuccess)
        )
          throw new Error(
            "Reconnect to the original vault before appending audio.",
          );
        const request = (appendIntent.current ??= appendPending.data ?? {
          session_id: sessionId,
          request_id: crypto.randomUUID(),
          capture_id: crypto.randomUUID(),
          admission_id: crypto.randomUUID(),
          processing: captureProcessingOptions(processing.state.values),
        });
        try {
          return await recording.client.append(request, appendCurrent);
        } finally {
          void cache.invalidateQueries({ queryKey: ["device", "recordings"] });
          void cache.invalidateQueries({
            queryKey: ["device", "append-preparations"],
          });
        }
      }
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
        processing: captureProcessingOptions(processing.state.values),
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
    (appendIntent.current || (!intent.current && audio.data)
      ? appendPermitted && (!!appendIntent.current || appendPending.isSuccess)
      : !!intent.current ||
        (transcript.isSuccess && !transcript.data.has_words));
  return (
    <div className="flex flex-col items-start gap-1">
      <DeviceProcessingOptions
        form={processing}
        disabled={
          !permitted ||
          start.isPending ||
          !!intent.current ||
          !!appendIntent.current ||
          !!appendPending.data
        }
      />
      <Button
        size="sm"
        variant="outline"
        disabled={!canStart || start.isPending || start.isSuccess}
        onClick={() => start.mutate()}
      >
        {start.isPending ? (
          <Trans>Starting recording…</Trans>
        ) : audio.data ? (
          <Trans>Append audio</Trans>
        ) : (
          <Trans>Record audio</Trans>
        )}
      </Button>
      {audio.data && (
        <p className="text-muted-foreground text-xs">
          {appendPending.data ? (
            <Trans>
              Retry the saved append request with its original recording and
              processing choices.
            </Trans>
          ) : (
            <Trans>
              Append to the verified recording. Enable transcription to include
              the new audio in the transcript.
            </Trans>
          )}
        </p>
      )}
      {(audio.error ||
        transcript.error ||
        appendPending.error ||
        start.error) && (
        <p role="alert" className="text-destructive text-xs">
          {start.error?.message ??
            audio.error?.message ??
            appendPending.error?.message ??
            transcript.error?.message}
        </p>
      )}
    </div>
  );
}
