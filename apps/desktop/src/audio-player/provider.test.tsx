import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const waveform = vi.hoisted(() => ({ create: vi.fn() }));
vi.mock("wavesurfer.js", () => ({ default: { create: waveform.create } }));

import {
  AudioPlayerProvider,
  AudioPlayerStatus,
  useAudioPlayer,
} from "./provider";

import type { PlaybackRecording } from "~/shared/application/playback";
import type { VaultClient } from "~/shared/application/vault-client";
import { VaultScope } from "~/shared/application/vault-scope";

const scopes: VaultScope[] = [];
beforeEach(() => {
  vi.resetAllMocks();
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
  waveform.create.mockImplementation(() => {
    const listeners = new Map<string, (...args: any[]) => void>();
    return {
      on: vi.fn((name: string, listener: (...args: any[]) => void) => {
        listeners.set(name, listener);
      }),
      emit: (name: string, ...args: any[]) => listeners.get(name)?.(...args),
      destroy: vi.fn(),
      getCurrentTime: vi.fn().mockReturnValue(0),
      isPlaying: vi.fn().mockReturnValue(false),
      setTime: vi.fn(),
      setPlaybackRate: vi.fn(),
      play: vi.fn().mockResolvedValue(undefined),
      pause: vi.fn(),
    };
  });
});
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.restoreAllMocks();
});

function Probe() {
  const {
    registerContainer,
    start,
    audioExists,
    canDeleteRecording,
    deleteRecording,
  } = useAudioPlayer();
  return (
    <>
      <div ref={registerContainer} />
      <p>{audioExists ? "recording exists" : "no recording"}</p>
      <button onClick={start}>Play</button>
      {canDeleteRecording && (
        <button onClick={() => void deleteRecording().catch(() => {})}>
          Delete
        </button>
      )}
      <AudioPlayerStatus />
    </>
  );
}
function fixture(peaks: PlaybackRecording["peaks"], canEdit = true) {
  const recordings: PlaybackRecording[] = [];
  let resumeKey = "recording-sha";
  const open = vi.fn(async () => {
    const recording = {
      id: crypto.randomUUID(),
      url: `media:${crypto.randomUUID()}`,
      resumeKey,
      deletionRevision: vi.fn().mockResolvedValue("current-sha"),
      peaks,
      close: vi.fn().mockResolvedValue(undefined),
    };
    recordings.push(recording);
    return recording;
  });
  const remove = vi.fn().mockResolvedValue(true);
  const scope = new VaultScope({
    scope: "fixture",
    networkMode: "always",
    canEdit,
    features: ["recording_delete"],
    deleteRecording: remove,
    openRecording: open,
  } as unknown as VaultClient);
  scopes.push(scope);
  const view = (
    enabled = true,
    onDeleteRecording?: (remove: () => Promise<void>) => Promise<void>,
  ) => (
    <StrictMode>
      <QueryClientProvider client={scope.queryClient}>
        <AudioPlayerProvider
          scope={scope}
          sessionId="session"
          enabled={enabled}
          onDeleteRecording={onDeleteRecording}
        >
          <Probe />
        </AudioPlayerProvider>
      </QueryClientProvider>
    </StrictMode>
  );
  return {
    scope,
    remove,
    recordings,
    open,
    view,
    setResumeKey: (value: string) => {
      resumeKey = value;
    },
  };
}

it.each([true, false])(
  "retains playback position only when refreshed content has the same identity: %s",
  async (same) => {
    const f = fixture(
      vi.fn().mockResolvedValue({ durationSec: 60, channels: [[1]] }),
    );
    render(f.view());
    await waitFor(() => expect(waveform.create).toHaveBeenCalledOnce());
    const first = waveform.create.mock.results[0].value;
    act(() => first.emit("ready", 60));
    first.getCurrentTime.mockReturnValue(34);
    first.isPlaying.mockReturnValue(true);
    if (!same) f.setResumeKey("replacement-sha");
    await act(async () => {
      await f.scope.queryClient.invalidateQueries({
        queryKey: f.scope.recordingOptions("session").queryKey,
      });
    });
    await waitFor(() => expect(waveform.create).toHaveBeenCalledTimes(2));
    const second = waveform.create.mock.results[1].value;
    act(() => second.emit("ready", 60));
    if (same) {
      expect(second.setTime).toHaveBeenCalledWith(34);
      expect(second.play).toHaveBeenCalledOnce();
    } else {
      expect(second.setTime).not.toHaveBeenCalled();
      expect(second.play).not.toHaveBeenCalled();
    }
  },
);

