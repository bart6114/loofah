import { ApplicationRequestError } from "./errors";
import type { MediaDeclaration, MediaRequestStatus } from "./protocol.gen";
import type { NativeRemoteConnection } from "./remote";
import type { MediaStatus, RequestOptions, VaultClient } from "./vault-client";

import type { MediaQuery, MediaReply } from "~/types/tauri.gen";

export function remoteMedia(
  connection: NativeRemoteConnection,
  vaultId: string,
): NonNullable<VaultClient["media"]> {
  const query = <K extends MediaReply["kind"]>(
    input: MediaQuery,
    expected: K,
    options?: RequestOptions,
  ) => connection.mediaRequest(vaultId, input, expected, options);
  const status = (id: string, options?: RequestOptions) =>
    query({ kind: "status", request_id: id }, "status", options);
  const saved = async (options: RequestOptions & { requestId: string }) => {
    const receipt = (await connection.mediaSubmissions(vaultId, options)).find(
      (value) => value.declaration.request_id === options.requestId,
    );
    return receipt
      ? mediaStatus(await status(options.requestId, options))
      : null;
  };
  const settings = async (transcribe: boolean, options?: RequestOptions) => {
    const preferences = await connection.request(
      { kind: "processing_settings", vault_id: vaultId },
      "processing_settings",
      options,
    );
    const policy = transcribe
      ? await connection.request(
          { kind: "transcription_policy", vault_id: vaultId },
          "transcription_policy",
          options,
        )
      : null;
    return {
      transcription:
        policy?.batch_enabled && policy.availability === "ready" && policy.model
          ? { model: policy.model, languages: [] }
          : null,
      summary: preferences.settings.summary_model
        ? {
            target: { kind: "summary" as const },
            title_policy: "generate_if_empty" as const,
          }
        : null,
    };
  };
  return {
    exportSource: connection.info.capabilities.features.includes(
      "media_source_export",
    )
      ? (id, options) => connection.exportMediaSource(vaultId, id, options)
      : undefined,
    pendingSubmissions: async (options) =>
      (await connection.mediaSubmissions(vaultId, options)).map(
        ({ declaration }) => ({
          request_id: declaration.request_id,
          filename:
            declaration.operation.kind === "transcribe"
              ? null
              : declaration.operation.filename,
          session_id:
            declaration.operation.kind === "transcribe"
              ? declaration.operation.expected.session_id
              : declaration.operation.session_id,
        }),
      ),
    source: async (sessionId, options) => {
      const source = await query(
        { kind: "source", session_id: sessionId },
        "source",
        options,
      );
      if (!source.recording) throw invalid("This note has no saved recording");
      return source.recording;
    },
    importAudio: async (input, options) => {
      const previous = await saved(options);
      if (previous) return previous;
      const expected = input.sessionId
        ? await query(
            { kind: "source", session_id: input.sessionId },
            "source",
            options,
          )
        : null;
      const processing = await settings(true, options);
      const declaration: MediaDeclaration = {
        request_id: options.requestId,
        operation: {
          kind: "import_audio",
          session_id: input.sessionId,
          expected,
          filename: input.prepared.input.filename,
          content_type: input.contentType ?? null,
          created_at: input.createdAt ?? null,
          transcribe: !!processing.transcription,
          source: {
            sha256: input.prepared.input.sha256,
            size_bytes: input.prepared.input.size_bytes,
          },
        },
        ...processing,
      };
      return mediaStatus(
        await connection.submitMedia(
          vaultId,
          declaration,
          input.prepared.source,
          options,
        ),
      );
    },
    retranscribe: async (expected, options) => {
      const previous = await saved(options);
      if (previous) return previous;
      const processing = await settings(true, options);
      if (!processing.transcription)
        throw invalid("Server transcription is not ready for this connection");
      return mediaStatus(
        await connection.submitMedia(
          vaultId,
          {
            request_id: options.requestId,
            operation: { kind: "transcribe", expected },
            ...processing,
          },
          null,
          options,
        ),
      );
    },
    importSubtitles: async (sessionId, prepared, options) => {
      const previous = await saved(options);
      if (previous) return previous;
      const expected = await query(
        { kind: "source", session_id: sessionId },
        "source",
        options,
      );
      const processing = await settings(false, options);
      return mediaStatus(
        await connection.submitMedia(
          vaultId,
          {
            request_id: options.requestId,
            operation: {
              kind: "import_subtitles",
              session_id: sessionId,
              expected,
              filename: prepared.input.filename,
              source: {
                sha256: prepared.input.sha256,
                size_bytes: prepared.input.size_bytes,
              },
            },
            ...processing,
          },
          prepared.source,
          options,
        ),
      );
    },
    list: async (sessionId, options) =>
      (
        await query({ kind: "list", session_id: sessionId }, "list", options)
      ).map(mediaStatus),
    status: async (id, options) => mediaStatus(await status(id, options)),
    result: (id, options) =>
      query({ kind: "result", request_id: id }, "result", options),
    cancel: async (id, options) =>
      mediaStatus(await connection.cancelMedia(vaultId, id, options)),
    recover: async (id, options) =>
      mediaStatus(await connection.recoverMedia(vaultId, id, options)),
    resumeSource: async (id, prepared, options) =>
      mediaStatus(
        await connection.resumeMediaSource(
          vaultId,
          id,
          prepared.source,
          options,
        ),
      ),
    summaryStatus: (id, options) =>
      query(
        { kind: "summary_status", request_id: id },
        "summary_status",
        options,
      ),
  };
}

function mediaStatus(status: MediaRequestStatus): MediaStatus {
  if (!status.media)
    throw invalid("The server did not return the original media reservation");
  return {
    ...status.media,
    state:
      status.phase === "receiving" || status.phase === "ready"
        ? status.phase
        : status.media.state,
  };
}

function invalid(message: string) {
  return new ApplicationRequestError({
    code: "validation",
    message,
    request_id: null,
    retryable: false,
    current_revision: null,
    mutation_outcome: "not_committed",
  });
}
