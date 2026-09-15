import { Trans } from "@lingui/react/macro";
import { AlertCircleIcon, RefreshCwIcon } from "lucide-react";

import { Button } from "@hypr/ui/components/ui/button";

import { useAITask } from "~/ai/contexts";
import { useLanguageModel } from "~/ai/hooks";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";

export function EnhanceError({
  sessionId,
  enhancedNoteId,
  error,
}: {
  sessionId: string;
  enhancedNoteId: string;
  error: Error | undefined;
}) {
  const model = useLanguageModel();
  const generate = useAITask((state) => state.generate);

  const handleRetry = () => {
    if (!model) return;

    const taskId = createTaskId(enhancedNoteId, "enhance");
    void generate(taskId, {
      model,
      taskType: "enhance",
      args: { sessionId, enhancedNoteId },
    });
  };

  return (
    <div
      role="alert"
      className="flex h-full min-h-[400px] flex-col items-center justify-center px-6 text-center"
    >
      <AlertCircleIcon
        aria-hidden
        className="text-muted-foreground mb-5 size-9 stroke-[1.5]"
      />
      <div className="mb-6 flex max-w-md flex-col gap-2">
        <p className="text-base font-medium">Summary generation failed</p>
        <p className="text-muted-foreground text-sm leading-relaxed">
          {error?.message || (
            <Trans>Something went wrong while generating the summary.</Trans>
          )}
        </p>
      </div>
      <Button
        onClick={handleRetry}
        disabled={!model}
        size="sm"
        className="gap-2"
        variant="default"
      >
        <RefreshCwIcon size={16} />
        <span>
          <Trans>Retry</Trans>
        </span>
      </Button>
    </div>
  );
}
