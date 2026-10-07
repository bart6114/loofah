import { useQueryClient } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";

import {
  commands as fsSyncCommands,
  events as fsSyncEvents,
} from "@hypr/plugin-fs-sync";
import { commands as notificationCommands } from "@hypr/plugin-notification";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY } from "./audio-import-notification";
import { estimateUploadedAudioSessionCreatedAt } from "./audio-note-date";
import { useListener } from "./contexts";
import { isStoppedTranscriptionError, useRunBatch } from "./useRunBatch";

import { getEnhancerService } from "~/services/enhancer";
import { enqueueSessionAudioOperation } from "~/session/audio-operations";
import { createSession, updateSession, useSession } from "~/session/queries";
import {
  type AudioImportItem,
  type AudioImportSource,
  isFinishedAudioImportStatus,
  useAudioImport,
} from "~/store/zustand/audio-import";

// Effect-run guards keyed by item+attempt so StrictMode remounts (and later
// re-renders with changed hook identities) never double-run a pipeline step.
const startedSteps = new Set<string>();

export function AudioImportWorker() {
  const activeItem = useAudioImport(
    (state) =>
      state.items.find((item) => item.id === state.activeItemId) ?? null,
  );
  const hasPending = useAudioImport((state) =>
    state.items.some((item) => item.status === "pending"),
  );
  const shouldAnnounceCompletion = useAudioImport(
    (state) =>
      !state.completionAnnounced &&
      state.activeItemId === null &&
      state.items.length > 0 &&
      state.items.every((item) => isFinishedAudioImportStatus(item.status)),
  );

  useEffect(() => {
    if (!activeItem && hasPending) {
      useAudioImport.getState().claimNext();
    }
  }, [activeItem, hasPending]);

  useEffect(() => {
    if (!shouldAnnounceCompletion) {
      return;
    }

    useAudioImport.getState().markCompletionAnnounced();
    const items = useAudioImport.getState().items;
    const done = items.filter((item) => item.status === "done").length;
    const failed = items.filter((item) => item.status === "failed").length;
    const lastImportedSessionId =
      [...items].reverse().find((item) => item.status === "done")?.sessionId ??
      null;
    void announceImportCompleted(done, failed, lastImportedSessionId);
  }, [shouldAnnounceCompletion]);

  if (!activeItem) {
    return null;
  }

  return (
    <ItemSessionCreator
      key={`${activeItem.id}:${activeItem.attempt}`}
      item={activeItem}
    />
  );
}

function ItemSessionCreator({ item }: { item: AudioImportItem }) {
  const sessionId = item.sessionId;

  useEffect(() => {
    if (sessionId || !acquireStep(`create:${item.id}:${item.attempt}`)) {
      return;
    }

    void prepareSession(item.source).then(
      (createdSessionId) =>
        useAudioImport.getState().setItemSession(item.id, createdSessionId),
      (error) => {
        console.error("[audio-import] failed to create session:", error);
        useAudioImport.getState().finishItem(item.id, errorMessage(error));
      },
    );
  }, [item, sessionId]);

  if (!sessionId) {
    return null;
  }

  return <ItemPipeline item={item} sessionId={sessionId} />;
}

function ItemPipeline({
  item,
  sessionId,
}: {
  item: AudioImportItem;
  sessionId: string;
}) {
  const queryClient = useQueryClient();
  const session = useSession(sessionId);
  const runBatch = useRunBatch(sessionId);
  const handleBatchStarted = useListener((state) => state.handleBatchStarted);
  const handleBatchFailed = useListener((state) => state.handleBatchFailed);
  const updateBatchProgress = useListener((state) => state.updateBatchProgress);
  const clearBatchSession = useListener((state) => state.clearBatchSession);
  const batchPercentage = useListener(
    (state) => state.batch[sessionId]?.percentage ?? null,
  );

  useEffect(() => {
    if (batchPercentage != null) {
      useAudioImport.getState().setItemProgress(item.id, batchPercentage);
    }
  }, [batchPercentage, item.id]);

  const sessionReady = session != null;

  useEffect(() => {
    if (!sessionReady || !acquireStep(`run:${item.id}:${item.attempt}`)) {
      return;
    }

    const run = async () => {
      const store = useAudioImport.getState();
      store.setItemStatus(item.id, "importing");
      handleBatchStarted(sessionId, "importing");

      const importedPath = await importAudioToSession(
        sessionId,
        item.source,
        (percentage) => updateBatchProgress(sessionId, percentage),
      );

      void queryClient.invalidateQueries({
        queryKey: ["audio", sessionId],
      });

      clearBatchSession(sessionId);
      store.setItemStatus(item.id, "transcribing");
      await runBatch(importedPath, { imported: true });

      // Summarization intentionally overlaps the next item's transcription:
      // auto-enhance is fire-and-forget, matching the single-file upload flow.
      void Promise.resolve(
        getEnhancerService()?.queueAutoEnhanceIfSummaryEmpty(sessionId),
      ).catch((error) => {
        console.error("[audio-import] failed to queue enhance:", error);
      });
    };

    run().then(
      () => useAudioImport.getState().finishItem(item.id),
      (error: unknown) => {
        console.error("[audio-import] item failed:", error);
        if (!isStoppedTranscriptionError(error)) {
          handleBatchFailed(sessionId, errorMessage(error));
        }
        useAudioImport.getState().finishItem(item.id, errorMessage(error));
      },
    );
  }, [
    clearBatchSession,
    handleBatchFailed,
    handleBatchStarted,
    item.attempt,
    item.id,
    item.source,
    queryClient,
    runBatch,
    sessionId,
    sessionReady,
    updateBatchProgress,
  ]);

  return null;
}

