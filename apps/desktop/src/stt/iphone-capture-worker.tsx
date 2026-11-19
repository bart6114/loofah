import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { useRunBatch } from "./useRunBatch";
import { useSTTConnection } from "./useSTTConnection";

import { useLLMConnection } from "~/ai/hooks/useLLMConnection";
import { getEnhancerService } from "~/services/enhancer";
import { enqueueSessionAudioOperation } from "~/session/audio-operations";
import { useSession } from "~/session/queries";
import { useIndexQuery } from "~/shared/index-query";
import { type CaptureStage, commands } from "~/types/tauri.gen";

let inFlight = false;

function data<T>(
  result: { status: "ok"; data: T } | { status: "error"; error: string },
) {
  if (result.status === "error") throw new Error(result.error);
  return result.data;
}

export function IphoneCaptureWorker() {
  const client = useQueryClient();
  const { conn: stt } = useSTTConnection();
  const { conn: llm, status } = useLLMConnection();
  const noSummaryModel =
    status.status === "pending" &&
    (status.reason === "missing_provider" || status.reason === "missing_model");
  const [paused, setPaused] = useState<ReadonlySet<string>>(() => new Set());
  const jobs = useIndexQuery({
    entity: ["session_headers", "artifacts"],
    queryKey: ["iphone-capture-jobs"],
    queryFn: async () => data(await commands.iphoneCaptureJobs()),
  });
  const job = jobs.data?.find(([id, stage]) => {
    const key = stageKey(id, stage);
    if (paused.has(key)) return false;
    return stage === "transcribe"
      ? Boolean(stt)
      : Boolean(llm || noSummaryModel);
  });
  const id = job?.[0] ?? "";
  const stage = job?.[1];
  const session = useSession(id);
  const runBatch = useRunBatch(id);
  const { isPending, mutate } = useMutation({
    networkMode: "always",
    mutationFn: async ({ id, stage }: { id: string; stage: CaptureStage }) => {
      if (stage === "transcribe") {
        await enqueueSessionAudioOperation(id, async () => {
          const complete = data(await commands.sessionHasTranscript(id));
          const audio = complete
            ? null
            : data(await commands.iphoneCaptureAudio(id));
          if (!complete && !audio)
            throw new Error("The iPhone recording is missing from the vault.");
          if (audio)
            await runBatch(audio, {
              imported: true,
              persistAfterCompletion: true,
            });
        });
      } else if (!noSummaryModel) {
        const service = getEnhancerService();
        if (!service || !(await service.finishImportedCaptureSummary(id))) {
          throw new Error(
            "The summary is pending. Try again after Intelligence is ready.",
          );
        }
      }
      data(await commands.iphoneCaptureAdvance(id, stage));
    },
    onError: (error, { id, stage }) => {
      const key = stageKey(id, stage);
      setPaused((previous) => new Set(previous).add(key));
      sonnerToast.error(
        stage === "transcribe"
          ? "iPhone transcription needs attention"
          : "iPhone summary needs attention",
        {
          description: error.message,
          action: {
            label: "Retry",
            onClick: () =>
              setPaused((previous) => {
                const next = new Set(previous);
                next.delete(key);
                return next;
              }),
          },
        },
      );
    },
    onSettled: async () => {
      try {
        await client.invalidateQueries({ queryKey: ["iphone-capture-jobs"] });
      } finally {
        inFlight = false;
      }
    },
  });

  useEffect(() => {
    if (!id || !stage || !session || isPending || inFlight) return;
    inFlight = true;
    mutate({ id, stage });
  }, [id, isPending, mutate, session, stage]);

  return null;
}

function stageKey(id: string, stage: CaptureStage) {
  return `${id}:${stage}`;
}
