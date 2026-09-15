import { Trans, useLingui } from "@lingui/react/macro";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { Command as CommandPrimitive } from "cmdk";
import { FileTextIcon, SearchIcon, XIcon } from "lucide-react";
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { useHotkeys } from "react-hotkeys-hook";

import { cn } from "@hypr/utils";

import { useSearchEngine } from "~/search/contexts/engine";
import type { SearchSnippet } from "~/search/contexts/engine";
import { snippetSegments } from "~/search/snippet";
import { useSessionSummaries } from "~/session/queries";
import { useMainContentCenterOffset } from "~/shared/main/content-offset";
import { useTabs } from "~/store/zustand/tabs";

const MAX_RECENT_DISPLAY = 5;
const SEARCH_LIMIT = 20;
const SNIPPET_MAX_CHARS = 120;
// cmdk mounts (and registers) a DOM item per row, so the empty-query list and the
// per-keystroke title scan are capped -- typing a query is the path to older
// notes, and everything beyond a cap is unreachable by scrolling anyway.
const MAX_ALL_NOTES_DISPLAY = 50;
const MAX_TITLE_MATCHES = 50;

interface OpenNoteDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  mainContentCenterOffset?: number;
}

type OpenNoteDialogContextValue = {
  open: () => void;
};

type NoteResult = {
  id: string;
  title: string;
  createdAt: string;
  snippet?: SearchSnippet | null;
};

const OpenNoteDialogContext = createContext<OpenNoteDialogContextValue | null>(
  null,
);

export function OpenNoteDialogProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const mainContentCenterOffset = useMainContentCenterOffset();

  const openDialog = useCallback(() => {
    setOpen(true);
  }, []);

  useHotkeys("mod+k", openDialog, {
    preventDefault: true,
    enableOnFormTags: true,
    enableOnContentEditable: true,
  });

  const value = useMemo(() => ({ open: openDialog }), [openDialog]);

  return (
    <OpenNoteDialogContext.Provider value={value}>
      {children}
      <OpenNoteDialog
        open={open}
        onOpenChange={setOpen}
        mainContentCenterOffset={mainContentCenterOffset}
      />
    </OpenNoteDialogContext.Provider>
  );
}

export function useOpenNoteDialog() {
  const context = useContext(OpenNoteDialogContext);
  if (!context) {
    throw new Error(
      "useOpenNoteDialog must be used within OpenNoteDialogProvider",
    );
  }
  return context;
}

