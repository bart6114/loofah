import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
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

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";

import { deleteSessionAudio } from "~/session/attachments";
import { listenerStore } from "~/store/zustand/listener/instance";

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

// Precomputed on the Rust side and cached beside the recording. Rendering from these peaks
// lets WaveSurfer skip fetching + WebAudio-decoding the whole file in the webview (seconds
// for an hour-long recording); playback still streams through the media element.
function useAudioPeaks(sessionId: string, enabled: boolean) {
  const peaksQuery = useQuery({
    enabled,
    queryKey: ["audio", sessionId, "peaks"],
    queryFn: () => fsSyncCommands.audioPeaks(sessionId),
    select: (result) => (result.status === "error" ? null : result.data),
    retry: false,
  });

  return {
    peaks: peaksQuery.data ?? null,
    peaksResolved: peaksQuery.isFetched || !enabled,
  };
}

function useAudioExistence(sessionId: string) {
  const audioExists = useQuery({
    queryKey: ["audio", sessionId, "exist"],
    queryFn: () => fsSyncCommands.audioExist(sessionId),
    select: (result) => {
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
  });

  return {
    audioExists: audioExists.data ?? false,
    audioExistsResolved: audioExists.isSuccess,
  };
}

export function useAudioExists(sessionId: string): boolean {
  return useAudioExistence(sessionId).audioExists;
}

export function AudioPlayerProvider({
  sessionId,
  url,
  children,
}: {
  sessionId: string;
  url: string;
  children: ReactNode;
}) {
  const queryClient = useQueryClient();
  const isPro = true;
  const [container, setContainer] = useState<HTMLDivElement | null>(null);
  const [wavesurfer, setWavesurfer] = useState<WaveSurfer | null>(null);
  const [state, setState] = useState<AudioPlayerState>("stopped");
  const [playbackRate, setPlaybackRateState] = useState(1);
  const timeStoreRef = useRef(new TimeStore());
  const stopRequestedRef = useRef(false);
  const { audioExists: audioExistsValue, audioExistsResolved } =
    useAudioExistence(sessionId);
  const { peaks, peaksResolved } = useAudioPeaks(sessionId, Boolean(url));

  const registerContainer = useCallback((el: HTMLDivElement | null) => {
    setContainer((prev) => (prev === el ? prev : el));
  }, []);

  useEffect(() => {
    // Waiting for peaksResolved avoids a race where WaveSurfer starts the expensive
    // fetch+decode fallback that precomputed peaks exist to skip.
    if (!container || !url || !peaksResolved) {
      return;
    }

    const store = timeStoreRef.current;
    store.reset();
    stopRequestedRef.current = false;

    const audio = new Audio(url);
    let lastReportedTime = 0;

    const ws = WaveSurfer.create({
      container,
      height: 20,
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
        { waveColor: "#e8d5d5", progressColor: "#c9a3a3", overlay: true },
        { waveColor: "#d5dde8", progressColor: "#a3b3c9", overlay: true },
      ],
      ...(peaks ? { peaks: peaks.channels, duration: peaks.durationSec } : {}),
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

    ws.on("decode", handleDecode);
    ws.on("play", handlePlay);
    ws.on("pause", handlePause);
    ws.on("finish", handleFinish);
    ws.on("ready", handleReady);
    ws.on("timeupdate", handleTimeupdate);
    ws.on("interaction", handleInteraction);
    ws.on("destroy", handleDestroy);

    setWavesurfer(ws);

    return () => {
      stopRequestedRef.current = false;
      ws.destroy();
      setWavesurfer(null);
      audio.pause();
      audio.src = "";
      audio.load();
    };
  }, [container, url, peaks, peaksResolved]);

  const start = useCallback(() => {
    if (wavesurfer) {
      void wavesurfer.play();
    }
  }, [wavesurfer]);

  const pause = useCallback(() => {
    if (wavesurfer) {
      wavesurfer.pause();
    }
  }, [wavesurfer]);

  const resume = useCallback(() => {
    if (wavesurfer) {
      void wavesurfer.play();
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

  const deleteRecordingMutation = useMutation({
    mutationFn: async () => {
      const deleted = await deleteSessionAudio(sessionId, () =>
        isSessionAudioIdle(sessionId),
      );
      if (!deleted) {
        throw new Error("audio_session_busy");
      }
    },
    onSuccess: () => {
      stop();
      timeStoreRef.current.reset();
      void queryClient.invalidateQueries({
        queryKey: ["audio", sessionId],
      });
    },
  });

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
      audioExists: audioExistsValue,
      audioExistsResolved,
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
      audioExistsValue,
      audioExistsResolved,
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

function isSessionAudioIdle(sessionId: string) {
  const state = listenerStore.getState();
  return (
    state.getSessionMode(sessionId) === "inactive" &&
    !(state.live.sessionId === sessionId && state.live.loading)
  );
}
