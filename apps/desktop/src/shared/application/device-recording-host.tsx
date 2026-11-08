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
import { DeviceRecordingQueue } from "./device-recording-queue";
import {
  createDeviceRecordingClient,
  createDeviceRecordingMonitor,
} from "./device-recordings";
import { useVaultReadScope } from "./use-vault-read-scope";
import { VaultHostContext } from "./vault-provider";
import type { VaultScope } from "./vault-scope";

import type {
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
  return null;
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
  const cleanupCurrent = () =>
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
      {current && (
        <PendingRecordButton
          key={current.scopeKey}
          profileId={profileId}
          recording={current}
        />
      )}
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
    </RecordingContext.Provider>
  );
}

function PendingRecordButton({
  profileId,
  recording,
}: {
  profileId: string;
  recording: DeviceRecordingOwner;
}) {
  const cache = useQueryClient();
  const processing = useDeviceProcessingOptions();
  const queryKey = ["device", "recordings", profileId];
  const inventory = useQuery({
    queryKey,
    queryFn: () => recording.client.recoveryInventory(),
    networkMode: "always",
    staleTime: Infinity,
    retry: false,
  });
  const intent = useRef<Omit<DeviceCaptureStartRequest, "scope"> | null>(null);
  const start = useMutation({
    networkMode: "always",
    retry: false,
    mutationFn: () => {
      const request = (intent.current ??= {
        request_id: crypto.randomUUID(),
        target: {
          kind: "pending_session",
          create_request_id: crypto.randomUUID(),
        },
        format: {
          encoding: "pcm_f32_le",
          sample_rate: 16000,
          channels: ["microphone", "system_audio"],
        },
        processing: captureProcessingOptions(processing.state.values),
      });
      return recording.client.start(request);
    },
    onSuccess: (recording) => {
      cache.setQueryData<import("~/types/tauri.gen").DeviceRecordingInfo[]>(
        queryKey,
        (items = []) => [
          ...items.filter(
            (item) =>
              JSON.stringify(item.identity) !==
              JSON.stringify(recording.identity),
          ),
          recording,
        ],
      );
    },
    onSettled: () => {
      void cache.invalidateQueries({ queryKey });
    },
  });
  const active = inventory.data?.some(
    (item) =>
      item.profile_id === profileId &&
      ["preparing", "starting", "capturing", "stopping", "pending"].includes(
        item.phase ?? "",
      ),
  );
  return (
    <div className="flex flex-col items-start gap-1">
      <DeviceProcessingOptions
        form={processing}
        disabled={start.isPending || (!!intent.current && !start.isSuccess)}
      />
      <Button
        size="sm"
        variant="outline"
        disabled={start.isPending || !!active || !inventory.isSuccess}
        onClick={() => {
          if (start.isSuccess) intent.current = null;
          start.mutate();
        }}
      >
        {start.isPending ? (
          <Trans>Starting recording…</Trans>
        ) : (
          <Trans>Record a new note</Trans>
        )}
      </Button>
      <p className="text-muted-foreground text-xs">
        <Trans>
          Audio is saved on this device. A note is created after upload to the
          selected vault.
        </Trans>
      </p>
      {(start.error || inventory.error) && (
        <p role="alert" className="text-destructive text-xs">
          {(start.error ?? inventory.error)?.message}
        </p>
      )}
    </div>
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
    !audio.data &&
    transcript.isSuccess &&
    !transcript.data.has_words;
  return (
    <div className="flex flex-col items-start gap-1">
      <DeviceProcessingOptions
        form={processing}
        disabled={start.isPending || !!intent.current}
      />
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
