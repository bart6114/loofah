import { useMutation, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

export type Snapshot = {
  vault: { local_path: string; icloud_path: string | null };
  startup_errors?: string[];
  sessions: {
    id: string;
    title: string;
    created_at: string;
    has_transcript_words: boolean;
  }[];
  recording: {
    session_id: string | null;
    stopping?: boolean;
    elapsed_seconds: number;
    interrupted: boolean;
    error: string | null;
  };
  jobs: {
    session_id: string;
    kind: "transcribe" | "summary" | "title";
    state: "queued" | "running" | "paused" | "failed";
    progress: number;
    error: string | null;
  }[];
  model: {
    ready: boolean;
    downloading: boolean;
    phase: "idle" | "downloading" | "verifying" | "ready";
    downloaded_bytes: number;
    total_bytes: number;
    model_id: string;
    revision: string;
  };
  summary: { available: boolean; reason: string | null };
  sync: {
    connected: boolean;
    state: string;
    pending: number;
    error: string | null;
    conflicts: { id: string; path: string }[];
  };
  settings: {
    transcription_model: "parakeet-v3" | "parakeet-v2";
    summary_provider: string;
    summary_model: string;
    summary_base_url: string;
    providers: Record<string, { base_url: string; has_api_key: boolean }>;
    summary_language: string;
    has_api_key: boolean;
  };
};
export type Session = {
  id: string;
  title: string;
  notes: string;
  summary: string | null;
  transcript: { text: string; start: number; end: number }[];
  tasks: { id: string; title: string; completed: boolean }[];
  audio_url: string | null;
  has_audio?: boolean;
  attachments?: { name: string; relative_path: string; url: string | null }[];
};
export const snapshotOptions = {
  queryKey: ["mobile", "snapshot"],
  queryFn: () => invoke<Snapshot>("mobile_snapshot"),
};
export function useEvents() {
  const client = useQueryClient();
  useEffect(() => {
    let disposed = false;
    const subscriptions = ["index-changed", "mobile-state-changed"].map(
      (event) =>
        listen(event, () => {
          void client.invalidateQueries({ queryKey: ["mobile"] });
        }),
    );
    for (const subscription of subscriptions)
      void subscription
        .then((unlisten) => {
          if (disposed) unlisten();
        })
        .catch(() => {});
    return () => {
      disposed = true;
      for (const subscription of subscriptions)
        void subscription.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [client]);
}
export function useCommand<T = void>(command: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (args: Record<string, unknown> = {}) =>
      invoke<T>(command, args),
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ["mobile"] });
    },
  });
}
export function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
export function visibleJobError(
  job: Snapshot["jobs"][number],
  modelReady: boolean,
) {
  return job.kind === "transcribe" &&
    modelReady &&
    job.error?.startsWith("Download the transcription model")
    ? null
    : job.error;
}
export function time(seconds: number) {
  return `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, "0")}`;
}

export function syncLabel(state: string) {
  return state === "idle" ? "iCloud folder up to date" : state;
}
