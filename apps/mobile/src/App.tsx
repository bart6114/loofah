import "./style.css";

import { useForm, useStore } from "@tanstack/react-form";
import { useMutation, useQuery } from "@tanstack/react-query";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  ChevronRight,
  Cloud,
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
  const [settings, setSettings] = useState(false);
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [selected, settings]);
  const [dirty, setDirty] = useState(false);
  const [navigation, setNavigation] = useState<{ action: () => void } | null>(
    null,
  );
  const leave = (action: () => void) => {
    if (dirty) setNavigation({ action });
    else action();
  };
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
  const imported = useCommand<string>("mobile_import_audio");
  const [search, setSearch] = useState("");
  const data = snapshot.data;
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
        (data?.recording.session_id || (!settings && !selected)) &&
          "has-capture",
      ])}
    >
      <header>
        <button
          className="icon"
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
        </button>
        <div
          className={cn([
            "navigation-title",
            !selected && !settings && "brand",
          ])}
        >
          {settings ? "Settings" : selected ? "" : "loofah"}
        </div>
        <button
          className="icon"
          aria-label="Settings"
          hidden={settings}
          onClick={() => leave(() => setSettings(true))}
        >
          <Settings size={22} />
        </button>
        {data && (
          <ActivityStatus
            snapshot={data}
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
            onDeleted={() => {
              setDeleted(selected);
              setDirty(false);
              setSelected(null);
              restore.reset();
            }}
          />
        ) : (
          <>
            <div className="page-heading">
              <div>
                <h1>Notes</h1>
                <p>Your notes and conversations</p>
              </div>
              <button
                className="icon primary"
                aria-label="New note"
                disabled={create.isPending}
                onClick={() => create.mutate({}, { onSuccess: setSelected })}
              >
                <Plus />
              </button>
            </div>
            <ErrorText error={create.error} />
            <ErrorText error={imported.error} />
            {(!data.model.ready ||
              data.sync.error ||
              data.recording.interrupted) && (
              <button
                className="notice"
                onClick={() => {
                  if (data.recording.interrupted && data.recording.session_id)
                    setSelected(data.recording.session_id);
                  else setSettings(true);
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
              placeholder="Find a note"
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
                      })}{" "}
                      · {s.has_transcript_words ? "Transcribed" : "Note"}
                    </small>
                  </span>
                  <ChevronRight size={18} />
                </button>
              ))}
            </div>
            {!data.sessions.length && (
              <div className="empty">
                <h2>A little space to think.</h2>
                <p>Create a note, record a conversation, or import audio.</p>
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
              className="wide subtle"
              disabled={imported.isPending}
              onClick={() => imported.mutate({}, { onSuccess: setSelected })}
            >
              <FileAudio size={18} />
              {imported.isPending ? "Importing…" : "Import audio"}
            </button>
            <button className="sync-link" onClick={() => setSettings(true)}>
              <Cloud size={16} />
              {data.sync.connected
                ? `${syncLabel(data.sync.state)}${data.sync.pending ? ` · ${data.sync.pending} pending` : ""}`
                : data.vault.icloud_path
                  ? "Saved on this iPhone · iCloud needs attention"
                  : "Saved on this iPhone"}
            </button>
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
  inline = false,
}: {
  snapshot: Snapshot;
  inline?: boolean;
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
        inline ? "recording-inline" : "recording-bar",
        active && "is-recording",
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
  onDeleted = () => {},
}: {
  id: string;
  snapshot: Snapshot;
  onDeleted?: () => void;
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
    jobPhase ?? "notes",
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
      if (jobPhase && !notesDirty) setTab(jobPhase);
      else if (session.data.summary) setTab("summary");
    } else if (phase && phase !== lastPhase.current && !notesDirty) {
      setTab(phase);
    }
    if (phase) lastPhase.current = phase;
    else if (!job) lastPhase.current = null;
    if (summaryArrived || (job?.state === "failed" && job.kind !== "title")) {
      sawTranscription.current = false;
    }
    lastSummary.current = session.data.summary;
  }, [job?.kind, job?.state, jobPhase, notesDirty, session.data]);
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
    ? (["summary", "notes", "transcript"] as const)
    : (["summary", "notes"] as const);
  const activeTab = tab === "transcript" && !canShowTranscript ? "notes" : tab;
  return (
    <Editor
      session={data}
      onDirty={handleDirty}
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
              onClick={() => setTab(view)}
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
        {!snapshot.recording.session_id &&
          !hasAudio &&
          !data.transcript.length && (
            <Recording
              inline
              snapshot={snapshot}
              selected={id}
              select={() => {}}
            />
          )}
      </div>
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
      {attachmentsOpen && (
        <section
          id="session-attachments"
          className="attachments-panel"
          aria-label="Note attachments"
        >
          {data.attachments?.length ? (
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
          ) : (
            <p className="muted">No attachments.</p>
          )}
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
          ) : (
            <p className="muted">
              {activeJob?.kind === "transcribe"
                ? activeJob.state === "queued"
                  ? "Transcription queued…"
                  : "Transcribing…"
                : recording
                  ? "Your transcript will be ready after recording."
                  : finishingRecording
                    ? "Finishing recording…"
                    : "No transcript yet."}
            </p>
          )}
          {!job && (
            <CommandButton
              command="mobile_transcribe"
              args={{ sessionId: id }}
              disabled={!snapshot.model.ready || !data.audio_url || recording}
            >
              {data.transcript.length ? "Transcribe again" : "Transcribe"}
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
                : "No summary yet."}
            </p>
          )}
          {!job && (
            <CommandButton
              command="mobile_summarize"
              args={{ sessionId: id }}
              disabled={
                !snapshot.summary.available ||
                notesDirty ||
                (!data.notes.trim() && !data.transcript.length)
              }
            >
              {data.summary ? "Regenerate summary" : "Create summary"}
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
            <p className="muted">
              {snapshot.summary.reason || "Configure summaries in settings."}
            </p>
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
  showNotes = true,
  children,
  actions,
}: {
  session: Session;
  showNotes?: boolean;
  children?: React.ReactNode;
  actions?: (notes: string) => React.ReactNode;
  onDirty: (dirty: boolean) => void;
}) {
  const save = useCommand("mobile_update_session");
  const [baseline, setBaseline] = useState({
    title: session.title,
    notes: session.notes,
  });
  const form = useForm({
    defaultValues: baseline,
    onSubmit: async ({ value }) => {
      await save.mutateAsync({
        sessionId: session.id,
        ...value,
        expectedTitle: baseline.title,
        expectedNotes: baseline.notes,
      });
      setBaseline(value);
      form.reset(value);
    },
  });
  const titleField = useRef<HTMLTextAreaElement>(null);
  const notesField = useRef<HTMLTextAreaElement>(null);
  const titleValue = useStore(form.store, (state) => state.values.title);
  const notesValue = useStore(form.store, (state) => state.values.notes);
  useLayoutEffect(() => {
    const resize = () => {
      for (const field of [
        titleField.current,
        showNotes ? notesField.current : null,
      ]) {
        if (field) {
          field.style.height = "auto";
          field.style.height = `${field.scrollHeight}px`;
        }
      }
    };
    resize();
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, [titleValue, notesValue, showNotes]);
  const isDirty = useStore(form.store, (state) => state.isDirty);
  useEffect(() => {
    onDirty(isDirty);
    return () => onDirty(false);
  }, [isDirty, onDirty]);
  useEffect(() => {
    if (!isDirty) {
      const current = { title: session.title, notes: session.notes };
      setBaseline(current);
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
            void form.handleSubmit().catch(() => {});
          }}
        >
          <form.Field name="title">
            {(field) => (
              <textarea
                ref={titleField}
                rows={1}
                aria-label="Note title"
                className="title-input"
                value={field.state.value}
                placeholder="Untitled note"
                onChange={(e) =>
                  field.handleChange(e.target.value.replace(/\r?\n/g, " "))
                }
                onBlur={field.handleBlur}
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
              onBlur={field.handleBlur}
            />
          )}
        </form.Field>
      </div>
      <form.Subscribe selector={(s) => [s.isDirty, s.isSubmitting]}>
        {([dirty, submitting]) =>
          showNotes || dirty ? (
            <button
              type="submit"
              form={`note-form-${session.id}`}
              disabled={!dirty || submitting}
            >
              {submitting ? "Saving…" : dirty ? "Save notes" : "Saved"}
            </button>
          ) : null
        }
      </form.Subscribe>
      <ErrorText error={save.error} />
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
      <h1>Settings</h1>
      <section className="card">
        <h2>Notes &amp; recordings folder</h2>
        <p>
          Your notes, recordings, and attachments are saved as files in this
          folder.
        </p>
        <div className="vault-location">
          <strong>On this iPhone</strong>
          <span>Loofah app storage</span>
        </div>
        <p className="caption">
          {snapshot.vault.icloud_path
            ? "A local copy stays on this iPhone so you can keep working offline."
            : "Your vault is ready to use. Everything is saved locally in Loofah’s private app folder; iCloud is optional."}
        </p>
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
            : "To sync with your Mac, connect the same vault folder in iCloud Drive. Your existing notes will sync with it."}
        </p>
        <ErrorText error={snapshot.sync.error} />
        <CommandButton command="mobile_connect_vault">
          {snapshot.vault.icloud_path
            ? "Reconnect iCloud folder"
            : "Connect iCloud folder"}
        </CommandButton>
        {snapshot.sync.connected && (
          <CommandButton command="mobile_sync">Sync now</CommandButton>
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
      <section className="card">
        <h2>On-device transcription</h2>
        <p>
          Transcription runs locally on your iPhone. Download the model once
          before transcribing. Recordings sync when iCloud is enabled.
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
