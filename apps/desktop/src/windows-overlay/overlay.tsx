import "./style.css";

import { useMutation } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";

import {
  commands,
  events,
  type FloatingBarState,
  type FloatingBarSettingsChange,
  type OverlaySnapshot,
} from "@hypr/plugin-windows";

const settings = (change: Partial<FloatingBarSettingsChange>) =>
  events.floatingBarSettingsChange.emit({
    floatingBarOpacity: null,
    liveCaptionOpacity: null,
    liveCaptionWidth: null,
    liveCaptionLineCount: null,
    liveCaptionPosition: null,
    liveCaptionMinimized: null,
    ...change,
  });

function RecordingBar({ state }: { state: FloatingBarState }) {
  const stop = useMutation({
    mutationFn: () => events.floatingBarStop.emit({}),
  });
  const captions = useMutation({
    mutationFn: () =>
      settings({ liveCaptionMinimized: !state.liveCaptionMinimized }),
  });
  return (
    <section
      className="bar"
      data-theme={state.colorScheme}
      style={{
        backgroundColor: `rgb(var(--surface) / ${Math.max(0.2, state.opacity)})`,
      }}
    >
      <span
        className="grip"
        data-tauri-drag-region
        aria-label="Drag recording controls"
      >
        ⠿
      </span>
      <span
        className="level"
        aria-label={state.status === "error" ? "Recording error" : "Recording"}
      >
        <span
          style={{
            transform: `scaleY(${Math.max(0.15, Math.min(1, state.amplitude))})`,
          }}
        />
      </span>
      <button
        className="title"
        onClick={() => void events.floatingBarOpenMain.emit({})}
      >
        {state.title || "Recording"}
      </button>
      {state.liveCaptionToggleVisible && (
        <button
          aria-label="Toggle live captions"
          aria-pressed={!state.liveCaptionMinimized}
          onClick={() => captions.mutate()}
        >
          CC
        </button>
      )}
      <button
        className="stop"
        disabled={stop.isPending}
        onClick={() => stop.mutate()}
        aria-label="Stop recording"
      >
        ■ Stop
      </button>
      {stop.isError && <span role="alert">Could not stop. Open Loofah.</span>}
    </section>
  );
}

function Transcript({ state }: { state: FloatingBarState }) {
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    body.current?.scrollTo({ top: body.current.scrollHeight });
  }, [state.transcriptBubbles]);
  const minimize = useMutation({
    mutationFn: () => settings({ liveCaptionMinimized: true }),
  });
  return (
    <section
      className="captions"
      data-theme={state.colorScheme}
      style={{
        backgroundColor: `rgb(var(--surface) / ${Math.max(0.2, state.liveCaptionOpacity)})`,
      }}
    >
      <header data-tauri-drag-region>
        <span>Live captions</span>
        <button
          aria-label="Minimize captions"
          onClick={() => minimize.mutate()}
        >
          −
        </button>
      </header>
      <div
        ref={body}
        className="transcript"
        role="log"
        aria-live="polite"
        aria-relevant="additions text"
      >
        {state.transcriptBubbles.length === 0 ? (
          <p className="muted">Listening…</p>
        ) : (
          state.transcriptBubbles.map((bubble) => (
            <p key={bubble.id} data-final={bubble.isFinal}>
              <strong>
                {bubble.speakerLabel || (bubble.isSelf ? "You" : "Speaker")}
              </strong>{" "}
              {bubble.text}
            </p>
          ))
        )}
      </div>
    </section>
  );
}

const previews = [
  ["navigation:onboarding", "Onboarding"],
  ["notifications:mic-detected", "Microphone reminder"],
  ["notifications:mic-options", "Reminder choices"],
  ["notifications:auto-stop", "Auto-stop reminder"],
  ["notifications:batch-done", "Transcription complete"],
  ["notifications:clear", "Clear notifications"],
  ["ota:available", "Update available"],
  ["ota:downloading", "Update downloading"],
  ["ota:ready", "Update ready"],
  ["ota:failed", "Update failed"],
  ["ota:clear", "Clear update preview"],
  ["toasts:clear", "Clear toasts"],
  ["error:trigger", "Test error boundary"],
];

export function Overlay() {
  const [snapshot, setSnapshot] = useState<OverlaySnapshot>({
    revision: 0,
    state: null,
  });
  useEffect(() => {
    let active = true;
    const receive = (value: OverlaySnapshot) => {
      if (active)
        setSnapshot((current) =>
          value.revision >= current.revision ? value : current,
        );
    };
    const unlisten = listen<OverlaySnapshot>("loofah-overlay-state", (event) =>
      receive(event.payload),
    );
    void unlisten
      .then(() => commands.overlaySnapshot())
      .then(receive)
      .catch(console.error);
    return () => {
      active = false;
      void unlisten.then((stop) => stop());
    };
  }, []);
  const state = snapshot.state;
  if (!state) return null;
  switch (state.type) {
    case "floatingBar":
      return <RecordingBar state={state.state} />;
    case "transcript":
      return <Transcript state={state.state} />;
    case "liveCaption":
      return (
        <section className="captions" style={{ opacity: state.state.opacity }}>
          <header data-tauri-drag-region>Live captions</header>
          <div className="transcript">
            {state.state.minimized
              ? "Captions minimized"
              : state.state.text || "Listening…"}
          </div>
        </section>
      );
    case "devtools":
      return (
        <section className="devtools">
          <header data-tauri-drag-region>Developer tools</header>
          {previews.map(([action, title]) => (
            <button
              key={action}
              onClick={() => void events.devtoolsPanelAction.emit({ action })}
            >
              {title}
            </button>
          ))}
        </section>
      );
  }
}
