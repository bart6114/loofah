import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  type ReactNode,
} from "react";

import { Button } from "@hypr/ui/components/ui/button";

import {
  captureProcessingOptions,
  DeviceProcessingOptions,
  useDeviceProcessingOptions,
} from "./device-processing-options";
import { DeviceRecordingShell } from "./device-recording-shell";
import type {
  createDeviceRecordingClient,
  createDeviceRecordingMonitor,
} from "./device-recordings";

import type {
  DeviceCaptureStartRequest,
  DeviceRecordingInfo,
} from "~/types/tauri.gen";

export type DeviceStartActions = {
  readonly canStart: boolean;
  startPending: (options?: {
    retry?: boolean;
    triggerAppIds?: string[];
  }) => Promise<DeviceRecordingInfo>;
};
const Actions = createContext<DeviceStartActions | null>(null);
export const useDeviceStartActions = () => useContext(Actions);
export const activeDeviceCapture = (item: DeviceRecordingInfo) =>
  ["preparing", "starting", "capturing", "stopping", "pending"].includes(
    item.phase ?? "",
  );

type Owner = {
  scopeKey: string;
  client: ReturnType<typeof createDeviceRecordingClient>;
};
type Intent = {
  profileId: string;
  owner: Owner;
  request: Omit<DeviceCaptureStartRequest, "scope">;
  promise?: Promise<DeviceRecordingInfo>;
  complete: boolean;
  triggerAppIds: string[];
};
export function DeviceRecordingControls({
  profileId,
  recording,
  monitor,
  startAllowed = true,
  children,
}: {
  profileId: string;
  recording: Owner | null;
  monitor: ReturnType<typeof createDeviceRecordingMonitor> | null;
  startAllowed?: boolean;
  children: ReactNode;
}) {
  const { t } = useLingui();
  const cache = useQueryClient();
  const processing = useDeviceProcessingOptions();
  const key = ["device", "recordings", profileId];
  const inventory = useQuery({
    queryKey: key,
    queryFn: () => monitor!.recoveryInventory(),
    enabled: !!monitor,
    networkMode: "always",
    staleTime: Infinity,
    retry: false,
  });
  const intent = useRef<Intent | null>(null);
  const original = useRef(recording);
  original.current = recording;
  useEffect(() => {
    original.current = recording;
    return () => {
      if (original.current === recording) original.current = null;
    };
  }, [recording]);
  const active = inventory.data?.some(activeDeviceCapture) ?? true;
  const availability = useRef({
    ready: inventory.isSuccess,
    active,
    allowed: startAllowed,
  });
  availability.current = {
    ready: inventory.isSuccess,
    active,
    allowed: startAllowed,
  };
  const start = useMutation({
    networkMode: "always",
    retry: false,
    mutationFn: (attempt: Intent) => {
      if (!availability.current.allowed)
        throw new Error(t`This workspace is read-only.`);
      return attempt.owner.client.start(attempt.request);
    },
    onSuccess: (item, attempt) => {
      cache.setQueryData<DeviceRecordingInfo[]>(
        ["device", "recordings", attempt.profileId],
        (items = []) => [
          ...items.filter(
            (previous) =>
              JSON.stringify(previous.identity) !==
              JSON.stringify(item.identity),
          ),
          item,
        ],
      );
    },
    onSettled: (_result, _error, attempt) => {
      void cache.invalidateQueries({
        queryKey: ["device", "recordings", attempt.profileId],
      });
    },
  });
  const canStart =
    startAllowed && !start.isPending && !active && inventory.isSuccess;
  const startPending = useCallback(
    async ({
      retry = false,
      triggerAppIds = [],
    }: { retry?: boolean; triggerAppIds?: string[] } = {}) => {
      if (
        !recording ||
        original.current !== recording ||
        !availability.current.ready
      )
        throw new Error("Recording destination is not ready");
      if (!availability.current.allowed)
        throw new Error(t`This workspace is read-only.`);
      const previous = intent.current;
      if (previous?.owner === recording && previous.promise)
        return previous.promise;
      if (availability.current.active)
        throw new Error(
          "A recording is already active. Use its existing controls.",
        );
      if (previous?.owner === recording && !previous.complete && !retry)
        throw new Error(
          "Check the original recording request before retrying.",
        );
      const attempt: Intent =
        previous?.owner === recording && !previous.complete
          ? previous
          : {
              profileId,
              owner: recording,
              complete: false,
              triggerAppIds: [...triggerAppIds],
              request: {
                request_id: crypto.randomUUID(),
                target: {
                  kind: "pending_session" as const,
                  create_request_id: crypto.randomUUID(),
                },
                format: {
                  encoding: "pcm_f32_le" as const,
                  sample_rate: 16000,
                  channels: ["microphone" as const, "system_audio" as const],
                },
                processing: structuredClone(
                  captureProcessingOptions(processing.state.values),
                ),
              },
            };
      intent.current = attempt;
      const promise = start
        .mutateAsync(attempt)
        .then((item) => {
          attempt.complete = true;
          return item;
        })
        .finally(() => {
          attempt.promise = undefined;
        });
      attempt.promise = promise;
      return promise;
    },
    [recording, profileId, inventory.isSuccess, active, processing, start, t],
  );
  return (
    <Actions.Provider
      value={
        recording
          ? {
              startPending,
              canStart:
                canStart &&
                !(
                  intent.current?.owner === recording &&
                  !intent.current.complete
                ),
            }
          : null
      }
    >
      {recording && (
        <div className="flex flex-col items-start gap-1">
          <DeviceProcessingOptions
            form={processing}
            disabled={
              !startAllowed ||
              start.isPending ||
              (intent.current?.owner === recording && !intent.current.complete)
            }
          />
          <Button
            size="sm"
            variant="outline"
            disabled={!canStart}
            onClick={() => {
              void startPending({ retry: true }).catch(() => {});
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
              Audio is saved on this device. A note is created after upload to
              the selected vault.
            </Trans>
          </p>
          {(start.error || inventory.error) && (
            <p role="alert" className="text-destructive text-xs">
              {(start.error ?? inventory.error)?.message}
            </p>
          )}
        </div>
      )}
      {monitor && (
        <DeviceRecordingShell profileId={profileId} monitor={monitor} />
      )}
      {children}
    </Actions.Provider>
  );
}
