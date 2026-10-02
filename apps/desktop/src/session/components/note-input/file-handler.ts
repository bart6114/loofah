import { convertFileSrc } from "@tauri-apps/api/core";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
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

import {
  useFileUpload,
  useFileUploadStates,
} from "~/shared/hooks/useFileUpload";
import { useNativeFileDrop } from "~/shared/hooks/useNativeFileDrop";
import { isAudioUploadFile, useUploadFile } from "~/stt/useUploadFile";

export type NoteFileDragKind = "audio" | "files" | "mixed";

export function useNoteFileHandlerConfig(
  sessionId: string,
  editorRef: RefObject<NoteEditorRef | null>,
) {
  const onFileUpload = useFileUpload(sessionId);
  const uploadStates = useFileUploadStates(sessionId);
  const { processAudioFile, processAudioPath } = useUploadFile(sessionId);
  const [fileDragKind, setFileDragKind] = useState<NoteFileDragKind | null>(
    null,
  );
  const fileDropTargetRef = useRef<HTMLDivElement>(null);

  const processAudioDrop = useCallback(
    (files: File[], items?: DataTransferItemList) => {
      const audioDrop = getAudioDrop(files, items);
      if (!audioDrop) return undefined;

      if (audioDrop.allowUnknownAudio) {
        processAudioFile(audioDrop.audioFile, {
          allowUnknownAudio: true,
          contentType: audioDrop.contentType,
        });
      } else {
        processAudioFile(audioDrop.audioFile);
      }
      return { remainingFiles: audioDrop.remainingFiles };
    },
    [processAudioFile],
  );

  const processNativeAudioDrop = useCallback(
    (candidates: FileUploadCandidate[]) => {
      const audioIndex = candidates.findIndex((candidate) =>
        isAudioUploadFile({ name: candidate.name, type: "" }),
      );
      if (audioIndex === -1) return undefined;

      const audio = candidates[audioIndex];
      if (audio.kind === "path") processAudioPath(audio.path);
      return {
        remainingCandidates: candidates.filter(
          (_, index) => index !== audioIndex,
        ),
      };
    },
    [processAudioPath],
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
      onFileUpload,
      onFileUploadError: handleFileUploadError,
      onFileUploadRemove: uploadStates.remove,
      onDrop: (files, _pos, items) => processAudioDrop(files, items),
      onNativeDrop: processNativeAudioDrop,
      onPaste: processAudioDrop,
    }),
    [
      handleFileUploadError,
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
    onInternalDrag: (event) => {
      const view = editorRef.current?.view;
      if (view) handleNativeEditorDrag(view, event);
    },
    onHoverPaths: (paths) => setFileDragKind(classifyNativePaths(paths)),
    onHoverEnd: resetFileDrag,
    onDrop: (paths, point) => {
      const view = editorRef.current?.view;
      if (!view) return;
      const pos =
        view.posAtCoords({ left: point.x, top: point.y })?.pos ??
        view.state.doc.content.size;
      handleNativeFileDrop(
        view,
        fileHandlerConfig,
        paths.map((path) => ({
          kind: "path",
          path,
          name: fileNameFromPath(path),
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

function fileNameFromPath(path: string) {
  return path.split("/").pop() ?? path;
}

function isImagePath(path: string) {
  return /\.(gif|jpe?g|png|webp)$/i.test(path);
}