function acquireStep(key: string) {
  if (startedSteps.has(key)) {
    return false;
  }

  startedSteps.add(key);
  return true;
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function sessionTitleFromFileName(name: string) {
  return name.replace(/\.[^.]+$/, "");
}

async function prepareSession(source: AudioImportSource) {
  const sessionId = await createSession(sessionTitleFromFileName(source.name));

  try {
    const createdAt = await estimateImportedAudioCreatedAt(source);
    if (createdAt) {
      await updateSession(sessionId, { created_at: createdAt });
    }
  } catch (error) {
    console.error("[audio-import] audio date inspection failed:", error);
  }

  return sessionId;
}

async function estimateImportedAudioCreatedAt(source: AudioImportSource) {
  if (source.kind === "path") {
    const result = await fsSyncCommands.audioSourceMetadata(source.path);
    if (result.status === "error") {
      return null;
    }
    return estimateUploadedAudioSessionCreatedAt(result.data);
  }

  const lastModified = source.file.lastModified;
  if (!Number.isFinite(lastModified) || lastModified <= 0) {
    return null;
  }
  return new Date(lastModified).toISOString();
}

function importAudioToSession(
  sessionId: string,
  source: AudioImportSource,
  onProgress: (percentage: number) => void,
): Promise<string> {
  return enqueueSessionAudioOperation(sessionId, async () => {
    const unlisten = await fsSyncEvents.audioImportEvent.listen((event) => {
      if (
        event.payload.type === "audioImportProgress" &&
        event.payload.session_id === sessionId
      ) {
        onProgress(event.payload.percentage);
      }
    });

    try {
      const result =
        source.kind === "path"
          ? await fsSyncCommands.audioImport(sessionId, source.path)
          : await fsSyncCommands.audioImportData(
              sessionId,
              Array.from(new Uint8Array(await source.file.arrayBuffer())),
              source.file.name,
              source.file.type || null,
            );
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    } finally {
      unlisten();
    }
  });
}

async function announceImportCompleted(
  done: number,
  failed: number,
  lastImportedSessionId: string | null,
) {
  const total = done + failed;
  const message =
    failed > 0
      ? `Imported ${done} of ${total} files. ${failed} failed.`
      : done === 1
        ? "Imported 1 file."
        : `Imported ${done} files.`;

  if (await isMainWindowVisibleAndFocused()) {
    if (failed > 0) {
      sonnerToast.warning("Audio import finished", { description: message });
    } else {
      sonnerToast.success("Audio import finished", { description: message });
    }
    return;
  }

  try {
    // A session source makes the notification click open that note; without
    // one (nothing succeeded) the click reopens the import dialog instead.
    const result = await notificationCommands.showNotification({
      key: AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY,
      title: "Audio import finished",
      message,
      timeout: null,
      source: lastImportedSessionId
        ? { type: "session", session_id: lastImportedSessionId }
        : null,
      start_time: null,
      participants: null,
      action_label: lastImportedSessionId ? "Open note" : "Open",
      action_variant: null,
      options: null,
      footer: null,
      icon: null,
    });
    if (result.status === "error") {
      console.error(
        "[audio-import] failed to show completion notification",
        result.error,
      );
    }
  } catch (error) {
    console.error(
      "[audio-import] failed to show completion notification",
      error,
    );
  }
}

async function isMainWindowVisibleAndFocused() {
  try {
    const window = getCurrentWindow();
    const [focused, visible] = await Promise.all([
      window.isFocused(),
      window.isVisible(),
    ]);
    return focused && visible;
  } catch (error) {
    console.error("[audio-import] failed to inspect window state", error);
    return false;
  }
}