it("deletes the displayed revision through the shared operation inside the device guard", async () => {
  const f = fixture(
    vi.fn().mockResolvedValue({ durationSec: 60, channels: [[1]] }),
  );
  const guard = vi.fn(async (remove: () => Promise<void>) => remove());
  const mounted = render(f.view(true, guard));
  await screen.findByText("recording exists");
  fireEvent.click(screen.getByText("Delete"));
  await waitFor(() =>
    expect(f.remove).toHaveBeenCalledExactlyOnceWith("session", {
      expected_sha256: "current-sha",
    }),
  );
  expect(guard).toHaveBeenCalledOnce();
  mounted.unmount();
});

it("shows guarded deletion failures and reloads without automatically retrying deletion", async () => {
  const f = fixture(
    vi.fn().mockResolvedValue({ durationSec: 60, channels: [[1]] }),
  );
  f.remove.mockRejectedValueOnce(new Error("recording changed"));
  render(f.view());
  await screen.findByText("recording exists");
  fireEvent.click(screen.getByText("Delete"));
  await screen.findByText(
    "Recording deletion was not confirmed. Reload it before trying again.",
  );
  expect(screen.queryByText("Delete")).toBeNull();
  const opened = f.open.mock.calls.length;
  fireEvent.click(screen.getByText("Retry"));
  await waitFor(() => expect(f.open).toHaveBeenCalledTimes(opened + 1));
  expect(f.remove).toHaveBeenCalledOnce();
  await screen.findByText("Delete");
});

it("never dispatches deletion when the device guard rejects or the member is a reader", async () => {
  const f = fixture(
    vi.fn().mockResolvedValue({ durationSec: 60, channels: [[1]] }),
  );
  const mounted = render(
    f.view(true, async () => {
      throw new Error("capture is active");
    }),
  );
  await screen.findByText("recording exists");
  fireEvent.click(screen.getByText("Delete"));
  await screen.findByRole("alert");
  expect(f.remove).not.toHaveBeenCalled();
  mounted.unmount();
  const reader = fixture(
    vi.fn().mockResolvedValue({ durationSec: 60, channels: [[1]] }),
    false,
  );
  render(reader.view());
  await screen.findByText("recording exists");
  expect(screen.queryByText("Delete")).toBeNull();
});

it("ignores a play failure from a replaced player", async () => {
  const f = fixture(
    vi.fn().mockResolvedValue({ durationSec: 60, channels: [[1]] }),
  );
  render(f.view());
  await waitFor(() => expect(waveform.create).toHaveBeenCalledOnce());
  let reject!: (error: Error) => void;
  waveform.create.mock.results[0].value.play.mockReturnValueOnce(
    new Promise((_, fail) => {
      reject = fail;
    }),
  );
  fireEvent.click(screen.getByText("Play"));
  await act(async () => {
    await f.scope.queryClient.invalidateQueries({
      queryKey: f.scope.recordingOptions("session").queryKey,
    });
  });
  await waitFor(() => expect(waveform.create).toHaveBeenCalledTimes(2));
  await act(async () => reject(new Error("old media was closed")));
  expect(screen.queryByRole("alert")).toBeNull();
});

it("does not queue deletion while offline or retry an uncertain mutation through an ambient client", async () => {
  const f = fixture(
    vi.fn().mockResolvedValue({ durationSec: 60, channels: [[1]] }),
  );
  f.remove.mockRejectedValue(new Error("Connection lost; outcome unknown"));
  const ambient = new QueryClient({
    defaultOptions: { mutations: { networkMode: "online", retry: 3 } },
  });
  onlineManager.setOnline(false);
  render(
    <QueryClientProvider client={ambient}>
      <AudioPlayerProvider scope={f.scope} sessionId="session">
        <Probe />
      </AudioPlayerProvider>
    </QueryClientProvider>,
  );
  await screen.findByText("recording exists");
  fireEvent.click(screen.getByText("Delete"));
  await screen.findByRole("alert");
  expect(f.remove).toHaveBeenCalledOnce();
  await act(async () => onlineManager.setOnline(true));
  expect(f.remove).toHaveBeenCalledOnce();
});

