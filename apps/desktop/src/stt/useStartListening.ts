import { useCallback } from "react";

import { useListener } from "./contexts";
import { useSTTConnection } from "./useSTTConnection";

import { useShell } from "~/contexts/shell";
import { useSession } from "~/session/queries";
import { beginSummaryActivity } from "~/session/summary-activity";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { useConfigValue } from "~/shared/config";
import {
  getLiveTranscriptionConfig,
  getTranscriptionLanguages,
} from "~/stt/capabilities";

export function useStartListening(
  sessionId: string,
  isAvailable: () => boolean,
) {
  const owner = useVaultReadScope();
  const session = useSession(sessionId);
  const meetingLanguages = useConfigValue("meeting_languages");
  const transcriptionTiming = useConfigValue("transcription_timing");
  const start = useListener((state) => state.start);
  const { conn } = useSTTConnection();
  const { leftsidebar } = useShell();
  const setLeftSidebarExpanded = leftsidebar.setExpanded;

  return useCallback(async () => {
    const current = () => isAvailable() && owner.current();
    if (!current()) return;
    const requestId = crypto.randomUUID();
    const activity = beginSummaryActivity(sessionId, "recording");
    let started = false;
    try {
      const languages = getTranscriptionLanguages(meetingLanguages);
      const config = await getLiveTranscriptionConfig({
        provider: conn?.provider,
        model: conn?.model,
        languages,
        timing: transcriptionTiming,
      });
      if (!current()) return;
      started = await start(
        {
          session_id: sessionId,
          languages: config.languages,
          onboarding: false,
          model: conn?.model ?? "",
          base_url: conn?.baseUrl ?? "",
          api_key: "",
          transcription_mode: conn?.baseUrl.trim()
            ? config.transcriptionMode
            : "batch",
          participant_human_ids: [],
          self_human_id: session?.user_id || null,
        },
        {
          requestId,
          isAvailable: current,
          // Native owns persistence and declared follow-up jobs through completion,
          // including when this window closes or the displayed vault changes.
          onStopped: () => activity.finish(),
        },
      );
      if (started && current()) setLeftSidebarExpanded(false);
    } finally {
      if (!started) activity.finish();
    }
  }, [
    conn,
    isAvailable,
    meetingLanguages,
    owner.current,
    owner.scope,
    session,
    sessionId,
    setLeftSidebarExpanded,
    start,
    transcriptionTiming,
  ]);
}
