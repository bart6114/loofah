import "./style.css";

import { useForm, useStore } from "@tanstack/react-form";
import { useMutation, useQuery } from "@tanstack/react-query";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  ChevronRight,
  Download,
  FileAudio,
  Mic,
  Paperclip,
  Plus,
  Settings,
  Square,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { PROVIDERS } from "@hypr/ai-providers";
import { cn } from "@hypr/utils";

import { ActivityStatus } from "./ActivityStatus";
import {
  errorMessage,
  snapshotOptions,
  syncLabel,
  time,
  useCommand,
  useEvents,
  visibleJobError,
  type Session,
  type Snapshot,
} from "./api";
import { AudioPlayer } from "./AudioPlayer";
import { NoteActions } from "./NoteActions";
import { SummarySettings } from "./SummarySettings";
import { Transcript } from "./Transcript";

export { SummarySettings } from "./SummarySettings";

function ErrorText({ error }: { error: unknown }) {
  return error ? (
    <p className="error" role="alert">
      {errorMessage(error)}
    </p>
  ) : null;
}
function CommandButton({
  command,
  args = {},
  children,
  disabled = false,
  className,
}: {
  command: string;
  args?: Record<string, unknown>;
  children: React.ReactNode;
  disabled?: boolean;
  className?: string;
}) {
  const mutation = useCommand(command);
  return (
    <>
      <button
        type="button"
        className={className}
        disabled={disabled || mutation.isPending}
        onClick={() => mutation.mutate(args)}
      >
        {mutation.isPending ? "Working…" : children}
      </button>
      <ErrorText error={mutation.error} />
    </>
  );
}
export function App() {
  useEvents();
  const [deleted, setDeleted] = useState<string | null>(null);
  const restore = useCommand("mobile_restore_session");
  const [selected, setSelected] = useState<string | null>(null);
  const [detailView, setDetailView] = useState<{
    id: string;
    tab: "notes" | "transcript" | "summary";
  } | null>(null);
  const [settings, setSettings] = useState(false);
  const [settingsSection, setSettingsSection] = useState<
    "summaries" | "transcription" | null
  >(null);
  const [dirty, setDirty] = useState(false);
  const [navigation, setNavigation] = useState<{ action: () => void } | null>(
    null,
  );
  const saveCurrentNote = useRef<(() => Promise<void>) | null>(null);
  const registerNoteSave = useCallback((save: (() => Promise<void>) | null) => {
    saveCurrentNote.current = save;
  }, []);
  const leave = (action: () => void) => {
    if (saveCurrentNote.current) {
      void saveCurrentNote
        .current()
        .then(action, () => setNavigation({ action }));
    } else if (dirty) setNavigation({ action });
    else action();
  };
  const openSettings = (section: "summaries" | "transcription" | null) =>
    leave(() => {
      setSettingsSection(section);
      setSettings(true);
    });
  const snapshot = useQuery({
    ...snapshotOptions,
    refetchInterval: (query) => {
      const s = query.state.data;
      return s &&
        (s.recording.session_id ||
          s.model.downloading ||
          s.model.phase === "verifying" ||
          s.jobs.some((j) => j.state === "running" || j.state === "queued") ||
          s.sync.pending > 0)
        ? 2000
        : false;
    },
  });
  const create = useCommand<string>("mobile_create_session");
  const imported = useCommand<string | null>("mobile_import_audio");
  const [search, setSearch] = useState("");
  const data = snapshot.data;
  const hasSettingsContent = !!data;
  useEffect(() => {
    if (settings && settingsSection) {
      if (!hasSettingsContent) return;
      document
        .getElementById(
          settingsSection === "summaries"
            ? "summary-settings"
            : "transcription-settings",
        )
        ?.scrollIntoView();
    } else window.scrollTo(0, 0);
  }, [hasSettingsContent, selected, settings, settingsSection]);
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;
    const update = () =>
      document.documentElement.style.setProperty(
        "--visual-viewport-top",
        `${viewport.offsetTop}px`,
      );
    update();
    viewport.addEventListener("scroll", update);
    viewport.addEventListener("resize", update);
    window.addEventListener("scroll", update, { passive: true });
    return () => {
      viewport.removeEventListener("scroll", update);
      viewport.removeEventListener("resize", update);
      window.removeEventListener("scroll", update);
      document.documentElement.style.removeProperty("--visual-viewport-top");
    };
  }, []);
  const home = !settings && !selected;
  const sessions =
    data?.sessions.filter((s) =>
      (s.title || "Untitled note")
        .toLocaleLowerCase()
        .includes(search.trim().toLocaleLowerCase()),
    ) ?? [];
  return (
    <div
      className={cn([
        "app",
        (data?.recording.session_id || home) && "has-capture",
        data?.recording.session_id && "has-active-capture",
      ])}
    >
      <header>
        <button
          className="icon back-button"
          aria-label={settings && selected ? "Back to note" : "Back to notes"}
          hidden={!selected && !settings}
          onClick={() =>
            leave(() => {
              if (settings) setSettings(false);
              else setSelected(null);
            })
          }
        >
          <ArrowLeft size={22} />
          <span>{settings && selected ? "Note" : "Notes"}</span>
        </button>
        {selected && !settings ? (
          <div className="navigation-title" />
        ) : (
          <h1 className={cn(["navigation-title", home && "home-title"])}>
            {settings ? "Settings" : "loofah"}
          </h1>
        )}
        {home && (
          <button
            className="icon primary new-note-button"
            aria-label="New note"
            disabled={create.isPending}
            onClick={() => create.mutate({}, { onSuccess: setSelected })}
          >
            <Plus />
          </button>
        )}
        <button
          className="icon"
          aria-label="Settings"
          hidden={settings}
          onClick={() => openSettings(null)}
        >
          <Settings size={22} />
        </button>
        {data && (
          <ActivityStatus
            snapshot={data}
            compact={home}
            onOpenTranscriptionSettings={() => openSettings("transcription")}
            onSelectSession={(id) =>
              selected === id && !settings
                ? undefined
                : leave(() => {
                    setSelected(id);
                    setSettings(false);
                  })
            }
          />
        )}
      </header>
      <main>
        {data?.startup_errors?.map((error, i) => (
          <ErrorText key={i} error={error} />
        ))}
        {!data ? (
          <div className="empty">
            <h1>Your conversations, kept close.</h1>
            {snapshot.isPending ? (
              <p>Opening your vault…</p>
            ) : (
              <>
                <ErrorText error={snapshot.error} />
                <button onClick={() => void snapshot.refetch()}>
                  Try again
                </button>
              </>
            )}
          </div>
        ) : settings ? (
          <Setup snapshot={data} onDirty={setDirty} />
        ) : selected ? (
          <Detail
            key={selected}
            id={selected}
            snapshot={data}
            onDirty={setDirty}
            onSaveReady={registerNoteSave}
            onOpenSettings={openSettings}
            initialView={
              detailView?.id === selected ? detailView.tab : undefined
            }
            onViewChange={(tab) => setDetailView({ id: selected, tab })}
            onDeleted={() => {
              setDeleted(selected);
              setDirty(false);
              setSelected(null);
              restore.reset();
            }}
          />
        ) : (
          <>
            <ErrorText error={create.error} />
            {(!data.model.ready ||
              data.sync.error ||
              data.recording.interrupted) && (
              <button
                className={cn([
                  "notice",
                  !data.recording.interrupted &&
                    !data.sync.error &&
                    "model-setup-link",
                ])}
                onClick={() => {
                  if (data.recording.interrupted && data.recording.session_id)
                    setSelected(data.recording.session_id);
                  else openSettings(data.model.ready ? null : "transcription");
                }}
              >
                {data.recording.interrupted
                  ? "Recording was interrupted. Open the recording to review its status."
                  : data.sync.error
                    ? "iCloud needs attention"
                    : "Set up on-device transcription"}
                <ChevronRight size={18} />
              </button>
            )}
            <input
              className="search"
              type="search"
              aria-label="Search note titles"
              placeholder="Search note titles"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
            <div className="session-list">
              {sessions.map((s) => (
                <button
                  className="session-row"
                  key={s.id}
                  onClick={() => setSelected(s.id)}
                >
                  <span>
                    <strong>{s.title || "Untitled note"}</strong>
                    <small>
                      {new Date(s.created_at).toLocaleDateString(undefined, {
                        day: "numeric",
                        month: "short",
                      })}
                      {s.has_transcript_words && " · Transcribed"}
                    </small>
                  </span>
                  <ChevronRight size={18} />
                </button>
              ))}
            </div>
            {!data.sessions.length && (
              <div className="empty">
                <h2>A little space to think.</h2>
                <p>Write a note or record a conversation.</p>
              </div>
            )}
            {data.sessions.length > 0 && sessions.length === 0 && (
              <div className="empty" role="status">
                <h2>No matching notes</h2>
                <p>Try another title or clear your search.</p>
                <button onClick={() => setSearch("")}>Clear search</button>
              </div>
            )}
            <button
              className="import-link"
              disabled={imported.isPending || !!data.recording.session_id}
              onClick={() =>
                imported.mutate(
                  {},
                  { onSuccess: (id) => id && setSelected(id) },
                )
              }
            >
              <FileAudio size={18} />
              {imported.isPending ? "Importing…" : "Import audio"}
            </button>
            <ErrorText error={imported.error} />
            {data.recording.session_id && (
              <p className="caption">
                Finish recording before importing audio.
              </p>
            )}
          </>
        )}
      </main>
      {data && (data.recording.session_id || (!settings && !selected)) && (
        <Recording
          snapshot={data}
          selected={settings ? null : selected}
          select={(id) =>
            selected === id && !settings
              ? undefined
              : leave(() => {
                  setSelected(id);
                  setSettings(false);
                })
          }
        />
      )}
      {deleted && (
        <aside className="undo-banner" aria-label="Deleted note">
          <span role="status">Note moved to trash</span>
          <button
            disabled={restore.isPending}
            onClick={() =>
              restore.mutate(
                { sessionId: deleted },
                {
                  onSuccess: () => {
                    setDeleted(null);
                  },
                },
              )
            }
          >
            {restore.isPending ? "Restoring…" : "Undo"}
          </button>
          <button
            className="icon"
            aria-label="Dismiss undo"
            disabled={restore.isPending}
            onClick={() => setDeleted(null)}
          >
            ×
          </button>
          <ErrorText error={restore.error} />
        </aside>
      )}
      {navigation && (
        <DiscardChanges
          cancel={() => setNavigation(null)}
          discard={() => {
            setDirty(false);
            setNavigation(null);
            navigation.action();
          }}
        />
      )}
    </div>
  );
}
function DiscardChanges({
  cancel,
  discard,
}: {
  cancel: () => void;
  discard: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  return (
    <dialog
      ref={dialog}
      className="discard-dialog"
      aria-labelledby="discard-title"
      aria-describedby="discard-description"
      onCancel={(event) => {
        event.preventDefault();
        cancel();
      }}
    >
      <h2 id="discard-title">Keep your changes?</h2>
      <p id="discard-description">
        Your latest changes haven’t been saved. Stay here to save them, or
        discard them.
      </p>
      <div className="dialog-actions">
        <button autoFocus className="primary" onClick={cancel}>
          Keep editing
        </button>
        <button onClick={discard}>Discard changes</button>
      </div>
    </dialog>
  );
}
function Recording({
  snapshot,
  selected,
  select,
}: {
  snapshot: Snapshot;
  selected: string | null;
  select: (id: string) => void;
}) {
  const start = useCommand("mobile_start_recording");
  const stop = useCommand("mobile_stop_recording");
  const create = useCommand<string>("mobile_create_session");
  const recording = snapshot.recording;
  const active = !!recording.session_id && !recording.stopping;
  const begin = async () => {
    if (selected) start.mutate({ sessionId: selected });
    else {
      try {
        const id = await create.mutateAsync({});
        select(id);
        start.mutate({ sessionId: id });
      } catch {
        /* Mutation renders the failure. */
      }
    }
  };
  return (
    <div
      className={cn([
        "recording-bar",
        active && "is-recording",
        !active && !recording.stopping && "idle-recording",
      ])}
    >
      <ErrorText
        error={start.error || stop.error || create.error || recording.error}
      />
      <div className="recording-inner">
        <div>
          {active ? (
            <button
              type="button"
              className="recording-title"
              onClick={() => select(recording.session_id!)}
            >
              <span className="recording-dot" />
              {recording.interrupted ? "Interrupted" : "Recording"}
              <b>{time(recording.elapsed_seconds)}</b>
            </button>
          ) : (
            <span className="recording-hint">
              {recording.stopping
                ? "Finishing recording…"
                : "Capture a conversation"}
            </span>
          )}
        </div>
        <button
          className={cn(["record-button", active && "stop"])}
          type="button"
          disabled={
            start.isPending ||
            stop.isPending ||
            create.isPending ||
            recording.stopping
          }
          onClick={() => (active ? stop.mutate({}) : void begin())}
        >
          {active ? (
            <>
              <Square size={17} fill="currentColor" />
              {stop.isPending ? "Saving…" : "Stop"}
            </>
          ) : recording.stopping ? (
            "Saving…"
          ) : (
            <>
              <Mic size={19} />
              {start.isPending || create.isPending ? "Starting…" : "Record"}
            </>
          )}
        </button>
      </div>
    </div>
  );
}
export function Detail({
  id,
  snapshot,
  onDirty,
  onSaveReady,
  onOpenSettings,
  initialView,
  onViewChange,
  onDeleted = () => {},
}: {
  id: string;
  snapshot: Snapshot;
  onDeleted?: () => void;
  onSaveReady?: (save: (() => Promise<void>) | null) => void;
  onOpenSettings?: (section: "summaries" | "transcription") => void;
  initialView?: "notes" | "transcript" | "summary";
  onViewChange?: (view: "notes" | "transcript" | "summary") => void;
  onDirty: (dirty: boolean) => void;
}) {
  const session = useQuery({
    queryKey: ["mobile", "session", id],
    queryFn: () => invoke<Session>("mobile_session", { sessionId: id }),
  });
  const activeJob = snapshot.jobs.find(
    (job) =>
      job.session_id === id &&
      (job.state === "queued" || job.state === "running"),
  );
  const job = activeJob ?? snapshot.jobs.find((job) => job.session_id === id);
  const finishingRecording =
    snapshot.recording.session_id === id && !!snapshot.recording.stopping;
  const jobPhase = finishingRecording
    ? "transcript"
    : activeJob
      ? activeJob.kind === "transcribe"
        ? "transcript"
        : "summary"
      : null;
  const [tab, setTab] = useState<"notes" | "transcript" | "summary">(
    initialView ?? jobPhase ?? "notes",
  );
  const [attachmentsOpen, setAttachmentsOpen] = useState(false);
  const initialized = useRef(false);
  const lastPhase = useRef(jobPhase);
  const sawTranscription = useRef(jobPhase === "transcript");
  const lastSummary = useRef<string | null>(null);
  const [notesDirty, setNotesDirty] = useState(false);
  const handleDirty = useCallback(
    (dirty: boolean) => {
      setNotesDirty(dirty);
      onDirty(dirty);
    },
    [onDirty],
  );
  const openAttachment = useCommand("mobile_open_attachment");
  const audio = useRef<HTMLAudioElement>(null);
  useEffect(() => {
    if (!session.data) return;
    const summaryArrived =
      initialized.current &&
      sawTranscription.current &&
      !!session.data.summary &&
      session.data.summary !== lastSummary.current;
    if (jobPhase === "transcript") sawTranscription.current = true;
    else if (jobPhase === "summary") sawTranscription.current = false;
    const phase = jobPhase ?? (summaryArrived ? "summary" : null);

    if (!initialized.current) {
      initialized.current = true;
      if (!initialView) {
        if (jobPhase && !notesDirty) setTab(jobPhase);
        else if (session.data.summary) setTab("summary");
      }
    } else if (phase && phase !== lastPhase.current && !notesDirty) {
      setTab(phase);
    }
    if (phase) lastPhase.current = phase;
    else if (!job) lastPhase.current = null;
    if (summaryArrived || (job?.state === "failed" && job.kind !== "title")) {
      sawTranscription.current = false;
    }
    lastSummary.current = session.data.summary;
  }, [initialView, job?.kind, job?.state, jobPhase, notesDirty, session.data]);
  if (!session.data)
    return (
      <>
        {session.isPending ? (
          <p role="status">Opening note…</p>
        ) : (
          <button onClick={() => void session.refetch()}>
            Try opening note again
          </button>
        )}
        <ErrorText error={session.error} />
      </>
    );
  const data = session.data;
  const recording =
    snapshot.recording.session_id === id && !snapshot.recording.stopping;
  const hasAudio = data.has_audio || !!data.audio_url;
  const canShowTranscript =
    !!data.transcript.length ||
    hasAudio ||
    recording ||
    finishingRecording ||
    job?.kind === "transcribe";
  const tabs: ("summary" | "notes" | "transcript")[] = canShowTranscript
    ? (["notes", "transcript", "summary"] as const)
    : (["notes", "summary"] as const);
  const activeTab = tab === "transcript" && !canShowTranscript ? "notes" : tab;
  return (
    <Editor
      session={data}
      onDirty={handleDirty}
      onSaveReady={onSaveReady}
      showNotes={activeTab === "notes"}
      actions={(notes) => (
        <NoteActions
          session={data}
          notes={notes}
          dirty={notesDirty}
          recording={recording}
          processing={snapshot.jobs.some(
            (job) => job.session_id === id && job.state === "running",
          )}
          onDeleted={onDeleted}
        />
      )}
    >
      <div className="session-controls">
        <div className="tabs" role="tablist" aria-label="Note views">
          {tabs.map((view) => (
            <button
              type="button"
              key={view}
              role="tab"
              id={`tab-${view}`}
              aria-controls={`panel-${view}`}
              aria-selected={activeTab === view}
              tabIndex={activeTab === view ? 0 : -1}
              className={cn([activeTab === view && "selected"])}
              onClick={() => {
                setTab(view);
                onViewChange?.(view);
              }}
              onKeyDown={(event) => {
                const index = tabs.indexOf(view);
                const next =
                  event.key === "ArrowRight"
                    ? (index + 1) % tabs.length
                    : event.key === "ArrowLeft"
                      ? (index + tabs.length - 1) % tabs.length
                      : event.key === "Home"
                        ? 0
                        : event.key === "End"
                          ? tabs.length - 1
                          : null;
                if (next !== null) {
                  event.preventDefault();
                  setTab(tabs[next]);
                  onViewChange?.(tabs[next]);
                  event.currentTarget.parentElement
                    ?.querySelectorAll("button")
                    [next]?.focus();
                }
              }}
            >
              {view === "notes"
                ? "Note"
                : view === "summary"
                  ? "Summary"
                  : "Transcript"}
            </button>
          ))}
        </div>
        {!!data.attachments?.length && (
          <button
            type="button"
            className={cn([
              "icon attachment-toggle",
              attachmentsOpen && "selected",
            ])}
            aria-label="Attachments"
            aria-expanded={attachmentsOpen}
            aria-controls="session-attachments"
            onClick={() => setAttachmentsOpen(!attachmentsOpen)}
          >
            <Paperclip size={18} />
          </button>
        )}
      </div>
      {!snapshot.recording.session_id &&
        !hasAudio &&
        !data.transcript.length && (
          <Recording snapshot={snapshot} selected={id} select={() => {}} />
        )}
      {data.audio_url && !snapshot.recording.session_id && (
        <AudioPlayer
          audioRef={audio}
          src={
            /^(https?:|asset:)/.test(data.audio_url)
              ? data.audio_url
              : convertFileSrc(data.audio_url)
          }
        />
      )}
      {!data.audio_url && hasAudio && snapshot.sync.connected && !recording && (
        <CommandButton command="mobile_download_audio" args={{ sessionId: id }}>
          <Download size={16} />
          Download audio
        </CommandButton>
      )}
      {attachmentsOpen && !!data.attachments?.length && (
        <section
          id="session-attachments"
          className="attachments-panel"
          aria-label="Note attachments"
        >
          <ul>
            {data.attachments.map((attachment) => (
              <li key={attachment.relative_path}>
                <button
                  type="button"
                  className="attachment-open"
                  disabled={!attachment.url || openAttachment.isPending}
                  onClick={() =>
                    openAttachment.mutate({
                      sessionId: id,
                      relativePath: attachment.relative_path,
                    })
                  }
                >
                  {attachment.name}
                </button>
              </li>
            ))}
          </ul>
          <ErrorText error={openAttachment.error} />
        </section>
      )}
      {activeTab === "transcript" && (
        <div
          id="panel-transcript"
          role="tabpanel"
          aria-labelledby="tab-transcript"
        >
          {data.transcript.length ? (
            <Transcript
              words={data.transcript}
              onSeek={
                data.audio_url && !snapshot.recording.session_id
                  ? (seconds) => {
                      if (audio.current) {
                        audio.current.currentTime = seconds;
                        void audio.current.play().catch(() => {});
                      }
                    }
                  : undefined
              }
            />
          ) : !snapshot.model.ready &&
            !recording &&
            !finishingRecording ? null : (
            <p className="muted">
              {activeJob?.kind === "transcribe"
                ? activeJob.state === "queued"
                  ? "Transcription queued…"
                  : "Transcribing…"
                : recording
                  ? "Your transcript will be ready after recording."
                  : finishingRecording
                    ? "Finishing recording…"
                    : job?.kind === "transcribe" && job.state === "failed"
                      ? "Transcription failed."
                      : "No transcript yet."}
            </p>
          )}
          {job?.kind === "transcribe" && job.state === "failed" && (
            <ErrorText error={visibleJobError(job, snapshot.model.ready)} />
          )}
          {!snapshot.model.ready &&
            !data.transcript.length &&
            !recording &&
            !finishingRecording && (
              <>
                {!(job?.kind === "transcribe" && job.state === "failed") && (
                  <p className="muted">
                    Download the on-device model to transcribe this recording.
                  </p>
                )}
                {onOpenSettings && (
                  <button
                    type="button"
                    onClick={() => onOpenSettings("transcription")}
                  >
                    Set up transcription
                  </button>
                )}
              </>
            )}
          {!activeJob &&
            !recording &&
            !finishingRecording &&
            snapshot.model.ready &&
            data.audio_url && (
              <CommandButton
                command="mobile_transcribe"
                args={{ sessionId: id }}
              >
                {data.transcript.length
                  ? "Transcribe again"
                  : job?.kind === "transcribe" && job.state === "failed"
                    ? "Retry transcription"
                    : job?.kind === "transcribe" && job.state === "paused"
                      ? "Resume transcription"
                      : "Transcribe"}
              </CommandButton>
            )}
        </div>
      )}
      {tab === "summary" && (
        <div id="panel-summary" role="tabpanel" aria-labelledby="tab-summary">
          {data.summary ? (
            <article className="prose">
              <Markdown remarkPlugins={[remarkGfm]}>{data.summary}</Markdown>
            </article>
          ) : (
            <p className="muted">
              {activeJob?.kind === "summary" || activeJob?.kind === "title"
                ? activeJob.state === "queued"
                  ? "Summary queued…"
                  : activeJob.kind === "title"
                    ? "Generating title…"
                    : "Generating summary…"
                : job?.kind === "summary" && job.state === "failed"
                  ? "Summary failed."
                  : job?.kind === "summary" && job.state === "paused"
                    ? "Summary paused."
                    : "No summary yet."}
            </p>
          )}
          {job?.kind === "summary" && job.state === "failed" && (
            <ErrorText error={job.error} />
          )}
          {!activeJob && (
            <CommandButton
              command="mobile_summarize"
              args={{ sessionId: id }}
              disabled={
                !snapshot.summary.available ||
                notesDirty ||
                (!data.notes.trim() && !data.transcript.length)
              }
            >
              {data.summary
                ? "Regenerate summary"
                : job?.kind === "summary" && job.state === "failed"
                  ? "Retry summary"
                  : job?.kind === "summary" && job.state === "paused"
                    ? "Resume summary"
                    : "Create summary"}
            </CommandButton>
          )}
          {notesDirty ? (
            <p className="muted">Save your notes before creating a summary.</p>
          ) : !data.notes.trim() && !data.transcript.length ? (
            <p className="muted">
              Add a note or transcript to create a summary.
            </p>
          ) : null}
          {!snapshot.summary.available && (
            <>
              <p className="muted">
                {snapshot.summary.reason || "Configure summaries in settings."}
              </p>
              {onOpenSettings && (
                <button
                  type="button"
                  onClick={() => onOpenSettings("summaries")}
                >
                  Set up summaries
                </button>
              )}
            </>
          )}
          {snapshot.summary.available &&
            snapshot.settings.summary_provider !== "none" && (
              <p className="caption">
                This sends your notes and transcript to your selected{" "}
                {PROVIDERS.find(
                  (provider) =>
                    provider.id === snapshot.settings.summary_provider,
                )?.displayName || snapshot.settings.summary_provider}{" "}
                model.
              </p>
            )}
        </div>
      )}
    </Editor>
  );
}
export function Editor({
  session,
  onDirty,
  onSaveReady,
  showNotes = true,
  children,
  actions,
}: {
  session: Session;
  showNotes?: boolean;
  children?: React.ReactNode;
  actions?: (notes: string) => React.ReactNode;
  onDirty: (dirty: boolean) => void;
  onSaveReady?: (save: (() => Promise<void>) | null) => void;
}) {
  const save = useCommand("mobile_update_session");
  const baseline = useRef({
    title: session.title,
    notes: session.notes,
  });
  const saveInFlight = useRef<Promise<void> | null>(null);
  const form = useForm({
    defaultValues: baseline.current,
    onSubmit: async ({ value }) => {
      await save.mutateAsync({
        sessionId: session.id,
        ...value,
        expectedTitle: baseline.current.title,
        expectedNotes: baseline.current.notes,
      });
      baseline.current = value;
      const latest = form.state.values;
      if (latest.title === value.title && latest.notes === value.notes)
        form.reset(value);
    },
  });
  const notesField = useRef<HTMLTextAreaElement>(null);
  const notesValue = useStore(form.store, (state) => state.values.notes);
  const isDirty = useStore(form.store, (state) => state.isDirty);
  const isSubmitting = useStore(form.store, (state) => state.isSubmitting);
  const commit = useCallback(async () => {
    while (saveInFlight.current) await saveInFlight.current;
    if (!form.state.isDirty) return;
    const task = form.handleSubmit();
    saveInFlight.current = task;
    try {
      await task;
    } finally {
      if (saveInFlight.current === task) saveInFlight.current = null;
    }
  }, [form]);
  useEffect(() => {
    onSaveReady?.(commit);
    return () => onSaveReady?.(null);
  }, [commit, onSaveReady]);
  useLayoutEffect(() => {
    const resize = () => {
      for (const field of [showNotes ? notesField.current : null]) {
        if (field) {
          field.style.height = "auto";
          field.style.height = `${field.scrollHeight}px`;
        }
      }
    };
    resize();
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, [isDirty, notesValue, showNotes]);
  useEffect(() => {
    onDirty(isDirty);
    return () => onDirty(false);
  }, [isDirty, onDirty]);
  useEffect(() => {
    if (!isDirty) {
      const current = { title: session.title, notes: session.notes };
      baseline.current = current;
      form.reset(current);
    }
  }, [session.title, session.notes]);
  return (
    <>
      <div className="note-title-row">
        <form
          id={`note-form-${session.id}`}
          onSubmit={(e) => {
            e.preventDefault();
            e.stopPropagation();
            void commit().catch(() => {});
          }}
        >
          <form.Field name="title">
            {(field) => (
              <input
                type="text"
                aria-label="Note title"
                className="title-input"
                value={field.state.value}
                placeholder="Untitled note"
                onChange={(e) => field.handleChange(e.target.value)}
                onBlur={() => {
                  field.handleBlur();
                  void commit().catch(() => {});
                }}
              />
            )}
          </form.Field>
        </form>
        {actions?.(notesValue)}
      </div>
      {children}
      <div
        hidden={!showNotes}
        id="panel-notes"
        role="tabpanel"
        aria-labelledby="tab-notes"
      >
        <form.Field name="notes">
          {(field) => (
            <textarea
              ref={notesField}
              form={`note-form-${session.id}`}
              aria-label="Notes"
              className="notes-input"
              value={field.state.value}
              placeholder="A thought, a question, something to remember…"
              onChange={(e) => field.handleChange(e.target.value)}
              onBlur={() => {
                field.handleBlur();
                void commit().catch(() => {});
              }}
            />
          )}
        </form.Field>
      </div>
      <ErrorText error={save.error} />
      {save.error && (
        <button
          type="button"
          onClick={() => void commit().catch(() => {})}
          disabled={isSubmitting}
        >
          Retry save
        </button>
      )}
    </>
  );
}
export function Setup({
  snapshot,
  onDirty,
}: {
  snapshot: Snapshot;
  onDirty?: (dirty: boolean) => void;
}) {
  const download = useCommand("mobile_download_model");
  const select = useCommand("mobile_select_model");
  const remove = useCommand("mobile_delete_model");
  const privacyPolicy = useMutation({
    mutationFn: () => openUrl("https://loofah.io/privacy-policy"),
  });
  const progressSnapshot = useQuery({
    ...snapshotOptions,
    enabled: download.isPending,
    refetchInterval: download.isPending ? 500 : false,
  });
  const current = download.isPending
    ? (progressSnapshot.data ?? snapshot)
    : snapshot;
  const model = current.model;
  const downloading =
    download.isPending || model.downloading || model.phase === "verifying";
  const busy =
    downloading ||
    select.isPending ||
    remove.isPending ||
    !!current.recording.session_id ||
    current.jobs.some(
      (job) => job.state === "queued" || job.state === "running",
    );
  const { downloaded_bytes: downloaded, total_bytes: total } = model;
  const percent =
    total > 0 ? Math.min(100, Math.floor((downloaded / total) * 100)) : null;
  const megabytes = (bytes: number) =>
    (bytes / 1_000_000).toLocaleString(undefined, { maximumFractionDigits: 1 });
  const downloadStatus =
    model.phase === "verifying"
      ? "Verifying downloaded model…"
      : percent === null
        ? `${megabytes(downloaded)} MB downloaded`
        : `${percent}% · ${megabytes(downloaded)} / ${megabytes(total)} MB`;
  return (
    <>
      <section className="card">
        <h2>Notes &amp; recordings</h2>
        <div className="vault-location">
          <strong>On this iPhone</strong>
          <span>Saved as files in Loofah app storage</span>
        </div>
        {snapshot.vault.icloud_path && (
          <p className="caption">
            A local copy stays on this iPhone for offline access.
          </p>
        )}
        <details className="vault-path">
          <summary>Show folder path</summary>
          <p>{snapshot.vault.local_path}</p>
        </details>
        <h3>iCloud sync</h3>
        {snapshot.vault.icloud_path ? (
          <>
            <div className="vault-location">
              <strong>
                {snapshot.vault.icloud_path.split("/").filter(Boolean).at(-1)}
              </strong>
              <span>iCloud Drive</span>
            </div>
            <details className="vault-path">
              <summary>Show iCloud folder path</summary>
              <p>{snapshot.vault.icloud_path}</p>
            </details>
            <p className="status">
              {snapshot.sync.connected
                ? syncLabel(snapshot.sync.state)
                : "Sync unavailable · changes are saved on this iPhone"}
              {snapshot.sync.pending > 0
                ? ` · ${snapshot.sync.pending} pending`
                : ""}
            </p>
          </>
        ) : (
          <p className="muted">Off · saved only on this iPhone</p>
        )}
        <p className="caption">
          {snapshot.vault.icloud_path
            ? "Notes, transcripts, summaries and recordings sync with this folder. Audio downloads when needed."
            : "Connect the same iCloud Drive folder as your Mac to sync notes and recordings."}
        </p>
        <ErrorText error={snapshot.sync.error} />
        <CommandButton command="mobile_connect_vault" disabled={busy}>
          {snapshot.vault.icloud_path
            ? "Reconnect iCloud folder"
            : "Connect iCloud folder"}
        </CommandButton>
        {snapshot.sync.connected && (
          <CommandButton command="mobile_sync" disabled={busy}>
            Sync now
          </CommandButton>
        )}
        {busy && (
          <p className="caption">
            Finish the current activity before changing iCloud sync.
          </p>
        )}
        {snapshot.sync.conflicts.map((c) => (
          <div className="conflict" key={c.id}>
            <strong>Conflicting changes</strong>
            <p>{c.path}</p>
            <p className="caption">
              Choose which version to keep, or keep a copy of both.
            </p>
            {(["local", "remote", "both"] as const).map((resolution) => (
              <CommandButton
                key={resolution}
                command="mobile_resolve_conflict"
                args={{ conflictId: c.id, resolution }}
                disabled={busy}
              >
                {resolution === "local"
                  ? "Keep iPhone"
                  : resolution === "remote"
                    ? "Keep iCloud"
                    : "Keep both"}
              </CommandButton>
            ))}
          </div>
        ))}
      </section>
      <section id="transcription-settings" className="card">
        <h2>On-device transcription</h2>
        <p>
          Transcription runs on this iPhone. Download a model once to start.
        </p>
        <label>
          Transcription model
          <select
            value={current.settings.transcription_model}
            disabled={busy}
            onChange={(event) => select.mutate({ modelId: event.target.value })}
          >
            <option value="parakeet-v3">Parakeet v3 — Multilingual</option>
            <option value="parakeet-v2">Parakeet v2 — English only</option>
          </select>
        </label>
        {current.recording.session_id ? (
          <p className="caption">
            Stop recording to change the transcription model.
          </p>
        ) : current.jobs.some(
            (job) => job.state === "queued" || job.state === "running",
          ) ? (
          <p className="caption">
            Wait for processing to finish before changing the transcription
            model.
          </p>
        ) : select.isPending ? (
          <p className="caption" role="status">
            Selecting model…
          </p>
        ) : null}
        <ErrorText error={select.error} />
        {total > 0 && (
          <p className="caption">Download size: {megabytes(total)} MB</p>
        )}
        <button
          disabled={model.ready || busy}
          onClick={() => download.mutate({})}
        >
          {downloading
            ? model.phase === "verifying"
              ? "Verifying model…"
              : "Downloading model…"
            : model.ready
              ? "Model ready"
              : download.isError
                ? "Retry download"
                : "Download model"}
        </button>
        <ErrorText error={download.error} />
        {downloading && (
          <div className="model-progress">
            <progress
              aria-label="Model download"
              aria-valuetext={downloadStatus}
              max={total > 0 ? total : undefined}
              value={
                total > 0 && model.phase !== "verifying"
                  ? Math.min(downloaded, total)
                  : undefined
              }
            />
            <p className="caption" role="status">
              {downloadStatus}
            </p>
          </div>
        )}
        {model.ready && (
          <>
            <button disabled={busy} onClick={() => remove.mutate({})}>
              {remove.isPending ? "Deleting model…" : "Delete downloaded model"}
            </button>
            <ErrorText error={remove.error} />
          </>
        )}
      </section>
      <SummarySettings snapshot={snapshot} onDirty={onDirty} />
      <section className="card">
        <h2>Privacy</h2>
        <p>
          Your recordings and notes stay on this iPhone unless you choose iCloud
          sync or a summary provider.
        </p>
        <button type="button" onClick={() => privacyPolicy.mutate()}>
          Read privacy policy
        </button>
        <ErrorText error={privacyPolicy.error} />
      </section>
    </>
  );
}