it("waits for bounded waveform data before creating a streaming player and closes on unmount", async () => {
  let finish!: (value: unknown) => void;
  const peaks = vi.fn(
    () =>
      new Promise<any>((resolve) => {
        finish = resolve;
      }),
  );
  const f = fixture(peaks);
  const mounted = render(f.view());
  await waitFor(() => expect(peaks).toHaveBeenCalled());
  expect(waveform.create).not.toHaveBeenCalled();
  await act(async () => finish({ durationSec: 60, channels: [[0, 1]] }));
  await waitFor(() => expect(waveform.create).toHaveBeenCalledOnce());
  const options = waveform.create.mock.calls[0][0];
  expect(options.peaks).toEqual([[0, 1]]);
  expect(options.duration).toBe(60);
  expect(options.media.preload).toBe("metadata");
  expect(options.media.src).toBe(f.recordings[f.recordings.length - 1]!.url);
  expect(screen.queryByText("Delete")).not.toBeNull();
  expect(f.remove).not.toHaveBeenCalled();
  mounted.unmount();
  await waitFor(() =>
    expect(f.recordings[f.recordings.length - 1]!.close).toHaveBeenCalledOnce(),
  );
});

it.each(["missing", "failed"])(
  "supplies a flat waveform after %s peaks so WaveSurfer cannot download the whole file",
  async (mode) => {
    const peaks =
      mode === "missing"
        ? vi.fn().mockResolvedValue(null)
        : vi.fn().mockRejectedValue(new Error("decoder unavailable"));
    const f = fixture(peaks);
    render(f.view());
    await waitFor(() => expect(waveform.create).toHaveBeenCalledOnce());
    const options = waveform.create.mock.calls[0][0];
    expect(options.peaks).toEqual([[0, 0]]);
    expect(options.duration).toBeUndefined();
    expect(options.media.preload).toBe("metadata");
  },
);

it("tears down playback during capture without requesting another waveform", async () => {
  const peaks = vi.fn().mockResolvedValue({ durationSec: 1, channels: [[1]] });
  const f = fixture(peaks);
  const mounted = render(f.view());
  await waitFor(() => expect(waveform.create).toHaveBeenCalledOnce());
  const ws = waveform.create.mock.results[0].value;
  act(() => ws.emit("ready", 1));
  ws.getCurrentTime.mockReturnValue(0.5);
  ws.isPlaying.mockReturnValue(true);
  mounted.rerender(f.view(false));
  await waitFor(() => expect(ws.destroy).toHaveBeenCalledOnce());
  expect(peaks).toHaveBeenCalledOnce();
  mounted.rerender(f.view(true));
  await waitFor(() => expect(waveform.create).toHaveBeenCalledTimes(2));
  const resumed = waveform.create.mock.results[1].value;
  act(() => resumed.emit("ready", 1));
  expect(resumed.setTime).not.toHaveBeenCalled();
  expect(resumed.play).not.toHaveBeenCalled();
});

it("opens a fresh descriptor on playback retry and reports rejected play promises", async () => {
  const f = fixture(
    vi.fn().mockResolvedValue({ durationSec: 1, channels: [[1]] }),
  );
  render(f.view());
  await waitFor(() => expect(waveform.create).toHaveBeenCalledOnce());
  waveform.create.mock.results[0].value.play.mockRejectedValueOnce(
    new Error("connection lost"),
  );
  fireEvent.click(screen.getByText("Play"));
  await screen.findByRole("alert");
  fireEvent.click(screen.getByText("Retry"));
  await waitFor(() => expect(waveform.create).toHaveBeenCalledTimes(2));
  expect(f.recordings[0].close).toHaveBeenCalledOnce();
  expect(screen.queryByRole("alert")).toBeNull();
});
