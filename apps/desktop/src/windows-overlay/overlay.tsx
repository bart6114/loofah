import "./style.css";

import { useForm } from "@tanstack/react-form";
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
  const preferences = useMutation({
    mutationFn: () => commands.overlaySetSettingsOpen(true),
  });
  return (
    <section
      className="bar"
      data-theme={state.colorScheme}
      data-status={state.status}
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
      <button
        aria-label="Recording display settings"
        onClick={() => preferences.mutate()}
      >
        ⚙
      </button>
      {stop.isError && <span role="alert">Could not stop. Open Loofah.</span>}
    </section>
  );
}

function DisplaySettings({ state }: { state: FloatingBarState }) {
  const save = useMutation({ mutationFn: settings });
  const close = useMutation({
    mutationFn: () => commands.overlaySetSettingsOpen(false),
  });
  const form = useForm({
    defaultValues: {
      floatingBarOpacity: state.opacity,
      liveCaptionOpacity: state.liveCaptionOpacity,
      liveCaptionWidth: state.liveCaptionWidth,
      liveCaptionLineCount: state.liveCaptionLineCount,
      liveCaptionPosition: state.liveCaptionPosition,
    },
    listeners: {
      onChange: ({ formApi }) => {
        void formApi.handleSubmit();
      },
    },
    onSubmit: ({ value }) => {
      save.mutate(value);
    },
  });
  return (
    <section className="display-settings" data-theme={state.colorScheme}>
      <header data-tauri-drag-region>
        <span>Recording display</span>
        <button
          aria-label="Close display settings"
          onClick={() => close.mutate()}
        >
          ×
        </button>
      </header>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        {(
          [
            ["floatingBarOpacity", "Control opacity", 0.3, 1, 0.05],
            ["liveCaptionOpacity", "Caption opacity", 0.3, 1, 0.05],
            ["liveCaptionWidth", "Caption width", 320, 1000, 10],
            ["liveCaptionLineCount", "Caption lines", 1, 12, 1],
          ] as const
        ).map(([name, title, min, max, step]) => (
          <form.Field key={name} name={name}>
            {(field) => (
              <label>
                <span>
                  {title} <output>{field.state.value}</output>
                </span>
                <input
                  type="range"
                  aria-label={title}
                  min={min}
                  max={max}
                  step={step}
                  value={field.state.value}
                  onChange={(event) =>
                    field.handleChange(Number(event.target.value))
                  }
                />
              </label>
            )}
          </form.Field>
        ))}
        <form.Field name="liveCaptionPosition">
          {(field) => (
            <label>
              Caption position
              <select
                value={field.state.value}
                onChange={(event) =>
                  field.handleChange(
                    event.target
                      .value as FloatingBarState["liveCaptionPosition"],
                  )
                }
              >
                {(
                  [
                    ["topLeft", "Top left"],
                    ["topCenter", "Top center"],
                    ["topRight", "Top right"],
                    ["bottomLeft", "Bottom left"],
                    ["bottomCenter", "Bottom center"],
                    ["bottomRight", "Bottom right"],
                  ] as const
                ).map(([value, title]) => (
                  <option key={value} value={value}>
                    {title}
                  </option>
                ))}
              </select>
            </label>
          )}
        </form.Field>
        {save.isError && <p role="alert">Could not save display settings.</p>}
      </form>
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
    case "settings":
      return <DisplaySettings state={state.state} />;
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
