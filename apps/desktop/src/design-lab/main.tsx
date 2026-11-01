import "@fontsource-variable/literata";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";

import { useForm } from "@tanstack/react-form";
import {
  ArrowLeft,
  ArrowRight,
  AudioLines,
  Check,
  CheckCheck,
  ChevronRight,
  Circle,
  Copy,
  FileText,
  Folder,
  Headphones,
  LayoutGrid,
  Leaf,
  MessageSquare,
  MoreHorizontal,
  PanelLeft,
  Pencil,
  Plus,
  Search,
  Settings,
  Sparkles,
  Square,
  Star,
  Waves,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { cn } from "@hypr/utils";

import loofahMark from "../../src-tauri/icons/src/loofah-mark-1024.png";

const directions = [
  {
    id: "fieldnotes",
    name: "Fieldnotes",
    character: "Literary · warm · considered",
    description:
      "Cream paper, editorial type, and a little burnt orange. A notebook you want to return to.",
    signature: "Serif titles + a warm, ink-like navigation",
    tradeoff:
      "More personality in the reading surface; less utility-app neutrality.",
    mark: "f.",
  },
  {
    id: "signal",
    name: "Signal",
    character: "Precise · graphic · energetic",
    description:
      "White, graphite, and electric green. The meeting becomes a well-tuned recording instrument.",
    signature: "Monospaced details + a graphic session track",
    tradeoff:
      "Denser and more technical; recording feels central to the product.",
    mark: "l_",
  },
  {
    id: "forma",
    name: "Forma",
    character: "Bold · friendly · unmistakable",
    description:
      "A confident cobalt sidebar, soft geometry, and generous sans-serif text. Recognizable at a glance.",
    signature: "A saturated sidebar + soft, rounded controls",
    tradeoff: "A stronger brand presence throughout the working day.",
    mark: "lo",
  },
  {
    id: "margin",
    name: "Margin",
    character: "Quiet · spacious · personal",
    description:
      "A slim lilac rail, a broad writing canvas, and thoughts in the margin. Built around room to think.",
    signature: "An icon rail + a distinctive annotation margin",
    tradeoff:
      "Less visible navigation; more space and emphasis for the note itself.",
    mark: "l°",
  },
  {
    id: "workshop",
    name: "Workshop",
    character: "Grounded · useful · tactile",
    description:
      "Forest green, paper tabs, and neatly ruled details. A working journal with a practical character.",
    signature: "Folder tabs + journal-like section markers",
    tradeoff: "More visible organization and structure around the writing.",
    mark: "l/",
  },
] as const;

const meetings = [
  {
    title: "Product direction",
    subtitle: "Maya, Alex, You",
    time: "10:00",
    day: "Today",
  },
  {
    title: "A quieter onboarding",
    subtitle: "Design team",
    time: "09:15",
    day: "Today",
  },
  {
    title: "Friday check-in",
    subtitle: "Alex, You",
    time: "16:30",
    day: "Yesterday",
  },
  {
    title: "Research interviews",
    subtitle: "Maya, You",
    time: "11:00",
    day: "Yesterday",
  },
];

const tasks = [
  "Prototype a simpler first recording",
  "Test the new note layout with five people",
  "Bring the next iteration on Friday",
];

function readChoices() {
  try {
    const saved = JSON.parse(
      localStorage.getItem("loofah-design-shortlist") ?? "{}",
    );
    return Object.fromEntries(
      directions.map(({ id }) => [
        id,
        Array.isArray(saved[id])
          ? saved[id].filter((value: unknown) => typeof value === "string")
          : [],
      ]),
    ) as Record<string, string[]>;
  } catch {
    return {} as Record<string, string[]>;
  }
}

function DesignLab() {
  const [focused, setFocused] = useState<string | null>(() => {
    const id = window.location.hash.slice(1);
    return directions.some((direction) => direction.id === id) ? id : null;
  });
  const [choices, setChoices] = useState(readChoices);
  const [copied, setCopied] = useState(false);
  const selected = directions.find((direction) => direction.id === focused);
  const shortlist = directions.filter(
    (direction) => choices[direction.id]?.length,
  );
  const form = useForm({ defaultValues: { feedback: "" } });

  useEffect(() => {
    const url = new URL(window.location.href);
    url.hash = focused ?? "";
    window.history.replaceState(null, "", url);
  }, [focused]);

  useEffect(() => {
    localStorage.setItem("loofah-design-shortlist", JSON.stringify(choices));
  }, [choices]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setFocused(null);
        window.scrollTo({ top: 0, behavior: "instant" });
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const openDirection = (id: string) => {
    setFocused(id);
    window.scrollTo({ top: 0, behavior: "instant" });
  };

  const toggleChoice = (id: string, aspect: string) => {
    setChoices((previous) => {
      const current = previous[id] ?? [];
      return {
        ...previous,
        [id]: current.includes(aspect)
          ? current.filter((value) => value !== aspect)
          : [...current, aspect],
      };
    });
    setCopied(false);
  };

  const copyFeedback = async () => {
    const text = [
      "Loofah design direction:",
      ...shortlist.map(
        (direction) => `${direction.name}: ${choices[direction.id].join(", ")}`,
      ),
      form.state.values.feedback.trim(),
    ]
      .filter(Boolean)
      .join("\n");
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <main className="lab-page">
      <header className="lab-header">
        <a
          href="/design-lab.html"
          className="lab-brand"
          aria-label="Loofah design studies home"
        >
          <span className="lab-brand-mark">
            <Waves size={19} />
          </span>
          <span>
            loofah<span className="lab-brand-suffix"> / design studies</span>
          </span>
        </a>
        <span className="lab-edition">Five directions · light appearance</span>
      </header>

      {selected ? (
        <section className="focus-view">
          <div className="focus-heading">
            <button
              className="lab-button back-button"
              onClick={() => {
                setFocused(null);
                window.scrollTo({ top: 0, behavior: "instant" });
              }}
            >
              <ArrowLeft size={16} /> All five directions
            </button>
            <div className="focus-switcher" aria-label="Design direction">
              {directions.map((direction) => (
                <button
                  key={direction.id}
                  aria-pressed={direction.id === focused}
                  onClick={() => openDirection(direction.id)}
                >
                  <span className={`direction-dot ${direction.id}`} />
                  {direction.name}
                </button>
              ))}
            </div>
          </div>
          <div className="focus-description">
            <div>
              <p className="eyebrow">{selected.character}</p>
              <h1>{selected.name}</h1>
            </div>
            <p>{selected.description}</p>
          </div>
          <MockWindow key={selected.id} direction={selected} interactive />
          <div className="focus-bottom">
            <div>
              <p className="signature-label">The signature</p>
              <p>{selected.signature}</p>
              <p className="tradeoff">{selected.tradeoff}</p>
            </div>
            <PreferencePicker
              direction={selected}
              values={choices[selected.id] ?? []}
              onToggle={toggleChoice}
            />
          </div>
        </section>
      ) : (
        <>
          <section className="lab-intro">
            <div>
              <p className="eyebrow">A more recognizable Loofah</p>
              <h1>Find its character.</h1>
            </div>
            <p>
              Five different takes on the same meeting.
              <br />
              Open one, try the controls, and keep the parts you like.
            </p>
          </section>
          <section className="direction-grid" aria-label="Five design mockups">
            {directions.map((direction, index) => (
              <article className="direction-card" key={direction.id}>
                <div className="direction-heading">
                  <div>
                    <span className="direction-index">0{index + 1}</span>
                    <h2>{direction.name}</h2>
                  </div>
                  <span className="direction-character">
                    {direction.character}
                  </span>
                </div>
                <div className="mock-preview">
                  <MockWindow direction={direction} interactive={false} />
                  <button
                    className="explore-overlay"
                    aria-label={`Explore ${direction.name} design`}
                    onClick={() => openDirection(direction.id)}
                  >
                    <span>
                      Explore {direction.name}
                      <ArrowRight size={15} />
                    </span>
                  </button>
                </div>
                <div className="direction-caption">
                  <p>{direction.description}</p>
                  <button
                    className="shortlist-button"
                    aria-pressed={Boolean(choices[direction.id]?.length)}
                    onClick={() => {
                      setChoices((previous) => ({
                        ...previous,
                        [direction.id]: previous[direction.id]?.length
                          ? []
                          : ["Overall feel"],
                      }));
                      setCopied(false);
                    }}
                  >
                    <Star
                      size={16}
                      fill={
                        choices[direction.id]?.length ? "currentColor" : "none"
                      }
                    />
                    {choices[direction.id]?.length ? "Saved" : "Save"}
                  </button>
                </div>
              </article>
            ))}
            <section className="shortlist-panel">
              <p className="eyebrow">There doesn’t have to be one winner</p>
              <h2>
                A little of this.
                <br />A little of that.
              </h2>
              <p>
                Keep the type from one, the sidebar from another. We can turn
                your favorites into a sixth direction.
              </p>
              <div className="shortlist-examples">
                <span>Typography</span>
                <span>Color</span>
                <span>Layout</span>
                <span>Overall feel</span>
              </div>
              <p className="shortlist-hint">
                <ArrowRight size={16} /> Open a design to save specific parts.
              </p>
            </section>
          </section>
        </>
      )}

      <section
        className="feedback-section"
        aria-label="Your design preferences"
      >
        <div className="feedback-heading">
          <div>
            <p className="eyebrow">Your direction</p>
            <h2>
              {shortlist.length
                ? "The shortlist is taking shape."
                : "What feels like Loofah?"}
            </h2>
          </div>
          <span className="feedback-count" aria-live="polite">
            {shortlist.length
              ? `${shortlist.length} saved`
              : "No favorites yet"}
          </span>
        </div>
        {shortlist.length > 0 && (
          <div className="saved-directions">
            {shortlist.map((direction) => (
              <div key={direction.id}>
                <button onClick={() => openDirection(direction.id)}>
                  <span className={`direction-dot ${direction.id}`} />
                  <strong>{direction.name}</strong>
                  <span>{choices[direction.id].join(" · ")}</span>
                </button>
                <button
                  aria-label={`Remove ${direction.name} from shortlist`}
                  onClick={() => {
                    setChoices((previous) => ({
                      ...previous,
                      [direction.id]: [],
                    }));
                    setCopied(false);
                  }}
                >
                  <X size={14} />
                </button>
              </div>
            ))}
          </div>
        )}
        <form.Field name="feedback">
          {(field) => (
            <label className="feedback-field">
              <span>A few words for the next iteration</span>
              <textarea
                placeholder="e.g. Fieldnotes’ type, Forma’s sidebar, but a little more compact…"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(event) => {
                  field.handleChange(event.target.value);
                  setCopied(false);
                }}
              />
            </label>
          )}
        </form.Field>
        <div className="feedback-actions">
          <p>
            Copy your picks into our conversation and we’ll refine from there.
          </p>
          <form.Subscribe selector={(state) => state.values.feedback}>
            {(feedback) => (
              <button
                className="lab-button primary-button"
                onClick={copyFeedback}
                disabled={!shortlist.length && !feedback.trim()}
              >
                {copied ? <Check size={16} /> : <Copy size={16} />}
                {copied ? "Copied — paste into chat" : "Copy feedback"}
              </button>
            )}
          </form.Subscribe>
        </div>
      </section>
      <footer className="lab-footer">
        <span>Loofah / design studies</span>
        <span>Light studies. Every direction has a matching dark palette.</span>
      </footer>
    </main>
  );
}

function PreferencePicker({
  direction,
  values,
  onToggle,
}: {
  direction: (typeof directions)[number];
  values: string[];
  onToggle: (id: string, aspect: string) => void;
}) {
  return (
    <div className="preference-picker">
      <p className="signature-label">Keep these parts</p>
      <div>
        {["Typography", "Color", "Layout", "Overall feel"].map((aspect) => (
          <button
            key={aspect}
            aria-pressed={values.includes(aspect)}
            onClick={() => onToggle(direction.id, aspect)}
          >
            {values.includes(aspect) ? <Check size={14} /> : <Plus size={14} />}
            {aspect}
          </button>
        ))}
      </div>
    </div>
  );
}

function MockWindow({
  direction,
  interactive,
}: {
  direction: (typeof directions)[number];
  interactive: boolean;
}) {
  const [view, setView] = useState("Summary");
  const [meeting, setMeeting] = useState(0);
  const [recording, setRecording] = useState(false);
  const [completed, setCompleted] = useState<number[]>([0]);
  const [searching, setSearching] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [elapsed, setElapsed] = useState(0);
  const isSignal = direction.id === "signal";
  const isMargin = direction.id === "margin";
  const isWorkshop = direction.id === "workshop";

  useEffect(() => {
    if (!recording) return;
    const timer = window.setInterval(
      () => setElapsed((seconds) => seconds + 1),
      1000,
    );
    return () => window.clearInterval(timer);
  }, [recording]);

  const record = () => {
    if (!recording) setElapsed(0);
    setRecording(!recording);
  };

  return (
    <div
      className={`mock-window ${direction.id}`}
      data-appearance="light"
      data-mini={!interactive}
      data-sidebar={sidebarOpen}
      inert={!interactive ? true : undefined}
    >
      <div className="window-topbar">
        <div className="traffic-lights" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>
        <span className="window-title">
          loofah{" "}
          {isSignal ? "/ session studio" : isWorkshop ? "/ working notes" : ""}
        </span>
        <button
          className="mock-icon-button"
          aria-label="Toggle sidebar"
          onClick={() => setSidebarOpen(!sidebarOpen)}
        >
          <PanelLeft size={15} />
        </button>
      </div>
      {isWorkshop && (
        <div className="workshop-tabs">
          <button className="active">
            <FileText size={13} /> {meetings[meeting].title}
            <X size={12} />
          </button>
          <button onClick={() => setMeeting((meeting + 1) % meetings.length)}>
            <Plus size={14} /> Open another note
          </button>
        </div>
      )}
      <div className="window-workspace">
        {sidebarOpen && (
          <aside className={cn(["mock-sidebar", isMargin && "icon-rail"])}>
            <div className="product-brand">
              <span className="product-mark" aria-hidden="true">
                {isSignal ? (
                  <span
                    className="signal-logo-mark"
                    style={{
                      maskImage: `url(${loofahMark})`,
                      WebkitMaskImage: `url(${loofahMark})`,
                    }}
                  />
                ) : (
                  direction.mark
                )}
              </span>
              {!isMargin && (
                <span>loofah{isSignal ? "_" : isWorkshop ? " /" : ""}</span>
              )}
            </div>
            {isMargin ? (
              <>
                <div className="rail-navigation">
                  <button
                    className="selected"
                    aria-label="Notes"
                    onClick={() => setView("Note")}
                  >
                    <FileText size={20} />
                  </button>
                  <button
                    aria-label="Summary"
                    onClick={() => setView("Summary")}
                  >
                    <LayoutGrid size={20} />
                  </button>
                  <button
                    aria-label="Transcript"
                    onClick={() => setView("Transcript")}
                  >
                    <Headphones size={20} />
                  </button>
                  <button
                    aria-label="Next note"
                    onClick={() => setMeeting((meeting + 1) % meetings.length)}
                  >
                    <Plus size={20} />
                  </button>
                </div>
                <button
                  className="rail-bottom"
                  aria-label="Toggle search"
                  onClick={() => setSearching(!searching)}
                >
                  <Search size={19} />
                </button>
              </>
            ) : (
              <>
                <button
                  className="mock-new-note"
                  onClick={() => {
                    setMeeting(0);
                    setView("Note");
                  }}
                >
                  <Plus size={15} />
                  <span>New note</span>
                  <kbd>⌘N</kbd>
                </button>
                <button
                  className="mock-search"
                  onClick={() => setSearching(!searching)}
                >
                  <Search size={14} />
                  <span>Search notes</span>
                  <kbd>⌘K</kbd>
                </button>
                <div className="sidebar-collections">
                  <button className="active" onClick={() => setMeeting(0)}>
                    {isWorkshop ? <Folder size={14} /> : <FileText size={14} />}
                    All notes <span>24</span>
                  </button>
                  {direction.id === "forma" && (
                    <button onClick={() => setView("Transcript")}>
                      <Headphones size={14} />
                      Recordings<span>12</span>
                    </button>
                  )}
                  {isWorkshop && (
                    <button onClick={() => setMeeting(3)}>
                      <Leaf size={14} />
                      Research<span>8</span>
                    </button>
                  )}
                </div>
                <div className="mock-meetings">
                  {meetings.map((item, index) => (
                    <div key={item.title}>
                      {(index === 0 || index === 2) && (
                        <p className="meeting-group">{item.day}</p>
                      )}
                      <button
                        className={index === meeting ? "selected" : ""}
                        onClick={() => {
                          setMeeting(index);
                          setView("Summary");
                        }}
                      >
                        <span className="meeting-row-title">{item.title}</span>
                        <span className="meeting-row-subtitle">
                          {item.subtitle}
                        </span>
                        <time>{item.time}</time>
                      </button>
                    </div>
                  ))}
                </div>
                <div className="sidebar-footer">
                  <span>
                    {direction.id === "fieldnotes"
                      ? "Good thoughts, kept."
                      : isSignal
                        ? "LOCAL / PRIVATE"
                        : isWorkshop
                          ? "A place for the work."
                          : "A little more headspace."}
                  </span>
                  <button
                    aria-label="Toggle note outline"
                    onClick={() =>
                      setView(view === "Transcript" ? "Summary" : "Transcript")
                    }
                  >
                    <Settings size={15} />
                  </button>
                </div>
              </>
            )}
          </aside>
        )}

        <section className="mock-main">
          <div className="note-toolbar">
            {isMargin && (
              <span className="margin-breadcrumb">
                Notes
                <ChevronRight size={12} />
                {meetings[meeting].title}
              </span>
            )}
            <div className="mock-view-tabs" aria-label="Note view">
              {["Summary", "Note", "Transcript"].map((tab) => (
                <button
                  key={tab}
                  aria-pressed={view === tab}
                  onClick={() => setView(tab)}
                >
                  {tab}
                </button>
              ))}
            </div>
            <div className="toolbar-actions">
              <button
                className={cn(["mock-record", recording && "is-recording"])}
                aria-pressed={recording}
                onClick={record}
              >
                {recording ? (
                  <Square size={11} fill="currentColor" />
                ) : (
                  <span className="record-dot" />
                )}
                {recording ? "Stop" : "Record"}
              </button>
              <button
                className="mock-icon-button"
                aria-label="Show transcript"
                onClick={() => setView("Transcript")}
              >
                <MoreHorizontal size={17} />
              </button>
            </div>
          </div>
          {searching && (
            <div className="mock-search-bar">
              <Search size={15} />
              <input
                autoFocus
                placeholder="Try another note…"
                onChange={(event) => {
                  const index = meetings.findIndex((item) =>
                    item.title
                      .toLowerCase()
                      .includes(event.target.value.toLowerCase()),
                  );
                  if (index >= 0) setMeeting(index);
                }}
              />
              <button
                aria-label="Close search"
                onClick={() => setSearching(false)}
              >
                <X size={15} />
              </button>
            </div>
          )}
          {recording && (
            <div className="preview-recording" role="status">
              <AudioLines size={16} />
              <span>Recording preview</span>
              <time>
                {String(Math.floor(elapsed / 60)).padStart(2, "0")}:
                {String(elapsed % 60).padStart(2, "0")}
              </time>
              <span className="preview-only">No audio is captured</span>
            </div>
          )}
          <div className="document-layout">
            <div className="mock-document">
              <div className="document-eyebrow">
                {isWorkshop ? (
                  <span className="journal-label">MEETING NOTES</span>
                ) : isSignal ? (
                  <span>
                    SESSION 024 <span className="signal-slash">/</span> DESIGN
                  </span>
                ) : isMargin ? (
                  <span>Friday’s thoughts</span>
                ) : (
                  <span>DESIGN TEAM</span>
                )}
                <span className="document-date">Oct 9, 2026</span>
              </div>
              <h1>{meetings[meeting].title}</h1>
              <div className="document-meta">
                <div className="participant-avatars" aria-hidden="true">
                  <span>M</span>
                  <span>A</span>
                  <span>Y</span>
                </div>
                <span>Maya, Alex & you</span>
                <span className="meta-dot">·</span>
                <span>32 min</span>
                {direction.id === "forma" && (
                  <span className="soft-tag">Design</span>
                )}
              </div>
              {view === "Transcript" ? (
                <div className="transcript-content">
                  <div>
                    <div className="speaker">
                      <span className="speaker-avatar">M</span>
                      <strong>Maya</strong>
                      <time>00:14</time>
                    </div>
                    <p>
                      I think we should start with the first thirty seconds. You
                      open Loofah, you press record, and you can get back to the
                      conversation. Everything else should support that.
                    </p>
                  </div>
                  <div>
                    <div className="speaker">
                      <span className="speaker-avatar">A</span>
                      <strong>Alex</strong>
                      <time>00:38</time>
                    </div>
                    <p>
                      Yes. And afterward, the note should feel like something
                      you want to read. A clear title, a useful summary, and
                      enough space to make it your own.
                    </p>
                  </div>
                  <div>
                    <div className="speaker">
                      <span className="speaker-avatar">Y</span>
                      <strong>You</strong>
                      <time>01:06</time>
                    </div>
                    <p>
                      Let’s prototype that smaller version first. We can try it
                      with five people before we commit to the full layout.
                    </p>
                  </div>
                </div>
              ) : (
                <>
                  {view === "Note" ? (
                    <div
                      className="editable-note"
                      contentEditable
                      suppressContentEditableWarning
                      role="textbox"
                      aria-label="Try writing a note"
                      aria-multiline="true"
                    >
                      <h2>Make room for the work.</h2>
                      <p>
                        The next release should feel lighter: fewer decisions,
                        clearer notes, and a recording flow you can trust.
                      </p>
                      <p>
                        Start with the first thirty seconds. Open Loofah, press
                        record, and get back to the conversation.
                      </p>
                      <p>Try writing a thought here…</p>
                    </div>
                  ) : (
                    <>
                      <div className="summary-intro">
                        {direction.id === "forma" && (
                          <span className="intro-icon">
                            <Sparkles size={18} />
                          </span>
                        )}
                        <div>
                          <h2>Make room for the work.</h2>
                          <p>
                            The next release should feel lighter: fewer
                            decisions, clearer notes, and a recording flow you
                            can trust.
                          </p>
                        </div>
                      </div>
                      <section className="mock-section">
                        <h3>
                          <span className="section-number">01</span>What we
                          agreed
                        </h3>
                        <ul>
                          <li>Make the first recording feel effortless.</li>
                          <li>
                            Give notes a clearer hierarchy and more room to
                            breathe.
                          </li>
                          <li>
                            Keep the useful details. Lose the unnecessary
                            decisions.
                          </li>
                        </ul>
                      </section>
                    </>
                  )}
                  <section className="mock-section next-steps">
                    <h3>
                      <span className="section-number">02</span>Next steps
                    </h3>
                    <div className="mock-tasks">
                      {tasks.map((task, index) => (
                        <label
                          key={task}
                          className={
                            completed.includes(index) ? "completed" : ""
                          }
                        >
                          <input
                            type="checkbox"
                            checked={completed.includes(index)}
                            onChange={() =>
                              setCompleted((previous) =>
                                previous.includes(index)
                                  ? previous.filter((value) => value !== index)
                                  : [...previous, index],
                              )
                            }
                          />
                          <span className="task-checkbox" aria-hidden="true">
                            {completed.includes(index) && <Check size={12} />}
                          </span>
                          <span>{task}</span>
                          {index === 2 && (
                            <span className="task-owner">You</span>
                          )}
                        </label>
                      ))}
                    </div>
                  </section>
                </>
              )}
              <div className="document-end">
                <CheckCheck size={13} />
                <span>Local note · design preview</span>
              </div>
            </div>
            {isSignal && (
              <aside className="session-track">
                <p>SESSION TRACK</p>
                <div className="waveform" aria-label="Audio waveform">
                  {[
                    12, 21, 14, 30, 22, 38, 17, 27, 42, 33, 15, 29, 20, 11, 25,
                    34, 18, 27,
                  ].map((height, index) => (
                    <span key={index} style={{ height }} />
                  ))}
                </div>
                <time>00:00 / 32:14</time>
                <div className="track-marker">
                  <span>01</span>
                  <p>Direction</p>
                  <time>00:14</time>
                </div>
                <div className="track-marker">
                  <span>02</span>
                  <p>Decisions</p>
                  <time>12:08</time>
                </div>
                <div className="track-marker">
                  <span>03</span>
                  <p>Next steps</p>
                  <time>24:32</time>
                </div>
                <button onClick={() => setView("Transcript")}>
                  <Headphones size={14} />
                  Read transcript
                  <ArrowRight size={13} />
                </button>
              </aside>
            )}
            {isMargin && (
              <aside className="annotation-margin">
                <p className="margin-label">
                  <MessageSquare size={13} />
                  In the margin
                </p>
                <div className="margin-thought">
                  <span className="margin-pin" />
                  <p>
                    A clear start.
                    <br />A quiet workspace.
                  </p>
                  <span>The thought to keep</span>
                </div>
                <div className="note-outline">
                  <p>On this page</p>
                  <button onClick={() => setView("Summary")}>
                    The direction
                  </button>
                  <button onClick={() => setView("Summary")}>
                    What we agreed
                  </button>
                  <button onClick={() => setView("Note")}>
                    Your notes <Pencil size={11} />
                  </button>
                </div>
              </aside>
            )}
          </div>
          <div className="mock-statusbar">
            <span>
              <Circle size={6} fill="currentColor" />
              On your Mac
            </span>
            <span>
              {recording
                ? "Recording preview"
                : view === "Transcript"
                  ? "Transcript · 3 speakers"
                  : "Private by default"}
            </span>
          </div>
        </section>
      </div>
    </div>
  );
}

const root = createRoot(document.getElementById("design-lab")!);
root.render(<DesignLab />);

if (import.meta.hot) {
  import.meta.hot.dispose(() => root.unmount());
}
