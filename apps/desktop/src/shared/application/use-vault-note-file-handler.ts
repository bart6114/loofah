import { useCallback, useMemo, useRef } from "react";

import {
  handleNativeEditorDrag,
  handleNativeFileDrop,
  type FileHandlerConfig,
  type NoteEditorProps,
  type NoteEditorRef,
} from "@hypr/editor/note";

import { deviceFileName } from "./device-attachment-source";
import {
  useVaultFileUpload,
  useVaultFileUploadStates,
} from "./use-vault-file-upload";
import type { VaultScope } from "./vault-scope";

import { useNativeFileDrop } from "~/shared/hooks/useNativeFileDrop";

export function useVaultNoteFileHandler(
  scope: VaultScope,
  sessionId: string,
  enabled: boolean,
  callbacks?: Pick<NoteEditorProps, "onViewReady" | "onViewDisposed">,
) {
  const upload = useVaultFileUpload(scope, sessionId);
  const { remove } = useVaultFileUploadStates(scope, sessionId);
  const targetRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<NoteEditorRef["view"]>(null);
  const fileHandlerConfig = useMemo<FileHandlerConfig>(
    () => ({
      onFileUpload: async (candidate, clientId) => {
        const metadata = await upload(candidate, clientId);
        // Node views resolve attachment IDs through the current vault scope.
        return { attachmentId: metadata.filename, url: "", path: "" };
      },
      onFileUploadRemove: remove,
    }),
    [upload, remove],
  );
  useNativeFileDrop(targetRef, {
    enabled,
    onInternalDrag: (event) => {
      const view = viewRef.current;
      if (view?.editable) handleNativeEditorDrag(view, event);
    },
    onDrop: (paths, point) => {
      const view = viewRef.current;
      if (!view?.editable) return;
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
        })),
        pos,
      );
    },
  });
  const onViewReady = useCallback<NonNullable<NoteEditorProps["onViewReady"]>>(
    (view) => {
      viewRef.current = view;
      callbacks?.onViewReady?.(view);
    },
    [callbacks?.onViewReady],
  );
  const onViewDisposed = useCallback<
    NonNullable<NoteEditorProps["onViewDisposed"]>
  >(
    (view) => {
      if (viewRef.current === view) viewRef.current = null;
      callbacks?.onViewDisposed?.(view);
    },
    [callbacks?.onViewDisposed],
  );
  return {
    targetRef,
    fileHandlerConfig: enabled ? fileHandlerConfig : undefined,
    onViewReady: enabled ? onViewReady : callbacks?.onViewReady,
    onViewDisposed: enabled ? onViewDisposed : callbacks?.onViewDisposed,
  };
}