function SnippetLine({ snippet }: { snippet: SearchSnippet }) {
  const segments = useMemo(() => snippetSegments(snippet), [snippet]);
  if (segments.length === 0) {
    return null;
  }

  return (
    <span className="text-muted-foreground/80 truncate text-xs">
      {segments.map((segment, index) =>
        segment.highlighted ? (
          <mark
            key={index}
            className="text-foreground bg-transparent font-semibold"
          >
            {segment.text}
          </mark>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </span>
  );
}

export function OpenNoteDialog({
  open,
  onOpenChange,
  mainContentCenterOffset = 0,
}: OpenNoteDialogProps) {
  const { t } = useLingui();
  const [query, setQuery] = useState("");
  const openCurrent = useTabs((state) => state.openCurrent);
  const recentlyOpenedSessionIds = useTabs(
    (state) => state.recentlyOpenedSessionIds,
  );
  const { search } = useSearchEngine();

  const sessions = useSessionSummaries(open);

  const trimmedQuery = query.trim();

  const { data: contentHits } = useQuery({
    queryKey: ["open-note-dialog-search", trimmedQuery, search],
    queryFn: () =>
      search(trimmedQuery, null, {
        limit: SEARCH_LIMIT,
        snippets: true,
        snippetMaxChars: SNIPPET_MAX_CHARS,
      }),
    enabled: open && trimmedQuery.length > 0,
    placeholderData: keepPreviousData,
  });

  const sessionsMap = useMemo(() => {
    return new Map<string, NoteResult>(
      sessions.map((session) => [
        session.id,
        {
          id: session.id,
          title: session.title || t`Untitled`,
          createdAt: session.created_at,
        },
      ]),
    );
  }, [sessions, t]);

  const allNotesSortedByDate = useMemo(() => {
    // Precompute sort keys: at 1,000+ notes a Date-parsing comparator dominates
    // the dialog-open cost.
    return [...sessionsMap.values()]
      .map((note) => ({
        note,
        sortKey: note.createdAt ? Date.parse(note.createdAt) || 0 : 0,
      }))
      .sort((a, b) => b.sortKey - a.sortKey)
      .map(({ note }) => note);
  }, [sessionsMap]);

  const recentSessions = useMemo(() => {
    return recentlyOpenedSessionIds
      .slice(0, MAX_RECENT_DISPLAY)
      .map((id) => sessionsMap.get(id))
      .filter((s): s is NoteResult => s !== undefined);
  }, [recentlyOpenedSessionIds, sessionsMap]);

  const recentSessionIdSet = useMemo(() => {
    return new Set(recentSessions.map((s) => s.id));
  }, [recentSessions]);

  const otherNotes = useMemo(() => {
    return allNotesSortedByDate
      .filter((note) => !recentSessionIdSet.has(note.id))
      .slice(0, MAX_ALL_NOTES_DISPLAY);
  }, [allNotesSortedByDate, recentSessionIdSet]);

  // Full-text hits (memo, summaries, transcripts) merged with client-side
  // title substring matches: Tantivy only matches whole tokens, so substring
  // matching keeps prefix typing ("plan" -> "Planning session") working.
  const searchResults = useMemo(() => {
    if (!trimmedQuery) return [];

    const sessionHits = (contentHits ?? []).filter(
      (hit) => hit.document.type === "session",
    );
    const snippetsById = new Map(
      sessionHits.map((hit) => [hit.document.id, hit.contentSnippet]),
    );

    const lowerQuery = trimmedQuery.toLowerCase();
    const results: NoteResult[] = [];
    const seen = new Set<string>();

    for (const note of allNotesSortedByDate) {
      if (results.length >= MAX_TITLE_MATCHES) {
        break;
      }
      if (note.title.toLowerCase().includes(lowerQuery)) {
        seen.add(note.id);
        results.push({ ...note, snippet: snippetsById.get(note.id) ?? null });
      }
    }

    for (const hit of sessionHits) {
      if (seen.has(hit.document.id)) continue;
      const note = sessionsMap.get(hit.document.id);
      if (!note) continue;
      seen.add(note.id);
      results.push({ ...note, snippet: hit.contentSnippet });
    }

    return results;
  }, [trimmedQuery, contentHits, allNotesSortedByDate, sessionsMap]);

  const hasAnyResults = trimmedQuery
    ? searchResults.length > 0
    : recentSessions.length > 0 || otherNotes.length > 0;

  const handleOpenChange = useCallback(
    (nextOpen: boolean) => {
      if (!nextOpen) {
        setQuery("");
      }
      onOpenChange(nextOpen);
    },
    [onOpenChange],
  );

  const focusInput = useCallback((node: HTMLInputElement | null) => {
    node?.focus();
  }, []);

  const handleSelect = useCallback(
    (note: NoteResult) => {
      handleOpenChange(false);
      openCurrent({ type: "sessions", id: note.id });
    },
    [handleOpenChange, openCurrent],
  );

  if (!open) return null;

  return createPortal(
    <div
      className="fixed inset-0 z-50 bg-black/20 backdrop-blur-xs"
      onClick={() => handleOpenChange(false)}
    >
      <div
        data-tauri-drag-region
        className="absolute top-0 right-0 left-0 h-[15%]"
        onClick={(e) => e.stopPropagation()}
      />
      <div
        className="absolute top-[15%] left-1/2 w-full max-w-lg -translate-x-1/2 px-4"
        style={{ marginLeft: mainContentCenterOffset }}
      >
        <div
          className={cn([
            "border-border/80 bg-background rounded-2xl border",
            "shadow-[0_25px_50px_-12px_rgba(0,0,0,0.25)]",
            "overflow-hidden",
          ])}
          onClick={(e) => e.stopPropagation()}
        >
          <CommandPrimitive
            shouldFilter={false}
            className="flex flex-col"
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                handleOpenChange(false);
              }
            }}
          >
            <div className="border-border/60 flex items-center gap-3 border-b px-4 py-3">
              <SearchIcon className="text-muted-foreground h-4 w-4 shrink-0" />
              <CommandPrimitive.Input
                ref={focusInput}
                value={query}
                onValueChange={setQuery}
                placeholder={t`Find a note...`}
                className={cn([
                  "flex-1 bg-transparent text-sm",
                  "placeholder:text-muted-foreground outline-hidden",
                ])}
              />
              <button
                aria-label={t`Close`}
                onClick={() => handleOpenChange(false)}
                className={cn([
                  "h-5 w-5 rounded-full",
                  "flex items-center justify-center",
                  "bg-accent/80 hover:bg-accent/80",
                  "text-muted-foreground text-xs",
                  "transition-none",
                ])}
              >
                <XIcon className="h-3 w-3" />
              </button>
            </div>

            <CommandPrimitive.List className="max-h-80 overflow-y-auto p-2">
              {!hasAnyResults ? (
                <CommandPrimitive.Empty className="text-muted-foreground py-6 text-center text-sm">
                  <Trans>No notes found.</Trans>
                </CommandPrimitive.Empty>
              ) : trimmedQuery ? (
                <CommandPrimitive.Group>
                  {searchResults.map((note) => (
                    <CommandPrimitive.Item
                      key={note.id}
                      value={note.id}
                      onSelect={() => handleSelect(note)}
                      className={cn([
                        "flex cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5",
                        "text-muted-foreground text-sm",
                        "data-[selected=true]:bg-accent/60",
                        "transition-none",
                      ])}
                    >
                      <FileTextIcon className="text-muted-foreground h-4 w-4 shrink-0" />
                      <span className="flex min-w-0 flex-col">
                        <span className="truncate">{note.title}</span>
                        {note.snippet && <SnippetLine snippet={note.snippet} />}
                      </span>
                    </CommandPrimitive.Item>
                  ))}
                </CommandPrimitive.Group>
              ) : (
                <>
                  {recentSessions.length > 0 && (
                    <CommandPrimitive.Group
                      className={otherNotes.length > 0 ? "pb-1.5" : ""}
                      heading={
                        <div className="text-muted-foreground px-2 py-1.5 text-xs font-medium tracking-wider uppercase">
                          <Trans>Recent</Trans>
                        </div>
                      }
                    >
                      {recentSessions.map((session) => (
                        <CommandPrimitive.Item
                          key={`recent-${session.id}`}
                          value={`recent-${session.id}`}
                          onSelect={() => handleSelect(session)}
                          className={cn([
                            "flex cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5",
                            "text-muted-foreground text-sm",
                            "data-[selected=true]:bg-accent/60",
                            "transition-none",
                          ])}
                        >
                          <FileTextIcon className="text-muted-foreground h-4 w-4 shrink-0" />
                          <span className="truncate">{session.title}</span>
                        </CommandPrimitive.Item>
                      ))}
                    </CommandPrimitive.Group>
                  )}

                  {otherNotes.length > 0 && (
                    <CommandPrimitive.Group
                      heading={
                        <div className="flex flex-col gap-3">
                          {recentSessions.length > 0 && (
                            <div className="bg-accent mx-2 h-px" />
                          )}
                          <div className="text-muted-foreground px-2 py-1.5 text-xs font-medium tracking-wider uppercase">
                            <Trans>All Notes</Trans>
                          </div>
                        </div>
                      }
                    >
                      {otherNotes.map((note) => (
                        <CommandPrimitive.Item
                          key={note.id}
                          value={note.id}
                          onSelect={() => handleSelect(note)}
                          className={cn([
                            "flex cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5",
                            "text-muted-foreground text-sm",
                            "data-[selected=true]:bg-accent/60",
                            "transition-none",
                          ])}
                        >
                          <FileTextIcon className="text-muted-foreground h-4 w-4 shrink-0" />
                          <span className="truncate">{note.title}</span>
                        </CommandPrimitive.Item>
                      ))}
                    </CommandPrimitive.Group>
                  )}
                </>
              )}
            </CommandPrimitive.List>
          </CommandPrimitive>
        </div>
      </div>
    </div>,
    document.body,
  );
}
