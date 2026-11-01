import { ChevronRight, FileText, Search, X } from "lucide-react";

import type { Snapshot } from "./api";

export function groupNotes(sessions: Snapshot["sessions"], now = new Date()) {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  const week = new Date(today);
  week.setDate(today.getDate() - 6);
  const groups = new Map<string, Snapshot["sessions"]>();
  const ordered = [...sessions].sort(
    (a, b) => Date.parse(b.created_at) - Date.parse(a.created_at),
  );
  for (const session of ordered) {
    const date = new Date(session.created_at);
    const label =
      date >= today
        ? "Today"
        : date >= yesterday
          ? "Yesterday"
          : date >= week
            ? "Previous 7 days"
            : Number.isNaN(date.getTime())
              ? "Earlier"
              : date.toLocaleDateString(undefined, {
                  month: "long",
                  year: "numeric",
                });
    const group = groups.get(label) ?? [];
    group.push(session);
    groups.set(label, group);
  }
  return [...groups.entries()];
}

export function NoteLibrary({
  sessions,
  search,
  onSearch,
  onSelect,
}: {
  sessions: Snapshot["sessions"];
  search: string;
  onSearch: (value: string) => void;
  onSelect: (id: string) => void;
}) {
  const matches = sessions.filter((s) =>
    (s.title || "Untitled note")
      .toLocaleLowerCase()
      .includes(search.trim().toLocaleLowerCase()),
  );
  return (
    <>
      <div className="page-heading">
        <h2>Notes</h2>
        <p>
          {sessions.length === 0
            ? "A little space to think."
            : `${sessions.length} ${sessions.length === 1 ? "note" : "notes"} · yours to keep`}
        </p>
      </div>
      <div className="library-search">
        <Search size={19} aria-hidden="true" />
        <input
          type="search"
          aria-label="Search note titles"
          placeholder="Search note titles"
          value={search}
          onChange={(e) => onSearch(e.target.value)}
        />
        {search && (
          <button
            className="icon"
            aria-label="Clear search field"
            onClick={() => onSearch("")}
          >
            <X size={18} />
          </button>
        )}
      </div>
      <div className="session-list">
        {groupNotes(matches).map(([label, notes]) => (
          <section className="note-group" key={label} aria-label={label}>
            <h3 className="section-label">{label}</h3>
            {notes.map((note) => (
              <button
                className="session-row"
                key={note.id}
                onClick={() => onSelect(note.id)}
              >
                <span className="session-row-copy">
                  <strong>{note.title || "Untitled note"}</strong>
                  <small>
                    {new Date(note.created_at).toLocaleDateString(undefined, {
                      day: "numeric",
                      month: "short",
                    })}
                    {note.has_transcript_words && " · Transcribed"}
                  </small>
                </span>
                <ChevronRight size={17} aria-hidden="true" />
              </button>
            ))}
          </section>
        ))}
      </div>
      {sessions.length === 0 && (
        <div className="empty library-empty">
          <FileText size={32} strokeWidth={1.25} aria-hidden="true" />
          <h2>Start with a thought.</h2>
          <p>
            Write a note, record a conversation,
            <br />
            or bring in an audio file.
          </p>
        </div>
      )}
      {sessions.length > 0 && matches.length === 0 && (
        <div className="empty" role="status">
          <h2>No matching notes</h2>
          <p>Try another title or clear your search.</p>
          <button onClick={() => onSearch("")}>Clear search</button>
        </div>
      )}
    </>
  );
}
