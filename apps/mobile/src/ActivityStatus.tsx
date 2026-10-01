import "./ActivityStatus.css";

import { CircleAlert, Clock3, LoaderCircle, Pause, X } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";

import { cn } from "@hypr/utils";

import {
  errorMessage,
  useCommand,
  visibleJobError,
  type Snapshot,
} from "./api";

function activityPhase(snapshot: Snapshot) {
  const running = snapshot.jobs.find((job) => job.state === "running");
  const queued = snapshot.jobs.find((job) => job.state === "queued");
  const paused = snapshot.jobs.find((job) => job.state === "paused");
  const recording = Boolean(snapshot.recording.session_id);

  if (snapshot.recording.stopping) {
    return { text: "Saving recording…", state: "active" } as const;
  }
  if (recording && (running || queued)) {
    return { text: "Processing paused", state: "paused" } as const;
  }
  if (running) {
    return {
      text:
        running.kind === "transcribe"
          ? "Transcribing…"
          : running.kind === "title"
            ? "Generating title…"
            : "Generating summary…",
      state: "active",
    } as const;
  }
  if (snapshot.model.downloading || snapshot.model.phase === "verifying") {
    return {
      text:
        snapshot.model.phase === "verifying"
          ? "Verifying model…"
          : "Downloading model…",
      state: "active",
    } as const;
  }
  if (queued) {
    return {
      text:
        queued.kind === "transcribe"
          ? "Transcription queued"
          : queued.kind === "title"
            ? "Title queued"
            : "Summary queued",
      state: "queued",
    } as const;
  }
  if (paused) {
    return {
      text:
        paused.kind === "transcribe"
          ? "Transcription paused"
          : paused.kind === "title"
            ? "Title paused"
            : "Summary paused",
      state: "paused",
    } as const;
  }
  return { text: "Needs attention", state: "attention" } as const;
}

