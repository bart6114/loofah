import { useQuery, useQueryClient } from "@tanstack/react-query";
import { convertFileSrc } from "@tauri-apps/api/core";
import React, { useEffect, useRef } from "react";

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";

import {
  NoteInput,
  shouldShowTranscriptTabSpinner,
  type NoteInputHandle,
} from "./components/note-input";
import {
  createEditorTabs,
  Header as NoteInputHeader,
} from "./components/note-input/header";
import { SearchProvider } from "./components/note-input/search/context";
import { OuterHeader } from "./components/outer-header";
import { SessionSurface } from "./components/session-surface";
import {
  computeCurrentNoteTab,
  getCanShowTranscript,
  hasStoredNoteContent,
  useHasTranscript,
} from "./components/shared";
import { useAutoEnhance } from "./hooks/useAutoEnhance";
import { shouldShowSessionTopAudioPlayer } from "./top-audio-player";

import * as AudioPlayer from "~/audio-player";
import {
  useEnhancedNoteRecords,
  useSession,
  useSessionRawMd,
} from "~/session/queries";
import { type Tab, useTabs } from "~/store/zustand/tabs";
import { useListener } from "~/stt/contexts";
import { consumePendingUpload } from "~/stt/pending-upload";
import { useStartListening } from "~/stt/useStartListening";
import { useSTTConnection } from "~/stt/useSTTConnection";
import { useUploadFile } from "~/stt/useUploadFile";

export function TabContentNote({
  standaloneWindow = false,
  tab,
}: {
  standaloneWindow?: boolean;
  tab: Extract<Tab, { type: "sessions" }>;
}) {
  const sessionMode = useListener((state) => state.getSessionMode(tab.id));
  const audioExists = AudioPlayer.useAudioExists(tab.id);
  const queryClient = useQueryClient();
  const isCapturing = sessionMode === "active" || sessionMode === "finalizing";

  const audioUrlQuery = useQuery({
    enabled: !isCapturing,
    queryKey: ["audio", tab.id, "url"],
    queryFn: () => fsSyncCommands.audioPath(tab.id),
    select: (result) => {
      if (result.status === "error") {
        return null;
      }
      return convertFileSrc(result.data);
    },
  });
  // While capturing, cached url/peaks describe the previous recording; withholding the
  // url tears the player down so nothing renders (or precomputes peaks) against a file
  // that is about to be replaced.
  const audioUrl = isCapturing ? null : audioUrlQuery.data;

  const wasCapturingRef = useRef(false);
  useEffect(() => {
    // A finished recording replaces the audio file at an unchanged path, so the audio
    // queries (exist/url/peaks) must be told rather than left to focus-driven refetches.
    if (wasCapturingRef.current && !isCapturing) {
      void queryClient.invalidateQueries({ queryKey: ["audio", tab.id] });
    }
    wasCapturingRef.current = isCapturing;
  }, [isCapturing, queryClient, tab.id]);

  return (
    <>
      {tab.state.autoStart && !standaloneWindow ? (
        <AutoStartListening tab={tab} />
      ) : null}
      <SearchProvider>
        <AudioPlayer.Provider sessionId={tab.id} url={audioUrl ?? ""}>
          <TabContentNoteInner
            tab={tab}
            standaloneWindow={standaloneWindow}
            audioUrlReady={Boolean(audioUrl)}
            audioExists={audioExists}
          />
        </AudioPlayer.Provider>
      </SearchProvider>
    </>
  );
}

function AutoStartListening({
  tab,
}: {
  tab: Extract<Tab, { type: "sessions" }>;
}) {
  const canStartLiveSession = useListener((state) =>
    state.canStartLiveSession(tab.id),
  );
  const updateSessionTabState = useTabs((state) => state.updateSessionTabState);
  const { conn } = useSTTConnection();
  const startListening = useStartListening(tab.id);
  const hasAttemptedAutoStart = useRef(false);

  useEffect(() => {
    if (hasAttemptedAutoStart.current) {
      return;
    }

    if (!canStartLiveSession) {
      return;
    }

    if (!conn) {
      return;
    }

    hasAttemptedAutoStart.current = true;
    startListening();
    updateSessionTabState(tab, { ...tab.state, autoStart: null });
  }, [
    tab,
    tab.state,
    canStartLiveSession,
    conn,
    startListening,
    updateSessionTabState,
  ]);

  return null;
}

