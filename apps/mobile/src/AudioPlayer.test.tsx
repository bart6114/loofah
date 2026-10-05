import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { createRef, StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AudioPlayer } from "./AudioPlayer";

beforeEach(() => {
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function metadata(audio: HTMLAudioElement, duration: number) {
  Object.defineProperty(audio, "duration", {
    value: duration,
    configurable: true,
  });
  fireEvent.loadedMetadata(audio);
}

describe("recording playback", () => {
  it("retains its source when development effects remount", () => {
    const audioRef = createRef<HTMLAudioElement>();
    render(
      <StrictMode>
        <AudioPlayer src="recording.mp3" audioRef={audioRef} />
      </StrictMode>,
    );
    expect(audioRef.current?.getAttribute("src")).toBe("recording.mp3");
  });
  it("tracks external playback and provides accessible seeking and speed", () => {
    const audioRef = createRef<HTMLAudioElement>();
    render(<AudioPlayer src="recording.mp3" audioRef={audioRef} />);
    const audio = audioRef.current!;
    const seek = screen.getByRole("slider", {
      name: "Seek recording",
    }) as HTMLInputElement;
    expect(seek.disabled).toBe(true);
    metadata(audio, 125);
    expect(seek.disabled).toBe(false);
    audio.currentTime = 65;
    fireEvent.timeUpdate(audio);
    expect(seek.getAttribute("aria-valuetext")).toBe("1:05 of 2:05");
    fireEvent.change(seek, { target: { value: "80" } });
    expect(audio.currentTime).toBe(80);
    fireEvent.play(audio);
    expect(
      screen.getByRole("button", { name: "Pause recording" }),
    ).toBeTruthy();
    fireEvent.change(screen.getByRole("combobox", { name: "Playback speed" }), {
      target: { value: "1.5" },
    });
    expect(audio.playbackRate).toBe(1.5);
    fireEvent.click(screen.getByRole("button", { name: "Pause recording" }));
    expect(audio.pause).toHaveBeenCalled();
  });

  it("shows buffering and starts ended recordings again from the beginning", async () => {
    const audioRef = createRef<HTMLAudioElement>();
    render(<AudioPlayer src="recording.mp3" audioRef={audioRef} />);
    const audio = audioRef.current!;
    metadata(audio, 60);
    fireEvent.waiting(audio);
    expect(screen.getByRole("status").textContent).toBe("Loading recording…");
    fireEvent.playing(audio);
    expect(screen.queryByRole("status")).toBeNull();
    audio.currentTime = 60;
    fireEvent.ended(audio);
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Play recording" })),
    );
    expect(audio.currentTime).toBe(0);
    expect(audio.play).toHaveBeenCalledOnce();
  });

  it("recovers from failed play requests and media errors", async () => {
    const audioRef = createRef<HTMLAudioElement>();
    vi.mocked(HTMLMediaElement.prototype.play).mockRejectedValueOnce(
      new Error("Unavailable"),
    );
    render(<AudioPlayer src="recording.mp3" audioRef={audioRef} />);
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Play recording" })),
    );
    expect(screen.getByRole("alert").textContent).toContain("Couldn’t play");
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Retry" })),
    );
    expect(screen.queryByRole("alert")).toBeNull();
    expect(audioRef.current?.load).toHaveBeenCalledOnce();
    fireEvent.error(audioRef.current!);
    expect(screen.getByRole("alert").textContent).toContain(
      "couldn’t be loaded",
    );
  });

  it("stops old audio and ignores pending playback failures after a source change", async () => {
    const audioRef = createRef<HTMLAudioElement>();
    let rejectPlay!: (error: Error) => void;
    vi.mocked(HTMLMediaElement.prototype.play).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectPlay = reject;
        }),
    );
    const view = render(<AudioPlayer src="first.mp3" audioRef={audioRef} />);
    const oldAudio = audioRef.current!;
    metadata(oldAudio, 120);
    oldAudio.currentTime = 40;
    fireEvent.timeUpdate(oldAudio);
    fireEvent.click(screen.getByRole("button", { name: "Play recording" }));
    view.rerender(<AudioPlayer src="second.mp3" audioRef={audioRef} />);
    expect(audioRef.current).not.toBe(oldAudio);
    expect(oldAudio.getAttribute("src")).toBeNull();
    expect(oldAudio.pause).toHaveBeenCalledOnce();
    await act(async () => rejectPlay(new Error("Late failure")));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("slider").getAttribute("value")).toBe("0");
    view.unmount();
    expect(HTMLMediaElement.prototype.pause).toHaveBeenCalledTimes(2);
  });
});