export function ActivityStatus({
  snapshot,
  onSelectSession,
  onOpenTranscriptionSettings,
  compact = false,
}: {
  snapshot: Snapshot;
  onSelectSession: (id: string) => void;
  onOpenTranscriptionSettings?: () => void;
  compact?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const dismissFailed = useCommand("mobile_dismiss_failed_jobs");
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const modelBusy =
    snapshot.model.downloading || snapshot.model.phase === "verifying";
  const count =
    snapshot.jobs.length +
    Number(modelBusy) +
    Number(Boolean(snapshot.recording.stopping));
  const attentionCount = snapshot.jobs.filter(
    (job) => job.state === "failed",
  ).length;
  const hasRunningJob = snapshot.jobs.some((job) => job.state === "running");
  const hasQueuedJob = snapshot.jobs.some((job) => job.state === "queued");
  const hasPausedJob = snapshot.jobs.some((job) => job.state === "paused");
  const phase = activityPhase(snapshot);

  useEffect(() => {
    if (!open) return;
    panel.current?.focus();
    const dismiss = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);

  useEffect(() => {
    if (!count) setOpen(false);
  }, [count]);

  if (!count) return null;

  const label = `${phase.text}, ${count} background ${count === 1 ? "activity" : "activities"}${
    attentionCount
      ? `, ${attentionCount} ${attentionCount === 1 ? "needs" : "need"} attention`
      : ""
  }`;

  return (
    <div className={cn(["activity-status", compact && "compact"])} ref={root}>
      <span
        className="activity-announcement"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {phase.text}
      </span>
      <button
        ref={trigger}
        className={cn([
          "activity-trigger",
          phase.state === "active" && "activity-active",
          phase.state === "attention" && "activity-attention",
        ])}
        aria-label={label}
        aria-expanded={open}
        aria-controls={panelId}
        aria-haspopup="dialog"
        onClick={() => setOpen(!open)}
      >
        {phase.state === "active" ? (
          <LoaderCircle
            className="activity-spinner"
            size={17}
            aria-hidden="true"
          />
        ) : phase.state === "queued" ? (
          <Clock3 size={17} aria-hidden="true" />
        ) : phase.state === "paused" ? (
          <Pause size={17} aria-hidden="true" />
        ) : (
          <CircleAlert size={17} aria-hidden="true" />
        )}
        {phase.state !== "attention" && (
          <span className="activity-phase" aria-hidden="true">
            {phase.text}
          </span>
        )}
        {attentionCount > 0 && phase.state !== "attention" && (
          <CircleAlert
            className="activity-warning"
            size={16}
            aria-hidden="true"
          />
        )}
        {phase.state !== "attention" && (
          <span className="activity-count" aria-hidden="true">
            {count}
          </span>
        )}
      </button>
      {open && (
        <div
          ref={panel}
          id={panelId}
          className="activity-panel"
          role="dialog"
          aria-label="Background activity"
          tabIndex={-1}
        >
          <div className="activity-heading">
            <strong>Background activity</strong>
            <div className="activity-heading-actions">
              {attentionCount > 0 && (
                <button
                  className="activity-dismiss"
                  disabled={dismissFailed.isPending}
                  onClick={() => dismissFailed.mutate({})}
                >
                  Dismiss all
                </button>
              )}
              <button
                className="activity-close"
                aria-label="Close background activity"
                onClick={() => {
                  setOpen(false);
                  trigger.current?.focus();
                }}
              >
                <X size={18} />
              </button>
            </div>
          </div>
          {dismissFailed.error && (
            <p className="activity-error" role="alert">
              {errorMessage(dismissFailed.error)}
            </p>
          )}
          {snapshot.recording.stopping && (
            <section
              className="activity-job"
              aria-label="Recording finalization"
            >
              <strong>Recording</strong>
              <p className="activity-description">Saving audio…</p>
              <progress aria-label="Recording finalization progress" />
            </section>
          )}
          {snapshot.jobs.length > 0 && (
            <p className="activity-hint">
              {snapshot.recording.stopping
                ? "You can keep using Loofah while the recording is saved."
                : snapshot.recording.session_id
                  ? "Processing waits while you record. You can keep using Loofah."
                  : hasRunningJob
                    ? "You can keep using Loofah while this runs. Processing pauses during recording and when the app is in the background."
                    : hasQueuedJob
                      ? "Queued work starts while Loofah is open. Processing waits during recording and when the app is in the background."
                      : snapshot.jobs.some(
                            (job) =>
                              job.kind === "transcribe" &&
                              (job.state === "failed" ||
                                job.state === "paused"),
                          ) && !snapshot.model.ready
                        ? "Download the transcription model to continue."
                        : hasPausedJob
                          ? "Resume a paused job to continue."
                          : "Retry a failed job to continue."}
            </p>
          )}
          {modelBusy && <ModelActivity model={snapshot.model} />}
          {snapshot.jobs.map((job) => (
            <JobActivity
              key={`${job.session_id}:${job.kind}`}
              job={job}
              title={
                snapshot.sessions.find(
                  (session) => session.id === job.session_id,
                )?.title || "Untitled note"
              }
              onSelect={() => {
                setOpen(false);
                onSelectSession(job.session_id);
              }}
              modelReady={snapshot.model.ready}
              onOpenTranscriptionSettings={
                onOpenTranscriptionSettings
                  ? () => {
                      setOpen(false);
                      onOpenTranscriptionSettings();
                    }
                  : undefined
              }
            />
          ))}
        </div>
      )}
    </div>
  );
}

function JobActivity({
  job,
  title,
  onSelect,
  modelReady,
  onOpenTranscriptionSettings,
}: {
  job: Snapshot["jobs"][number];
  title: string;
  onSelect: () => void;
  modelReady: boolean;
  onOpenTranscriptionSettings?: () => void;
}) {
  const resume = useCommand(
    job.kind === "transcribe"
      ? "mobile_transcribe"
      : job.kind === "title"
        ? "mobile_generate_title"
        : "mobile_summarize",
  );
  const pause = useCommand("mobile_cancel_job");
  const kind =
    job.kind === "transcribe"
      ? "transcription"
      : job.kind === "title"
        ? "title"
        : "summary";
  const state = {
    running:
      job.kind === "transcribe"
        ? "Transcribing"
        : job.kind === "title"
          ? "Generating title"
          : "Summarizing",
    queued:
      job.kind === "transcribe"
        ? "Transcription queued"
        : job.kind === "title"
          ? "Title queued"
          : "Summary queued",
    paused:
      job.kind === "transcribe"
        ? "Transcription paused"
        : job.kind === "title"
          ? "Title paused"
          : "Summary paused",
    failed:
      job.kind === "transcribe"
        ? "Transcription failed"
        : job.kind === "title"
          ? "Title failed"
          : "Summary failed",
  }[job.state];
  const progress =
    job.kind === "transcribe" &&
    Number.isFinite(job.progress) &&
    job.progress > 0
      ? Math.max(0, Math.min(1, job.progress))
      : undefined;
  const resumable = job.state === "paused" || job.state === "failed";
  const action = job.state === "failed" ? "Retry" : "Resume";
  const error = resume.error || pause.error;
  const visibleError = visibleJobError(job, modelReady);
  const needsModel = job.kind === "transcribe" && resumable && !modelReady;

  return (
    <section className="activity-job" aria-label={`${kind} for ${title}`}>
      <button className="activity-note" onClick={onSelect}>
        {title}
      </button>
      <div className="activity-description">
        <span>{state}</span>
        {progress !== undefined && <span>{Math.round(progress * 100)}%</span>}
      </div>
      {(job.state === "running" ||
        (job.kind === "transcribe" && progress !== undefined)) && (
        <progress
          aria-label={`${kind} progress for ${title}`}
          value={progress}
          max={1}
        />
      )}
      {visibleError && <p className="activity-error">{visibleError}</p>}
      {needsModel && onOpenTranscriptionSettings ? (
        <button
          className="activity-action"
          onClick={onOpenTranscriptionSettings}
        >
          Set up transcription
        </button>
      ) : (
        <button
          className="activity-action"
          disabled={resume.isPending || pause.isPending}
          aria-label={`${resumable ? action : "Pause"} ${kind} for ${title}`}
          onClick={() =>
            (resumable ? resume : pause).mutate({ sessionId: job.session_id })
          }
        >
          {resume.isPending || pause.isPending
            ? "Working…"
            : resumable
              ? action
              : "Pause"}
        </button>
      )}
      {error && (
        <p className="activity-error" role="alert">
          {errorMessage(error)}
        </p>
      )}
    </section>
  );
}

function ModelActivity({ model }: { model: Snapshot["model"] }) {
  const verifying = model.phase === "verifying";
  const progress =
    model.total_bytes > 0
      ? Math.max(0, Math.min(1, model.downloaded_bytes / model.total_bytes))
      : undefined;
  return (
    <section className="activity-job" aria-label="Transcription model setup">
      <strong>Transcription model</strong>
      <p className="activity-description">
        {verifying
          ? "Verifying download…"
          : progress === undefined
            ? "Downloading…"
            : `Downloading · ${Math.round(progress * 100)}%`}
      </p>
      <progress
        aria-label="Transcription model download progress"
        value={verifying ? undefined : progress}
        max={1}
      />
    </section>
  );
}
