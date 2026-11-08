import { onlineManager } from "@tanstack/react-query";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type HTMLAttributes,
  type RefObject,
} from "react";

import {
  type FileHandlerConfig,
  type FileUploadCandidate,
  handleNativeFileDrop,
  handleNativeEditorDrag,
  type NoteEditorRef,
} from "@hypr/editor/note";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { deviceFileName } from "~/shared/application/device-attachment-source";
import {
  useVaultReadScope,
  legacyTranscriptControlsAvailable,
} from "~/shared/application/use-vault-read-scope";
import {
  useFileUpload,
  useFileUploadStates,
} from "~/shared/hooks/useFileUpload";
import { useNativeFileDrop } from "~/shared/hooks/useNativeFileDrop";
import { useListenerStore } from "~/stt/contexts";
import { isAudioUploadFile, useUploadFile } from "~/stt/useUploadFile";

export type NoteFileDragKind = "audio" | "files" | "mixed";

export function useNoteFileHandlerConfig(
  sessionId: string,
  editorRef: RefObject<NoteEditorRef | null>,
) {
  const { scope, current, readable } = useVaultReadScope();
  const capture = useListenerStore();
  const mounted = useRef(false);
  const session = useRef(sessionId);
  session.current = sessionId;
  useLayoutEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const available = useCallback(
    () =>
      mounted.current &&
      current() &&
      session.current === sessionId &&
      scope.getSnapshot().state === "connected" &&
      scope.client.canEdit,
    [current, scope, sessionId],
  );
  const online = useSyncExternalStore(
    (changed) => onlineManager.subscribe(changed),
    () => onlineManager.isOnline(),
  );
  const attachAvailable = useCallback(
    () =>
      available() &&
      scope.client.features.includes("attachment_upload") &&
      (scope.client.networkMode === "always" || onlineManager.isOnline()),
    [available, scope],
  );
  const canUpload =
    readable &&
    scope.getSnapshot().state === "connected" &&
    (scope.client.networkMode === "always" || online) &&
    scope.client.canEdit &&
    scope.client.features.includes("attachment_upload");
  const audioAvailable = useCallback(
    () => available() && legacyTranscriptControlsAvailable(scope),
    [available, scope],
  );
  const assertAudio = useCallback(() => {
    const state = capture.getState();
    const mode = state.getSessionMode(sessionId);
    if (
      !audioAvailable() ||
      (state.live.sessionId === sessionId && state.live.loading) ||
      mode === "active" ||
      mode === "finalizing"
    )
      throw new Error("Audio import is unavailable for this session");
  }, [audioAvailable, capture, sessionId]);
  const onFileUpload = useFileUpload(sessionId);
  const uploadStates = useFileUploadStates(sessionId);
  const { processAudioFile, processAudioPath } = useUploadFile(
    sessionId,
    assertAudio,
  );
  const [fileDragKind, setFileDragKind] = useState<NoteFileDragKind | null>(
    null,
  );
  const fileDropTargetRef = useRef<HTMLDivElement>(null);

  const processAudioDrop = useCallback(
    (files: File[], items?: DataTransferItemList) => {
      if (!audioAvailable()) return undefined;
      assertAudio();
      const audioDrop = getAudioDrop(files, items);
      if (!audioDrop) return undefined;

      if (audioDrop.allowUnknownAudio) {
        void processAudioFile(audioDrop.audioFile, {
          allowUnknownAudio: true,
          contentType: audioDrop.contentType,
        });
      } else {
        void processAudioFile(audioDrop.audioFile);
      }
      return { remainingFiles: audioDrop.remainingFiles };
    },
    [processAudioFile, audioAvailable, assertAudio],
  );

  const processNativeAudioDrop = useCallback(
    (candidates: FileUploadCandidate[]) => {
      if (!audioAvailable()) return undefined;
      assertAudio();
      const audioIndex = candidates.findIndex((candidate) =>
        isAudioUploadFile({ name: candidate.name, type: "" }),
      );
      if (audioIndex === -1) return undefined;

      const audio = candidates[audioIndex];
      if (audio.kind === "path") void processAudioPath(audio.path);
      return {
        remainingCandidates: candidates.filter(
          (_, index) => index !== audioIndex,
        ),
      };
    },
    [processAudioPath, audioAvailable, assertAudio],
  );

  const handleFileUploadError = useCallback(
    (candidate: FileUploadCandidate, error: unknown) => {
      const detail = error instanceof Error ? `: ${error.message}` : "";
      sonnerToast.error(`Couldn’t attach “${candidate.name}”${detail}`, {
        id: `attachment-upload-failed:${sessionId}`,
      });
    },
    [sessionId],
  );

  const fileHandlerConfig = useMemo<FileHandlerConfig>(
    () => ({
      onFileUpload: async (candidate, clientId) => {
        if (!attachAvailable())
          throw new Error("Attachment upload is unavailable");
        return onFileUpload(candidate, clientId);
      },
      onFileUploadError: handleFileUploadError,
      onFileUploadRemove: uploadStates.remove,
      onDrop: (files, _pos, items) => processAudioDrop(files, items),
      onNativeDrop: processNativeAudioDrop,
      onPaste: processAudioDrop,
    }),
    [
      handleFileUploadError,
      attachAvailable,
      onFileUpload,
      processAudioDrop,
      processNativeAudioDrop,
      uploadStates.remove,
    ],
  );

  const resetFileDrag = useCallback(() => setFileDragKind(null), []);
  useEffect(() => {
    resetFileDrag();
    return resetFileDrag;
  }, [resetFileDrag, sessionId]);

  useNativeFileDrop(fileDropTargetRef, {
    enabled: canUpload,
    onInternalDrag: (event) => {
      const view = editorRef.current?.view;
      if (available() && view?.editable) handleNativeEditorDrag(view, event);
    },
    onHoverPaths: (paths) => {
      if (available())
        setFileDragKind(
          audioAvailable() ? classifyNativePaths(paths) : "files",
        );
    },
    onHoverEnd: resetFileDrag,
    onDrop: (paths, point) => {
      const view = editorRef.current?.view;
      if (!attachAvailable() || !view?.editable) return;
      const pos =
        view.posAtCoords({ left: point.x, top: point.y })?.pos ??
        view.state.doc.content.size;
      handleNativeFileDrop(
        view,
        fileHandlerConfig,
        paths.map((path) => ({
          kind: "path",
          path,
          name: deviceFileName(path),
          previewUrl: isImagePath(path) ? convertFileSrc(path) : undefined,
        })),
        pos,
      );
    },
  });

  const fileDropTargetProps = useMemo<HTMLAttributes<HTMLDivElement>>(
    () => ({}),
    [],
  );

  return useMemo(
    () => ({
      fileDragKind,
      fileDropTargetProps,
      fileDropTargetRef,
      fileHandlerConfig,
      resetFileDrag,
    }),
    [fileDragKind, fileDropTargetProps, fileHandlerConfig, resetFileDrag],
  );
}

function classifyNativePaths(paths: string[]): NoteFileDragKind {
  const audioCount = paths.filter((path) =>
    isAudioUploadFile({ name: path, type: "" }),
  ).length;
  if (paths.length === 1 && audioCount === 1) return "audio";
  if (audioCount > 0) return "mixed";
  return "files";
}

function getAudioDrop(files: File[], items?: DataTransferItemList) {
  const dataTransferItems = Array.from(items ?? []).filter(
    (item) => item.kind === "file",
  );
  const audioFileIndex = files.findIndex((file, index) =>
    isAudioDropFile(file, dataTransferItems[index]),
  );
  if (audioFileIndex === -1) return null;
  const audioFile = files[audioFileIndex];

  return {
    allowUnknownAudio: !isAudioUploadFile(audioFile),
    audioFile,
    contentType:
      audioFile.type || dataTransferItems[audioFileIndex]?.type || undefined,
    remainingFiles: files.filter((file) => file !== audioFile),
  };
}

function isAudioDropFile(file: File, item?: DataTransferItem) {
  return isAudioUploadFile(file) || item?.type.startsWith("audio/") === true;
}

function isImagePath(path: string) {
  return /\.(gif|jpe?g|png|webp)$/i.test(path);
}
