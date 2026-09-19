import { createTaskId, type TaskConfig } from ".";
import {
  appendTagLineToMarkdown,
  extractEnhanceTagNames,
} from "./summary-tags";
import {
  getPersistableGeneratedTitle,
  persistGeneratedTitle,
} from "./title-success";

import { persistGeneratedEnhancedNote } from "~/session/content-mutations";
import { loadSessionContentSnapshot } from "~/session/content-queries";
import { ensureMarkdownFirstLineTitle } from "~/session/title-content";
import { hasLiveSessionTitleDraft } from "~/store/zustand/live-title";

const onSuccess: NonNullable<TaskConfig<"enhance">["onSuccess"]> = async ({
  text,
  args,
  transformedArgs,
  model,
  startTask,
  getTaskState,
  signal,
}) => {
  const summaryText = text.trim();
  if (!summaryText) {
    return;
  }

  const tagNames = extractEnhanceTagNames(summaryText, transformedArgs);
  const textWithTags = appendTagLineToMarkdown(summaryText, tagNames);
  const initialSnapshot = await loadSessionContentSnapshot(args.sessionId);
  if (!initialSnapshot) {
    throw new Error(`Session ${args.sessionId} no longer exists`);
  }

  let trimmedTitle = initialSnapshot.title.trim();
  let generatedTitle = "";
  let shouldPersistGeneratedTitle = false;

  if (!trimmedTitle && !hasLiveSessionTitleDraft(args.sessionId)) {
    const titleTaskId = createTaskId(args.sessionId, "title");
    const titleTask = getTaskState(titleTaskId);

    if (titleTask?.status === "success" || titleTask?.status === "generating") {
      generatedTitle = getPersistableGeneratedTitle(titleTask.streamedText);
    } else {
      await startTask(titleTaskId, {
        model,
        taskType: "title",
        args: {
          sessionId: args.sessionId,
          enhancedNote: textWithTags,
          skipPersist: true,
        },
        onComplete: (title) => {
          generatedTitle = getPersistableGeneratedTitle(title);
        },
      });
    }

    if (signal.aborted) {
      return;
    }
  }

  const snapshot = await loadSessionContentSnapshot(args.sessionId);
  if (!snapshot) {
    throw new Error(`Session ${args.sessionId} no longer exists`);
  }
  const note = snapshot.enhancedNotes.find((candidate) =>
    args.templateDocumentId
      ? candidate.id === args.templateDocumentId
      : candidate.kind === "summary",
  );
  if (!note || transformedArgs.expectedMarkdown === null) {
    throw new Error(`Summary ${args.sessionId} no longer exists`);
  }

  trimmedTitle = snapshot.title.trim();
  if (
    !trimmedTitle &&
    !hasLiveSessionTitleDraft(args.sessionId) &&
    generatedTitle
  ) {
    trimmedTitle = generatedTitle;
    shouldPersistGeneratedTitle = true;
  }

  const titledText = ensureMarkdownFirstLineTitle(summaryText, trimmedTitle);
  // A reset/regenerate aborts this run; a stale run that persisted anyway
  // would overwrite the replacement's summary with old content.
  if (signal.aborted) {
    return;
  }

  const persistableText = appendTagLineToMarkdown(titledText, tagNames);
  await persistGeneratedEnhancedNote({
    sessionId: args.sessionId,
    ownerUserId: snapshot.ownerUserId,
    note: {
      id: note.id,
      currentMarkdown: transformedArgs.expectedMarkdown,
      nextMarkdown: persistableText,
    },
    tagNames,
  });

  if (shouldPersistGeneratedTitle && !signal.aborted) {
    await persistGeneratedTitle({
      text: generatedTitle,
      args: { sessionId: args.sessionId },
    });
  }
};

export const enhanceSuccess: Pick<TaskConfig<"enhance">, "onSuccess"> = {
  onSuccess,
};
