import { Trans, useLingui } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
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
import { useSessionSummaries } from "~/session/summary-queries";
import type { VaultScope } from "~/shared/application/vault-scope";
import { useMainContentCenterOffset } from "~/shared/main/content-offset";
import { useTabs } from "~/store/zustand/tabs";

const MAX_RECENT_DISPLAY = 5;
const SEARCH_LIMIT = 20;
const SNIPPET_MAX_CHARS = 120;
// cmdk registers a DOM item per row, so bounded lists keep the palette responsive.
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
    <span className="text-muted-foreground truncate text-xs">
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

export function OpenNoteDialog(props: OpenNoteDialogProps) {
  const { scope } = useSearchEngine();
  if (
    !scope ||
    !["connected", "reconnecting"].includes(scope.getSnapshot().state)
  )
    return null;
  return (
    <ScopedOpenNoteDialog key={scope.client.scope} scope={scope} {...props} />
  );
}

function ScopedOpenNoteDialog({
  scope,
  open,
  onOpenChange,
  mainContentCenterOffset = 0,
}: OpenNoteDialogProps & { scope: VaultScope }) {
  const { t } = useLingui();
  const [query, setQuery] = useState("");
  const openCurrent = useTabs((state) => state.openCurrent);
  const recentlyOpenedSessionIds = useTabs(
    (state) => state.recentlyOpenedSessionIds,
  );
  const { search, ready } = useSearchEngine();

  const trimmedQuery = query.trim();
  const recentIds = useMemo(
    () => recentlyOpenedSessionIds.slice(0, MAX_RECENT_DISPLAY),
    [recentlyOpenedSessionIds],
  );
  const latest = useSessionSummaries(
    scope,
    {
      kind: "list",
      limit: MAX_ALL_NOTES_DISPLAY + MAX_RECENT_DISPLAY,
      order: "newest_first",
      titled_only: false,
    },
    open && ready && !trimmedQuery,
  );
  const recent = useSessionSummaries(
    scope,
    { kind: "ids", ids: recentIds },
    open && ready && !trimmedQuery && recentIds.length > 0,
  );
  const titles = useSessionSummaries(
    scope,
    { kind: "title", query: trimmedQuery, limit: MAX_TITLE_MATCHES },
    open && ready && !!trimmedQuery,
  );

  const content = useQuery(
    {
      queryKey: [
        "vault",
        scope.client.scope,
        "search",
        "open-note-dialog",
        trimmedQuery,
      ],
      queryFn: ({ signal }) =>
        search(
          trimmedQuery,
          null,
          {
            limit: SEARCH_LIMIT,
            snippets: true,
            snippetMaxChars: SNIPPET_MAX_CHARS,
          },
          signal,
        ),
      enabled: open && ready && trimmedQuery.length > 0,
      placeholderData: (previous, previousQuery) =>
        previousQuery?.queryKey[1] === scope.client.scope
          ? previous
          : undefined,
    },
    scope.queryClient,
  );
  const sessionHits = useMemo(
    () =>
      (content.data ?? [])
        .filter((hit) => hit.document.type === "session")
        .slice(0, SEARCH_LIMIT),
    [content.data],
  );
  const hitIds = useMemo(
    () => [...new Set(sessionHits.map((hit) => hit.document.id))],
    [sessionHits],
  );
  const confirmed = useSessionSummaries(
    scope,
    { kind: "ids", ids: hitIds },
    open && ready && !!trimmedQuery && hitIds.length > 0,
  );

  const recentSessions = useMemo(() => {
    const summaries = new Map(
      (recent.data ?? []).map((summary) => [summary.id, summary]),
    );
    return recentIds.flatMap((id) => {
      const summary = summaries.get(id);
      return summary
        ? [
            {
              id: summary.id,
              title: summary.title || t`Untitled`,
              createdAt: summary.created_at,
            },
          ]
        : [];
    });
  }, [recent.data, recentIds, t]);
  const otherNotes = useMemo(() => {
    const recentSet = new Set(recentSessions.map((note) => note.id));
    return (latest.data ?? [])
      .filter((summary) => !recentSet.has(summary.id))
      .slice(0, MAX_ALL_NOTES_DISPLAY)
      .map((summary) => ({
        id: summary.id,
        title: summary.title || t`Untitled`,
        createdAt: summary.created_at,
      }));
  }, [latest.data, recentSessions, t]);
  const searchResults = useMemo(() => {
    if (!trimmedQuery) return [];
    const snippets = new Map(
      sessionHits.map((hit) => [hit.document.id, hit.contentSnippet]),
    );
    const summaries = new Map(
      (confirmed.data ?? []).map((summary) => [summary.id, summary]),
    );
    const results: NoteResult[] = [];
    const seen = new Set<string>();
    for (const summary of (titles.data ?? []).slice(0, MAX_TITLE_MATCHES)) {
      seen.add(summary.id);
      results.push({
        id: summary.id,
        title: summary.title || t`Untitled`,
        createdAt: summary.created_at,
        snippet: snippets.get(summary.id) ?? null,
      });
    }
    for (const hit of sessionHits) {
      const summary = summaries.get(hit.document.id);
      if (!summary || seen.has(summary.id)) continue;
      seen.add(summary.id);
      results.push({
        id: summary.id,
        title: summary.title || t`Untitled`,
        createdAt: summary.created_at,
        snippet: hit.contentSnippet,
      });
    }
    return results;
  }, [trimmedQuery, sessionHits, confirmed.data, titles.data, t]);
  const pending =
    ready &&
    (trimmedQuery
      ? titles.isPending ||
        titles.isFetching ||
        content.isPending ||
        content.isFetching ||
        (hitIds.length > 0 && (confirmed.isPending || confirmed.isFetching))
      : latest.isPending ||
        latest.isFetching ||
        (recentIds.length > 0 && (recent.isPending || recent.isFetching)));
  const error = trimmedQuery
    ? (titles.error ?? content.error ?? confirmed.error)
    : (latest.error ?? recent.error);

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
              {error && (
                <p
                  role="alert"
                  className="text-muted-foreground px-2 py-3 text-sm"
                >
                  {error.message}
                </p>
              )}
              {!hasAnyResults ? (
                pending ? (
                  <p
                    role="status"
                    className="text-muted-foreground py-6 text-center text-sm"
                  >
                    <Trans>Loading…</Trans>
                  </p>
                ) : (
                  !error && (
                    <CommandPrimitive.Empty className="text-muted-foreground py-6 text-center text-sm">
                      <Trans>No notes found.</Trans>
                    </CommandPrimitive.Empty>
                  )
                )
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
