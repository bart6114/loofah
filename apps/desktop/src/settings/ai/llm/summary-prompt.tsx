import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { BracesIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import {
  PromptEditor,
  type PromptEditorHandle,
  type PromptTokenDefinition,
} from "@hypr/editor/prompt";
import { commands as templateCommands } from "@hypr/plugin-template";
import { Badge } from "@hypr/ui/components/ui/badge";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { setSettingValue } from "~/settings/queries";
import { useConfigValue } from "~/shared/config";

export const SUMMARY_PROMPT_TOKENS: readonly PromptTokenDefinition[] = [
  { name: "current_date", label: "Current date" },
  { name: "language", label: "Language" },
];

const DRAFT_QUERY_KEY = ["settings", "summary-prompt-draft"] as const;

export function SummaryPromptSettings({
  initiallyOpen = false,
}: { initiallyOpen?: boolean } = {}) {
  const promptOverride = useConfigValue("auto_summary_prompt");
  const sourceQuery = useQuery({
    queryKey: ["template-source", "enhance-system"],
    queryFn: loadDefaultSummaryPrompt,
    staleTime: Number.POSITIVE_INFINITY,
  });

  if (sourceQuery.isLoading) {
    return (
      <div className="text-muted-foreground flex h-full items-center justify-center text-sm">
        <Trans>Loading summary prompt...</Trans>
      </div>
    );
  }

  if (sourceQuery.error || !sourceQuery.data) {
    return (
      <div className="text-destructive flex h-full items-center justify-center px-6 text-center text-sm">
        {sourceQuery.error?.message || "Summary prompt is unavailable."}
      </div>
    );
  }

  return (
    <SummaryPromptForm
      initiallyOpen={initiallyOpen}
      defaultPrompt={sourceQuery.data}
      promptOverride={promptOverride}
    />
  );
}

export function SummaryPromptForm({
  defaultPrompt,
  promptOverride,
  initiallyOpen = false,
}: {
  defaultPrompt: string;
  promptOverride: string;
  initiallyOpen?: boolean;
}) {
  const { t } = useLingui();
  const editorRef = useRef<PromptEditorHandle>(null);
  const queryClient = useQueryClient();
  const draft = queryClient.getQueryData<string>(DRAFT_QUERY_KEY);
  const [open, setOpen] = useState(initiallyOpen || draft !== undefined);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  const saveMutation = useMutation({
    mutationFn: async (source: string) => {
      const normalized = normalizePrompt(source);
      if (!normalized) {
        throw new Error(t`Summary prompt cannot be empty.`);
      }
      const stored = promptsMatch(normalized, defaultPrompt) ? "" : normalized;
      const rendered = await templateCommands.render({
        enhanceSystem: {
          language: "en",
          promptOverride: stored,
        },
      });
      if (rendered.status === "error") {
        throw new Error(rendered.error);
      }

      await setSettingValue("auto_summary_prompt", stored);
      return stored;
    },
  });

  const savedOverride = saveMutation.data ?? promptOverride;
  const isCustomized = Boolean(savedOverride.trim());
  const initialPrompt = isCustomized ? savedOverride : defaultPrompt;
  const form = useForm({
    defaultValues: { prompt: draft ?? initialPrompt },
    listeners: {
      onChange: ({ formApi }) => {
        const prompt = formApi.state.values.prompt;
        if (promptsMatch(prompt, initialPrompt)) {
          queryClient.removeQueries({ queryKey: DRAFT_QUERY_KEY });
        } else {
          queryClient.setQueryData(DRAFT_QUERY_KEY, prompt);
        }
        setConfirmDiscard(false);
      },
    },
    onSubmit: async ({ value }) => {
      const stored = await saveMutation.mutateAsync(value.prompt);
      const nextPrompt = stored || defaultPrompt;
      form.reset({ prompt: nextPrompt });
      queryClient.removeQueries({ queryKey: DRAFT_QUERY_KEY });
      setConfirmDiscard(false);
      setOpen(false);
    },
  });

  useEffect(() => {
    const preventUnsavedClose = (event: BeforeUnloadEvent) => {
      if (!promptsMatch(form.state.values.prompt, initialPrompt)) {
        event.preventDefault();
        event.returnValue = "";
      }
    };
    window.addEventListener("beforeunload", preventUnsavedClose);
    return () =>
      window.removeEventListener("beforeunload", preventUnsavedClose);
  }, [form, initialPrompt]);

  const discardChanges = () => {
    form.reset({ prompt: initialPrompt });
    queryClient.removeQueries({ queryKey: DRAFT_QUERY_KEY });
    setConfirmDiscard(false);
    setOpen(false);
  };
  const requestClose = () => {
    if (saveMutation.isPending) return;
    if (!promptsMatch(form.state.values.prompt, initialPrompt)) {
      setConfirmDiscard(true);
    } else {
      discardChanges();
    }
  };

  return (
    <section id="summary-prompt" className="rounded-2xl border p-4">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-medium">
              <Trans>Summary instructions</Trans>
            </h3>
            <Badge variant="secondary">
              {isCustomized ? (
                <Trans>Customized</Trans>
              ) : (
                <Trans>Default</Trans>
              )}
            </Badge>
          </div>
          <p className="text-muted-foreground mt-1 text-xs">
            <Trans>Choose how Loofah writes your summaries.</Trans>
          </p>
        </div>
        <Button
          variant="outline"
          onClick={() => {
            if (saveMutation.error) saveMutation.reset();
            setOpen(true);
          }}
        >
          <Trans>Customize instructions</Trans>
        </Button>
      </div>
      <Dialog
        open={open}
        onOpenChange={(nextOpen) => (nextOpen ? setOpen(true) : requestClose())}
      >
        <DialogContent className="flex max-h-[85vh] max-w-3xl flex-col overflow-hidden">
          <DialogHeader>
            <DialogTitle>
              <Trans>Summary instructions</Trans>
            </DialogTitle>
            <DialogDescription>
              <Trans>
                Saved changes apply to future summaries. Existing text changes
                only when you regenerate it.
              </Trans>
            </DialogDescription>
          </DialogHeader>
          <form
            className="flex min-h-0 flex-1 flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void form.handleSubmit().catch(() => {});
            }}
          >
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto">
              <form.Field name="prompt">
                {(field) => (
                  <div className="border-border bg-card overflow-hidden rounded-2xl border">
                    <PromptEditor
                      ref={editorRef}
                      ariaLabel={t`Summary prompt`}
                      className="min-h-[16rem] px-4 py-3 font-mono text-sm leading-5"
                      initialValue={field.state.value}
                      maxLength={16000}
                      onChange={field.handleChange}
                      onBlur={field.handleBlur}
                      tokens={SUMMARY_PROMPT_TOKENS}
                    />
                    <div className="border-border bg-muted/40 flex items-center justify-between gap-3 border-t px-3 py-2">
                      <span className="text-muted-foreground text-xs font-medium">
                        <Trans>Variables</Trans>
                      </span>
                      <div className="flex flex-wrap justify-end gap-1.5">
                        {SUMMARY_PROMPT_TOKENS.map((token) => (
                          <Button
                            key={token.name}
                            type="button"
                            variant="outline"
                            size="sm"
                            className="h-7 rounded-full px-2.5 text-xs"
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() =>
                              editorRef.current?.insertToken(token.name)
                            }
                          >
                            <BracesIcon className="size-3.5" />
                            {token.label}
                          </Button>
                        ))}
                      </div>
                    </div>
                  </div>
                )}
              </form.Field>
              <details className="rounded-2xl border px-4 py-3">
                <summary className="cursor-pointer text-sm font-medium">
                  <Trans>Context always provided</Trans>
                </summary>
                <p className="text-muted-foreground mt-2 text-xs">
                  <Trans>
                    Loofah sends meeting notes, the transcript, session details,
                    and participants separately. Editing these instructions
                    cannot remove that source material.
                  </Trans>
                </p>
              </details>
            </div>
            {saveMutation.error ? (
              <p role="alert" className="text-destructive text-sm">
                {saveMutation.error.message}
              </p>
            ) : null}
            <form.Subscribe
              selector={(state) =>
                [state.canSubmit, state.values.prompt] as const
              }
            >
              {([canSubmit, currentPrompt]) => (
                <div className="border-border space-y-3 border-t pt-4">
                  {!promptsMatch(currentPrompt, initialPrompt) && (
                    <p role="status" className="text-sm">
                      <Trans>Unsaved changes</Trans>
                    </p>
                  )}
                  {confirmDiscard ? (
                    <div className="space-y-3" role="alert">
                      <p className="text-sm">
                        <Trans>Discard your unsaved instructions?</Trans>
                      </p>
                      <DialogFooter>
                        <Button
                          type="button"
                          variant="outline"
                          onClick={() => setConfirmDiscard(false)}
                        >
                          <Trans>Keep editing</Trans>
                        </Button>
                        <Button
                          type="button"
                          variant="destructive"
                          onClick={discardChanges}
                        >
                          <Trans>Discard changes</Trans>
                        </Button>
                      </DialogFooter>
                    </div>
                  ) : (
                    <DialogFooter>
                      <Button
                        type="button"
                        variant="ghost"
                        className="sm:mr-auto"
                        disabled={
                          promptsMatch(currentPrompt, defaultPrompt) ||
                          saveMutation.isPending
                        }
                        onClick={() => {
                          form.setFieldValue("prompt", defaultPrompt);
                          editorRef.current?.setValue(defaultPrompt);
                        }}
                      >
                        <Trans>Reset to default</Trans>
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        disabled={saveMutation.isPending}
                        onClick={requestClose}
                      >
                        <Trans>Cancel</Trans>
                      </Button>
                      <Button
                        type="submit"
                        disabled={
                          !canSubmit ||
                          promptsMatch(currentPrompt, initialPrompt) ||
                          saveMutation.isPending
                        }
                      >
                        {saveMutation.isPending ? (
                          <Trans>Saving...</Trans>
                        ) : (
                          <Trans>Save</Trans>
                        )}
                      </Button>
                    </DialogFooter>
                  )}
                </div>
              )}
            </form.Subscribe>
          </form>
        </DialogContent>
      </Dialog>
    </section>
  );
}

async function loadDefaultSummaryPrompt(): Promise<string> {
  const result = await templateCommands.getTemplateSource("enhanceSystem");
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

function normalizePrompt(value: string): string {
  return value.replace(/\r\n/g, "\n").trim();
}

function promptsMatch(a: string, b: string): boolean {
  return normalizePrompt(a) === normalizePrompt(b);
}
