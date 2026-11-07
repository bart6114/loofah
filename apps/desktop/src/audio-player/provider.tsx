import { Trans } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import WaveSurfer from "wavesurfer.js";

import type { VaultScope } from "~/shared/application/vault-scope";

const TIME_UPDATE_STEP_SECONDS = 0.1;

type AudioPlayerState = "playing" | "paused" | "stopped";

interface TimeSnapshot {
  current: number;
  total: number;
}

class TimeStore {
  private snapshot: TimeSnapshot = { current: 0, total: 0 };
  private listeners = new Set<() => void>();

  getSnapshot = (): TimeSnapshot => {
    return this.snapshot;
  };

  subscribe = (cb: () => void): (() => void) => {
    this.listeners.add(cb);
    return () => {
      this.listeners.delete(cb);
    };
  };

  setCurrent(value: number) {
    if (value === this.snapshot.current) return;
    this.snapshot = { ...this.snapshot, current: value };
    this.notify();
  }

  setTotal(value: number) {
    if (value === this.snapshot.total) return;
    this.snapshot = { ...this.snapshot, total: value };
    this.notify();
  }

  reset() {
    this.snapshot = { current: 0, total: 0 };
    this.notify();
  }

  private notify() {
    for (const cb of this.listeners) {
      cb();
    }
  }
}

interface AudioPlayerContextValue {
  registerContainer: (el: HTMLDivElement | null) => void;
  wavesurfer: WaveSurfer | null;
  state: AudioPlayerState;
  timeStore: TimeStore;
  start: () => void;
  pause: () => void;
  resume: () => void;
  stop: () => void;
  seek: (sec: number) => void;
  audioExists: boolean;
  audioExistsResolved: boolean;
  playbackRate: number;
  setPlaybackRate: (rate: number) => void;
  deleteRecording: () => Promise<void>;
  isDeletingRecording: boolean;
  canDeleteRecording: boolean;
  audioReady: boolean;
  error: Error | null;
  deletionFailed: boolean;
  retry: () => void;
}

const AudioPlayerContext = createContext<AudioPlayerContextValue | null>(null);

export function useAudioPlayer() {
  const context = useContext(AudioPlayerContext);
  if (!context) {
    throw new Error("useAudioPlayer must be used within AudioPlayerProvider");
  }
  return context;
}

export function useAudioTime(): TimeSnapshot {
  const { timeStore } = useAudioPlayer();
  return useSyncExternalStore(timeStore.subscribe, timeStore.getSnapshot);
}

export function AudioPlayerStatus() {
  const { error, retry, deletionFailed } = useAudioPlayer();
  if (!error) return null;
  return (
    <div
      role="alert"
      className="text-destructive flex items-center gap-2 text-xs"
    >
      <span>
        {deletionFailed ? (
          <Trans>
            Recording deletion was not confirmed. Reload it before trying again.
          </Trans>
        ) : (
          <Trans>Recording playback is unavailable.</Trans>
        )}
      </span>
      <button type="button" className="underline" onClick={retry}>
        <Trans>Retry</Trans>
      </button>
    </div>
  );
}

