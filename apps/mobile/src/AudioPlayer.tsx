import "./AudioPlayer.css";

import { LoaderCircle, Pause, Play, RotateCcw } from "lucide-react";
import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type RefObject,
} from "react";

import { time } from "./api";

export function AudioPlayer(props: {
  src: string;
  audioRef?: RefObject<HTMLAudioElement | null>;
}) {
  return <RecordingPlayer key={props.src} {...props} />;
}

function RecordingPlayer({
  src,
  audioRef,
}: {
  src: string;
  audioRef?: RefObject<HTMLAudioElement | null>;
}) {
  const internalRef = useRef<HTMLAudioElement>(null);
  const ref = audioRef ?? internalRef;
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [rate, setRate] = useState(1);
  const request = useRef(0);

  useEffect(() => {
    const audio = ref.current;
    audio?.setAttribute("src", src);
    return () => {
      request.current += 1;
      audio?.pause();
      audio?.removeAttribute("src");
      audio?.load();
    };
  }, [ref, src]);

  const updateTime = () => {
    const audio = ref.current;
    if (!audio) return;
    const total =
      Number.isFinite(audio.duration) && audio.duration > 0
        ? audio.duration
        : 0;
    setDuration(total);
    setPosition(
      Number.isFinite(audio.currentTime) ? Math.max(0, audio.currentTime) : 0,
    );
  };

  const play = async () => {
    const audio = ref.current;
    if (!audio) return;
    const currentRequest = ++request.current;
    setFailure(null);
    setLoading(true);
    if (audio.ended || (duration > 0 && audio.currentTime >= duration)) {
      audio.currentTime = 0;
      setPosition(0);
    }
    try {
      await audio.play();
      if (currentRequest === request.current) setLoading(false);
    } catch (error) {
      if (currentRequest !== request.current) return;
      setLoading(false);
      setPlaying(false);
      if (error instanceof DOMException && error.name === "AbortError") return;
      setFailure("Couldn’t play this recording. Try again.");
    }
  };

  const toggle = () => {
    const audio = ref.current;
    if (!audio) return;
    if (playing || loading) {
      request.current += 1;
      audio.pause();
      setPlaying(false);
      setLoading(false);
    } else {
      void play();
    }
  };

  const progress = duration > 0 ? Math.min(position / duration, 1) * 100 : 0;
  return (
    <section className="recording-player" aria-label="Recording playback">
      <audio
        ref={ref}
        src={src}
        preload="metadata"
        aria-label="Note recording"
        onLoadedMetadata={updateTime}
        onDurationChange={updateTime}
        onTimeUpdate={updateTime}
        onSeeked={updateTime}
        onPlay={() => setPlaying(true)}
        onPlaying={() => {
          setPlaying(true);
          setLoading(false);
          setFailure(null);
        }}
        onWaiting={() => setLoading(true)}
        onCanPlay={() => setLoading(false)}
        onPause={() => {
          setPlaying(false);
          setLoading(false);
        }}
        onEnded={() => {
          setPlaying(false);
          setLoading(false);
          updateTime();
        }}
        onRateChange={() => setRate(ref.current?.playbackRate ?? 1)}
        onError={() => {
          request.current += 1;
          setPlaying(false);
          setLoading(false);
          setFailure("This recording couldn’t be loaded. Try again.");
        }}
      />
      <button
        className="recording-player-toggle"
        type="button"
        aria-label={playing || loading ? "Pause recording" : "Play recording"}
        onClick={toggle}
        disabled={!!failure}
      >
        {loading ? (
          <LoaderCircle size={19} className="recording-player-spinner" />
        ) : playing ? (
          <Pause size={18} fill="currentColor" />
        ) : (
          <Play size={18} fill="currentColor" />
        )}
      </button>
      <div className="recording-player-track">
        <input
          type="range"
          className="recording-player-seek"
          aria-label="Seek recording"
          aria-valuetext={`${time(position)} of ${time(duration)}`}
          min={0}
          max={duration || 1}
          step={0.1}
          value={Math.min(position, duration)}
          disabled={!duration || !!failure}
          style={{ "--recording-progress": `${progress}%` } as CSSProperties}
          onChange={(event) => {
            const audio = ref.current;
            if (!audio || !duration) return;
            const next = Math.max(
              0,
              Math.min(Number(event.target.value), duration),
            );
            audio.currentTime = next;
            setPosition(next);
          }}
        />
        <div className="recording-player-times">
          <span>{time(position)}</span>
          <span>{duration ? time(duration) : "–:––"}</span>
        </div>
      </div>
      <select
        className="recording-player-rate"
        aria-label="Playback speed"
        value={rate}
        onChange={(event) => {
          const next = Number(event.target.value);
          if (ref.current) ref.current.playbackRate = next;
          setRate(next);
        }}
      >
        {[0.5, 0.75, 1, 1.25, 1.5, 1.75, 2].map((speed) => (
          <option key={speed} value={speed}>
            {speed}×
          </option>
        ))}
      </select>
      {loading && (
        <span className="recording-player-status" role="status">
          Loading recording…
        </span>
      )}
      {failure && (
        <div className="recording-player-error">
          <span role="alert">{failure}</span>
          <button
            type="button"
            onClick={() => {
              setFailure(null);
              ref.current?.load();
              void play();
            }}
          >
            <RotateCcw size={15} /> Retry
          </button>
        </div>
      )}
    </section>
  );
}
