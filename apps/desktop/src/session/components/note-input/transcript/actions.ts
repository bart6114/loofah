import { useMutation } from "@tanstack/react-query";
import { useCallback } from "react";

import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { confirmRegenerateSpeakerReset } from "./regenerate-confirm";

import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { collectAssignedHumanIdsFromTranscriptRows } from "~/stt/render-transcript";

export function useRegenerateTranscript(
  sessionId: string,
  assertCurrent?: () => void,
) {
  const owner = useVaultReadScope();
  const check = useCallback(() => {
    assertCurrent?.();
    if (
      !owner.current() ||
      owner.scope.getSnapshot().state !== "connected" ||
      !owner.scope.client.media
    )
      throw new Error("Transcription is unavailable for this vault.");
  }, [assertCurrent, owner.current, owner.scope]);
  const mutation = useMutation({
    retry: false,
    networkMode: "always",
    mutationFn: async () => {
      check();
      const media = owner.scope.client.media!;
      const expected = await owner.scope.run(() => media.source(sessionId));
      check();
      const transcripts = await owner.scope.run(() =>
        owner.scope.client.transcripts(sessionId),
      );
      check();
      const assigned =
        collectAssignedHumanIdsFromTranscriptRows(transcripts).length;
      if (assigned > 0) {
        const confirmed = await confirmRegenerateSpeakerReset(assigned);
        check();
        if (!confirmed) return;
      }
      const result = await owner.scope.run(() =>
        media.retranscribe(expected, { requestId: crypto.randomUUID() }),
      );
      check();
      return result;
    },
    onError: (error) => {
      if (owner.current())
        sonnerToast.error("Re-transcription failed", {
          id: `transcript-regenerate-failed-${sessionId}`,
          description: error instanceof Error ? error.message : String(error),
        });
    },
  });
  const run = mutation.mutateAsync;
  return useCallback(async () => {
    await run();
  }, [run]);
}