export function AudioPlayerProvider({
  sessionId,
  scope,
  enabled = true,
  onDeleteRecording,
  children,
}: {
  sessionId: string;
  scope: VaultScope | null;
  enabled?: boolean;
  onDeleteRecording?: (remove: () => Promise<void>) => Promise<void>;
  children: ReactNode;
}) {
  const recordingQuery = useQuery(
    {
      queryKey: ["vault", "unbound", "audio_resource", sessionId],
      queryFn: async () => null,
      ...scope?.recordingOptions(sessionId),
      enabled: !!scope && enabled,
    },
    scope?.queryClient,
  );
  const recording = recordingQuery.data ?? null;
  const peaksQuery = useQuery(
    {
      queryKey: ["vault", "unbound", "audio_peaks", sessionId],
      queryFn: async () => null,
      ...scope?.recordingPeaksOptions(sessionId, recording),
      enabled: !!scope && enabled && !!recording,
    },
    scope?.queryClient,
  );
  const peaks = peaksQuery.data ?? null;
  const peaksResolved = peaksQuery.isFetched;
  const url = enabled ? (recording?.url ?? "") : "";
  const [playbackError, setPlaybackError] = useState<Error | null>(null);
  const isPro = true;
  const [container, setContainer] = useState<HTMLDivElement | null>(null);
  const [wavesurfer, setWavesurfer] = useState<WaveSurfer | null>(null);
  const [state, setState] = useState<AudioPlayerState>("stopped");
  const [playbackRate, setPlaybackRateState] = useState(1);
  const timeStoreRef = useRef(new TimeStore());
  const stopRequestedRef = useRef(false);
  const currentPlayerRef = useRef<WaveSurfer | null>(null);
  const checkpointRef = useRef<{
    key: string;
    time: number;
    playing: boolean;
  } | null>(null);
  const playbackKey = recording
    ? JSON.stringify([scope?.client.scope, sessionId, recording.resumeKey])
    : null;

  const registerContainer = useCallback((el: HTMLDivElement | null) => {
    setContainer((prev) => (prev === el ? prev : el));
  }, []);

  useEffect(() => {
    if (!enabled) {
      checkpointRef.current = null;
      timeStoreRef.current.reset();
    }
    // Waiting for peaksResolved avoids a race where WaveSurfer starts the expensive
    // fetch+decode fallback that precomputed peaks exist to skip.
    if (!container || !url || !peaksResolved) {
      return;
    }

    const store = timeStoreRef.current;
    store.reset();
    const checkpoint =
      checkpointRef.current?.key === playbackKey ? checkpointRef.current : null;
    checkpointRef.current = checkpoint;
    if (checkpoint) store.setCurrent(checkpoint.time);
    stopRequestedRef.current = false;
    let ready = false;
    let active = true;

    setPlaybackError(null);
    const audio = new Audio();
    audio.preload = "metadata";
    audio.src = url;
    let lastReportedTime = 0;

    const ws = WaveSurfer.create({
      container,
      height: 28,
      waveColor: "#e5e5e5",
      progressColor: "#a8a8a8",
      cursorColor: "#737373",
      cursorWidth: 1.5,
      barWidth: 2,
      barGap: 2,
      barRadius: 2,
      barHeight: 1,
      media: audio,
      dragToSeek: true,
      normalize: true,
      splitChannels: [
        { waveColor: "#d6b8b8", progressColor: "#a77474", overlay: true },
        { waveColor: "#b7c6d8", progressColor: "#748fae", overlay: true },
      ],
      // An unavailable waveform must never trigger a whole-recording WebAudio fetch.
      peaks: peaks?.channels ?? [[0, 0]],
      ...(peaks ? { duration: peaks.durationSec } : {}),
    });

    if (peaks) {
      store.setTotal(peaks.durationSec);
    }

    const syncCurrentTime = (currentTime: number, force = false) => {
      if (
        !force &&
        Math.abs(currentTime - lastReportedTime) < TIME_UPDATE_STEP_SECONDS
      ) {
        return;
      }

      lastReportedTime = currentTime;
      store.setCurrent(currentTime);
    };

    const handleReady = (dur: number) => {
      if (dur && isFinite(dur)) {
        store.setTotal(dur);
      }
      if (!ready && checkpoint) {
        ws.setTime(Math.min(checkpoint.time, dur || checkpoint.time));
        if (checkpoint.playing)
          void ws.play().catch((error) => {
            if (active) setPlaybackError(error);
          });
      }
      ready = true;
    };

    const handlePlay = () => {
      stopRequestedRef.current = false;
      syncCurrentTime(ws.getCurrentTime(), true);
      setState("playing");
    };

    const handlePause = () => {
      const currentTime = ws.getCurrentTime();
      syncCurrentTime(currentTime, true);

      if (stopRequestedRef.current) {
        stopRequestedRef.current = false;
        setState("stopped");
        return;
      }

      setState("paused");
    };

    const handleFinish = () => {
      stopRequestedRef.current = false;
      syncCurrentTime(ws.getDuration(), true);
      setState("stopped");
    };

    const handleTimeupdate = (currentTime: number) => {
      syncCurrentTime(currentTime);
    };

    const handleInteraction = (currentTime: number) => {
      syncCurrentTime(currentTime, true);
    };

    const handleDecode = (dur: number) => {
      if (dur && isFinite(dur)) {
        store.setTotal(dur);
      }
    };

    const handleDestroy = () => {
      stopRequestedRef.current = false;
      setState("stopped");
    };

    ws.on("error", (error) => {
      if (active) setPlaybackError(error);
    });
    ws.on("decode", handleDecode);
    ws.on("play", handlePlay);
    ws.on("pause", handlePause);
    ws.on("finish", handleFinish);
    ws.on("ready", handleReady);
    ws.on("timeupdate", handleTimeupdate);
    ws.on("interaction", handleInteraction);
    ws.on("destroy", handleDestroy);

    setWavesurfer(ws);
    currentPlayerRef.current = ws;

    return () => {
      active = false;
      if (currentPlayerRef.current === ws) currentPlayerRef.current = null;
      if (ready && playbackKey)
        checkpointRef.current = {
          key: playbackKey,
          time: ws.getCurrentTime(),
          playing: ws.isPlaying(),
        };
      stopRequestedRef.current = false;
      ws.destroy();
      setWavesurfer(null);
      audio.pause();
      audio.src = "";
      audio.load();
    };
  }, [container, url, peaks, peaksResolved, playbackKey, enabled]);

  const start = useCallback(() => {
    if (wavesurfer) {
      void wavesurfer.play().catch((error) => {
        if (currentPlayerRef.current === wavesurfer) setPlaybackError(error);
      });
    }
  }, [wavesurfer]);

  const pause = useCallback(() => {
    if (wavesurfer) {
      wavesurfer.pause();
    }
  }, [wavesurfer]);

  const resume = useCallback(() => {
    if (wavesurfer) {
      void wavesurfer.play().catch((error) => {
        if (currentPlayerRef.current === wavesurfer) setPlaybackError(error);
      });
    }
  }, [wavesurfer]);

  const stop = useCallback(() => {
    if (wavesurfer) {
      const wasPlaying = wavesurfer.isPlaying();
      stopRequestedRef.current = wasPlaying;
      wavesurfer.stop();
      timeStoreRef.current.setCurrent(0);
      if (!wasPlaying) {
        setState("stopped");
      }
    }
  }, [wavesurfer]);

  const seek = useCallback(
    (timeInSeconds: number) => {
      if (wavesurfer) {
        wavesurfer.setTime(timeInSeconds);
      }
    },
    [wavesurfer],
  );

  const setPlaybackRate = useCallback(
    (rate: number) => {
      if (!isPro && rate !== 1) {
        return;
      }
      if (wavesurfer) {
        wavesurfer.setPlaybackRate(rate, false);
      }
      setPlaybackRateState(rate);
    },
    [isPro, wavesurfer],
  );

  useEffect(() => {
    if (!wavesurfer) {
      return;
    }

    const nextRate = isPro ? playbackRate : 1;
    wavesurfer.setPlaybackRate(nextRate, false);

    if (nextRate !== playbackRate) {
      setPlaybackRateState(1);
    }
  }, [isPro, playbackRate, wavesurfer]);

  const hasDeleteCapability =
    !!scope?.client.canEdit &&
    scope.client.features.includes("recording_delete");
  const deleteRecordingMutation = useMutation({
    networkMode: "always",
    retry: false,
    mutationFn: async () => {
      if (!scope || !recording || !hasDeleteCapability)
        throw new Error("Recording deletion is unavailable");
      const remove = async () => {
        const expected_sha256 = await recording.deletionRevision();
        if (expected_sha256 === null) return;
        await scope.run(() =>
          scope.client.deleteRecording(sessionId, { expected_sha256 }),
        );
      };
      if (onDeleteRecording) await onDeleteRecording(remove);
      else await remove();
    },
    onSuccess: () => {
      stop();
      setPlaybackError(null);
      timeStoreRef.current.reset();
      for (const kind of ["audio_resource", "audio_peaks", "audio_metadata"]) {
        void scope?.queryClient.invalidateQueries({
          queryKey: ["vault", scope.client.scope, kind, sessionId],
        });
      }
    },
  });
  const canDeleteRecording =
    hasDeleteCapability &&
    enabled &&
    !!recording &&
    !recordingQuery.isFetching &&
    !recordingQuery.isError &&
    !deleteRecordingMutation.isError;
  const { refetch: refetchRecording } = recordingQuery;
  const { reset: resetDeletion } = deleteRecordingMutation;
  const retry = useCallback(() => {
    resetDeletion();
    setPlaybackError(null);
    void refetchRecording();
  }, [refetchRecording, resetDeletion]);
  const error =
    deleteRecordingMutation.error ?? recordingQuery.error ?? playbackError;

  const value = useMemo<AudioPlayerContextValue>(
    () => ({
      registerContainer,
      wavesurfer,
      state,
      timeStore: timeStoreRef.current,
      start,
      pause,
      resume,
      stop,
      seek,
      audioExists: !!recording,
      audioExistsResolved: recordingQuery.isSuccess,
      audioReady: !!url,
      canDeleteRecording,
      error,
      deletionFailed: !!deleteRecordingMutation.error,
      retry,
      playbackRate,
      setPlaybackRate,
      deleteRecording: deleteRecordingMutation.mutateAsync,
      isDeletingRecording: deleteRecordingMutation.isPending,
    }),
    [
      registerContainer,
      wavesurfer,
      state,
      start,
      pause,
      resume,
      stop,
      seek,
      recording,
      recordingQuery.isSuccess,
      url,
      canDeleteRecording,
      deleteRecordingMutation.error,
      error,
      retry,
      playbackRate,
      setPlaybackRate,
      deleteRecordingMutation.mutateAsync,
      deleteRecordingMutation.isPending,
    ],
  );

  return (
    <AudioPlayerContext.Provider value={value}>
      {children}
    </AudioPlayerContext.Provider>
  );
}
