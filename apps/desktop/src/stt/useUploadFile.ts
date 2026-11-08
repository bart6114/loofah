import { useMutation } from "@tanstack/react-query";
import { downloadDir } from "@tauri-apps/api/path";
import { open as selectFile } from "@tauri-apps/plugin-dialog";
import { useCallback } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { deviceFileName } from "~/shared/application/device-attachment-source";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";

export const AUDIO_EXTENSIONS = [
  "wav",
  "mp3",
  "ogg",
  "mp4",
  "m4a",
  "flac",
  "webm",
  "aac",
];
const AUDIO_TRANSFER_EXTENSIONS = [...AUDIO_EXTENSIONS, "qta"];
const TRANSCRIPT_EXTENSIONS = ["vtt", "srt"];
function fileExtension(value: string) {
  const extension = value.toLowerCase().split(".").pop();
  return extension && extension !== value.toLowerCase() ? extension : "";
}
export function isAudioUploadFile(file: Pick<File, "name" | "type">) {
  return (
    AUDIO_TRANSFER_EXTENSIONS.includes(fileExtension(file.name)) ||
    file.type.startsWith("audio/")
  );
}

export function useUploadFile(
  sessionId: string,
  assertCurrent?: () => void,
  enabled = true,
) {
  const owner = useVaultReadScope();
  const check = useCallback(() => {
    assertCurrent?.();
    if (
      !enabled ||
      !owner.current() ||
      owner.scope.getSnapshot().state !== "connected" ||
      !owner.scope.client.media
    )
      throw new Error("Media import is unavailable for this vault.");
  }, [assertCurrent, enabled, owner.current, owner.scope]);
  const mutation = useMutation({
    retry: false,
    networkMode: "always",
    mutationFn: async (input: {
      kind: "audio" | "transcript";
      source:
        | { kind: "path"; path: string; name: string }
        | { kind: "file"; file: File; name: string };
      createdAt?: string;
      contentType?: string;
    }) => {
      check();
      const media = owner.scope.client.media!;
      const requestId = crypto.randomUUID();
      const prepared = await owner.scope.prepareAttachmentSource(input.source);
      try {
        check();
        const result = await owner.scope.run(() =>
          input.kind === "audio"
            ? media.importAudio(
                {
                  sessionId,
                  prepared,
                  createdAt: input.createdAt,
                  contentType: input.contentType,
                },
                { requestId },
              )
            : media.importSubtitles(sessionId, prepared, { requestId }),
        );
        check();
        return result;
      } finally {
        await prepared.close();
      }
    },
    onError: (error) => {
      if (owner.current())
        sonnerToast.error("Media import failed", {
          description: error instanceof Error ? error.message : String(error),
        });
    },
  });
  const submit = mutation.mutateAsync;
  const processFile = useCallback(
    (path: string, kind: "audio" | "transcript") => {
      check();
      if (
        !(
          kind === "audio" ? AUDIO_TRANSFER_EXTENSIONS : TRANSCRIPT_EXTENSIONS
        ).includes(fileExtension(path))
      )
        return;
      return submit({ kind, source: { kind: "path", path } });
    },
    [check, submit],
  );
  const processAudioFile = useCallback(
    (
      file: File,
      options?: { allowUnknownAudio?: boolean; contentType?: string },
    ) => {
      check();
      if (!options?.allowUnknownAudio && !isAudioUploadFile(file)) return;
      const path = (file as { path?: unknown }).path;
      return submit({
        kind: "audio",
        source:
          typeof path === "string" && path.trim()
            ? { kind: "path", path, name: deviceFileName(path) }
            : { kind: "file", file, name: file.name },
        createdAt:
          Number.isFinite(file.lastModified) && file.lastModified > 0
            ? new Date(file.lastModified).toISOString()
            : undefined,
        contentType: options?.contentType || file.type || undefined,
      });
    },
    [check, submit],
  );
  const processAudioPath = useCallback(
    (path: string) => processFile(path, "audio"),
    [processFile],
  );
  const selectAndUpload = useCallback(
    async (kind: "audio" | "transcript") => {
      check();
      const defaultPath = await downloadDir();
      check();
      const selection = await selectFile({
        title: kind === "audio" ? "Upload Audio" : "Upload Transcript",
        multiple: false,
        directory: false,
        defaultPath,
        filters: [
          {
            name: kind === "audio" ? "Audio" : "Transcript",
            extensions:
              kind === "audio" ? AUDIO_EXTENSIONS : TRANSCRIPT_EXTENSIONS,
          },
        ],
      });
      check();
      const path = Array.isArray(selection) ? selection[0] : selection;
      if (path) return processFile(path, kind);
    },
    [check, processFile],
  );
  return {
    uploadAudio: useCallback(() => selectAndUpload("audio"), [selectAndUpload]),
    uploadTranscript: useCallback(
      () => selectAndUpload("transcript"),
      [selectAndUpload],
    ),
    processFile,
    processAudioFile,
    processAudioPath,
  };
}
