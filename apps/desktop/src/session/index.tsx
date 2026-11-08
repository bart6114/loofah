import React, { useCallback, useEffect, useRef } from "react";

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
import { deleteSessionAudio } from "~/session/attachments";
import {
  useEnhancedNoteRecords,
  useSession,
  useSessionRawMd,
  useSessionMetadata,
} from "~/session/queries";
import { useVaultHost } from "~/shared/application/vault-provider";
import { listenerStore } from "~/store/zustand/listener/instance";
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
  const { scope } = useVaultHost();
  const isCapturing = sessionMode === "active" || sessionMode === "finalizing";
  const deleteRecording = useCallback(
    async (remove: () => Promise<void>) => {
      const deleted = await deleteSessionAudio(
        tab.id,
        () => {
          const state = listenerStore.getState();
          return (
            state.getSessionMode(tab.id) === "inactive" &&
            !(state.live.sessionId === tab.id && state.live.loading)
          );
        },
        remove,
      );
      if (!deleted) throw new Error("audio_session_busy");
    },
    [tab.id],
  );
  const wasCapturingRef = useRef(false);
  useEffect(() => {
    if (wasCapturingRef.current && !isCapturing) {
      for (const kind of ["audio_resource", "audio_peaks", "audio_metadata"]) {
        void scope?.queryClient.invalidateQueries({
          queryKey: ["vault", scope.client.scope, kind, tab.id],
        });
      }
    }
    wasCapturingRef.current = isCapturing;
  }, [isCapturing, scope, tab.id]);

  return (
    <>
      {tab.state.autoStart && !standaloneWindow ? (
        <AutoStartListening tab={tab} />
      ) : null}
      <SearchProvider>
        <AudioPlayer.Provider
          sessionId={tab.id}
          scope={scope}
          enabled={!isCapturing}
          onDeleteRecording={deleteRecording}
        >
          <TabContentNoteInner tab={tab} standaloneWindow={standaloneWindow} />
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
}: {
  tab: Extract<Tab, { type: "sessions" }>;
  standaloneWindow: boolean;
}) {
  const noteInputRef = React.useRef<NoteInputHandle>(null);
  const { audioReady: audioUrlReady, audioExists } =
    AudioPlayer.useAudioPlayer();

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
            onStartRecording={() => handleTabChange({ type: "raw" })}
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
                  <div className="@container/audio-player min-w-6 flex-1">
                    <div
                      data-session-top-audio-player
                      className="h-7 w-full @max-[14rem]/audio-player:hidden"
                    >
                      <AudioPlayer.Timeline contentClassName="gap-2.5" />
                    </div>
                    <div className="@[14rem]/audio-player:hidden">
                      <AudioPlayer.CompactPlayButton />
                    </div>
                  </div>
                ) : null}
              </div>
            }
          />
        }
      >
        <AudioPlayer.Status />
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
  const title = useSessionMetadata(sessionId)?.meta.title;

  useEffect(() => {
    if (autoFocusedSessionRef.current === sessionId) return;

    if (!title) {
      noteInputRef.current?.focusAtStart();
      autoFocusedSessionRef.current = sessionId;
    }
  }, [sessionId, title]);
}