function TabContentNoteInner({
  tab,
  standaloneWindow,
  audioUrlReady,
  audioExists,
}: {
  tab: Extract<Tab, { type: "sessions" }>;
  standaloneWindow: boolean;
  audioUrlReady: boolean;
  audioExists: boolean;
}) {
  const noteInputRef = React.useRef<NoteInputHandle>(null);

  const sessionId = tab.id;
  usePendingUpload(sessionId);

  const hasTranscript = useHasTranscript(sessionId);
  const sessionMode = useListener((state) => state.getSessionMode(sessionId));
  const batchError = useListener((state) => state.batch[sessionId]?.error);
  const hasLiveSegments = useListener(
    (state) =>
      state.live.sessionId === sessionId && state.liveSegments.length > 0,
  );
  const canShowTranscript = getCanShowTranscript({
    audioExists,
    batchError: Boolean(batchError),
    hasLiveSegments,
    hasTranscript,
    sessionMode,
  });
  const enhancedNotes = useEnhancedNoteRecords(sessionId);
  const enhancedNoteIds = React.useMemo(
    () => enhancedNotes.map((note) => note.id),
    [enhancedNotes],
  );
  const defaultEnhancedNoteId = React.useMemo(
    () =>
      enhancedNotes.find((note) => hasStoredNoteContent(note.content))?.id ??
      null,
    [enhancedNotes],
  );
  const session = useSession(sessionId);
  const rawMd = useSessionRawMd(sessionId);
  const updateSessionTabState = useTabs((state) => state.updateSessionTabState);

  useAutoEnhance(tab);
  const isTranscribing = shouldShowTranscriptTabSpinner(sessionMode);
  const isLiveSessionActive = sessionMode === "active";
  const editorTabs = React.useMemo(
    () =>
      createEditorTabs({
        enhancedNoteIds,
        canShowTranscript,
      }),
    [enhancedNoteIds, canShowTranscript],
  );
  const currentView = React.useMemo(() => {
    return computeCurrentNoteTab(
      tab.state.view ?? null,
      isLiveSessionActive,
      enhancedNoteIds,
      canShowTranscript,
      defaultEnhancedNoteId,
    );
  }, [
    tab.state.view,
    isLiveSessionActive,
    enhancedNoteIds,
    canShowTranscript,
    defaultEnhancedNoteId,
  ]);
  useAutoFocusTitle({ sessionId, noteInputRef });

  const showTopAudioPlayer = shouldShowSessionTopAudioPlayer({
    audioExists,
    audioUrlReady,
    sessionMode,
  });

  const handleTabChange = React.useCallback(
    (view: typeof currentView) => {
      noteInputRef.current?.prepareForTabChange();
      updateSessionTabState(tab, { ...tab.state, view });
    },
    [tab, updateSessionTabState],
  );
  return (
    <>
      <SessionSurface
        header={
          <OuterHeader
            sessionId={sessionId}
            currentView={currentView}
            standaloneWindow={standaloneWindow}
            title={
              <div className="flex min-w-0 items-center gap-3">
                <div className="min-w-0 shrink">
                  <NoteInputHeader
                    sessionId={sessionId}
                    editorTabs={editorTabs}
                    currentTab={currentView}
                    handleTabChange={handleTabChange}
                    isTranscribing={isTranscribing}
                  />
                </div>
                {showTopAudioPlayer ? (
                  <>
                    <div
                      data-session-top-audio-player
                      className="border-border bg-card h-7 w-56 shrink-0 overflow-hidden rounded-full border @max-[36rem]:hidden"
                    >
                      <AudioPlayer.Timeline contentClassName="gap-2 py-[3px] pr-2.5 pl-[3px]" />
                    </div>
                    <div className="shrink-0 @[36rem]:hidden">
                      <AudioPlayer.CompactPlayButton />
                    </div>
                  </>
                ) : null}
              </div>
            }
          />
        }
      >
        {session && rawMd !== null ? (
          <NoteInput
            ref={noteInputRef}
            tab={tab}
            rawMd={rawMd}
            sessionTitle={session.title}
            editorTabs={editorTabs}
            currentTab={currentView}
            handleTabChange={handleTabChange}
            sessionMode={sessionMode}
            hideHeader
          />
        ) : (
          <SessionContentLoading />
        )}
      </SessionSurface>
    </>
  );
}

function SessionContentLoading() {
  return (
    <div className="flex h-full flex-col gap-3 px-4 py-5">
      <div className="bg-muted h-5 w-3/5 animate-pulse rounded-md" />
      <div className="bg-muted/80 h-4 w-4/5 animate-pulse rounded-md" />
      <div className="bg-muted/70 h-4 w-2/3 animate-pulse rounded-md" />
    </div>
  );
}

function usePendingUpload(sessionId: string) {
  const { processFile } = useUploadFile(sessionId);
  const processFileRef = useRef(processFile);
  processFileRef.current = processFile;

  useEffect(() => {
    const pending = consumePendingUpload(sessionId);
    if (pending) {
      processFileRef.current(pending.filePath, pending.kind);
    }
  }, [sessionId]);
}

function useAutoFocusTitle({
  sessionId,
  noteInputRef,
}: {
  sessionId: string;
  noteInputRef: React.RefObject<NoteInputHandle | null>;
}) {
  const autoFocusedSessionRef = useRef<string | null>(null);
  const title = useSession(sessionId)?.title;

  useEffect(() => {
    if (autoFocusedSessionRef.current === sessionId) return;

    if (!title) {
      noteInputRef.current?.focusAtStart();
      autoFocusedSessionRef.current = sessionId;
    }
  }, [sessionId, title]);
}
