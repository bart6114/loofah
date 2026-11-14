import { useMutation } from "@tanstack/react-query";

import { commands, events } from "@hypr/plugin-transcription";

import { useRegenerateTranscript } from "./actions";

import { useSessionTranscriptMetadata } from "~/stt/queries";

async function retryMissingAudio(sessionId: string, runId: string) {
  let unlisten: (() => void) | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      events.transcriptionEvent
        .listen(({ payload }) => {
          if (payload.session_id !== sessionId) return;
          if (payload.type === "failed") reject(new Error(payload.error));
          if (payload.type === "stopped")
            reject(new Error("Recovery stopped. Your progress is saved."));
          if (payload.type === "completed") resolve();
        })
        .then(async (stop) => {
          unlisten = stop;
          try {
            const result = await commands.retryMissingAudio(sessionId, runId);
            if (result.status === "error") reject(new Error(result.error));
          } catch (error) {
            reject(error);
          }
        })
        .catch(reject);
    });
  } finally {
    unlisten?.();
  }
}

export function IncompleteTranscript({ sessionId }: { sessionId: string }) {
  const { data } = useSessionTranscriptMetadata(sessionId);
  const regenerate = useRegenerateTranscript(sessionId);
  const retry = useMutation({
    mutationFn: (runId: string) => retryMissingAudio(sessionId, runId),
  });
  const stop = useMutation({
    mutationFn: async () => {
      const result = await commands.stopTranscription(sessionId);
      if (result.status === "error") throw new Error(result.error);
    },
  });
  const recoveries =
    data?.batch_recoveries.filter(
      (recovery) => recovery.status !== "complete",
    ) ?? [];
  if (!recoveries.length) return null;
  return (
    <div
      role="status"
      className="shrink-0 border-b border-amber-200 bg-amber-50 px-4 py-3 text-sm text-neutral-900"
    >
      <p className="font-medium">Incomplete transcript</p>
      <p>Your recovered text is saved. Missing audio:</p>
      {recoveries.map((recovery) => (
        <div key={recovery.run_id}>
          <ul className="my-2 max-h-28 overflow-auto">
            {recovery.unresolved.map((range) => (
              <li
                key={`${range.channel}:${range.start_sample}:${range.end_sample}`}
                title={range.reason}
              >
                Channel {range.channel + 1}: {formatTime(range.start_sample)}–
                {formatTime(range.end_sample)}
              </li>
            ))}
          </ul>
          <button
            className="underline disabled:opacity-50"
            disabled={retry.isPending}
            onClick={() => retry.mutate(recovery.run_id)}
          >
            {retry.isPending
              ? "Retrying missing audio…"
              : "Retry missing audio"}
          </button>
        </div>
      ))}
      {retry.isPending && (
        <button
          className="ml-3 underline"
          disabled={stop.isPending}
          onClick={() => stop.mutate()}
        >
          Stop recovery
        </button>
      )}
      {(retry.error || stop.error) && (
        <p role="alert" className="mt-2">
          {(retry.error || stop.error)?.message}
        </p>
      )}
      <button
        className="mt-2 block underline"
        disabled={retry.isPending}
        onClick={() => void regenerate()}
      >
        Transcribe full recording again
      </button>
    </div>
  );
}

function formatTime(sample: number) {
  const seconds = Math.floor(sample / 16000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
