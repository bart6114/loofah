import { useMemo } from "react";

import { hasSummarySource } from "~/services/enhancer/source";
import { useSession } from "~/session/queries";
import { useSessionTranscripts } from "~/stt/queries";

export function useSummarySource(sessionId: string) {
  const note = useSession(sessionId)?.raw_md ?? "";
  const transcripts = useSessionTranscripts(sessionId);
  return useMemo(
    () => hasSummarySource(note, transcripts),
    [note, transcripts],
  );
}
