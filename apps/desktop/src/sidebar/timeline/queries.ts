import { useMemo } from "react";

import { useIndexQuery } from "~/shared/index-query";
import type {
  TimelineSessionRow,
  TimelineSessionsTable,
} from "~/sidebar/timeline/utils";
import { useUndoDelete } from "~/store/zustand/undo-delete";
import { commands, type SessionListHeader } from "~/types/tauri.gen";

const EMPTY_SESSIONS: Record<string, TimelineSessionRow> = {};

export function useTimelineSessionsTable(): TimelineSessionsTable {
  const { data: timelineSessionsTable = EMPTY_SESSIONS } = useIndexQuery({
    entity: "session_headers",
    queryKey: ["timeline-sessions"],
    queryFn: async () => {
      const result = await commands.sessionListHeaders();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return mapTimelineSessionEntries(result.data);
    },
  });
  const pendingDeletions = useUndoDelete((state) => state.pendingDeletions);

  // Sessions with a pending deletion are hidden optimistically, before the
  // delete write commits and the index re-emits.
  return useMemo(() => {
    const pendingIds = Object.keys(pendingDeletions).filter(
      (sessionId) => sessionId in timelineSessionsTable,
    );
    if (pendingIds.length === 0) return timelineSessionsTable;

    const filtered = { ...timelineSessionsTable };
    for (const sessionId of pendingIds) {
      delete filtered[sessionId];
    }
    return filtered;
  }, [timelineSessionsTable, pendingDeletions]);
}

// session_list_headers is already (created_at, id) ASC -- the order the timeline expects.
function mapTimelineSessionEntries(
  entries: SessionListHeader[],
): Record<string, TimelineSessionRow> {
  return Object.fromEntries(
    entries.map((entry) => [
      entry.id,
      {
        title: entry.title,
        created_at: entry.created_at,
        folder_id: entry.folder ?? "",
        tags: entry.tags,
        author: entry.author,
      },
    ]),
  );
}
