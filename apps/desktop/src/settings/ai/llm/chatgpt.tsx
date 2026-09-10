import { Trans, useLingui } from "@lingui/react/macro";
import { OpenAI } from "@lobehub/icons";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Channel } from "@tauri-apps/api/core";
import { useState } from "react";

import {
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@hypr/ui/components/ui/accordion";
import { Button } from "@hypr/ui/components/ui/button";
import { cn } from "@hypr/utils";

import { useLlmSettings } from "./context";

import {
  CHATGPT_ACCOUNT_KEY,
  CHATGPT_PROVIDER,
  listChatgptModels,
  unwrapChatgpt,
  useChatgptAccount,
} from "~/ai/chatgpt-account";
import { ProviderBadge, ProviderIconSlot } from "~/settings/ai/shared";
import { setSettingValues } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import { commands } from "~/types/tauri.gen";

export function ChatgptSettings() {
  const { t } = useLingui();
  const { accordionValue } = useLlmSettings();
  const [browserOpened, setBrowserOpened] = useState(false);
  const account = useChatgptAccount(accordionValue === CHATGPT_PROVIDER);
  const queryClient = useQueryClient();
  const { current_llm_provider, current_llm_model } = useConfigValues([
    "current_llm_provider",
    "current_llm_model",
  ] as const);
  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: CHATGPT_ACCOUNT_KEY });
    await queryClient.invalidateQueries({
      queryKey: ["models", CHATGPT_PROVIDER],
    });
  };
  const login = useMutation({
    mutationFn: async () => {
      setBrowserOpened(false);
      const opened = new Channel<null>();
      opened.onmessage = () => setBrowserOpened(true);
      unwrapChatgpt(await commands.chatgptLogin(opened));
      await refresh();
      const { models } = await listChatgptModels();
      const model =
        current_llm_provider === CHATGPT_PROVIDER &&
        models.includes(current_llm_model ?? "")
          ? current_llm_model
          : models[0];
      if (model)
        await setSettingValues({
          current_llm_provider: CHATGPT_PROVIDER,
          current_llm_model: model,
        });
    },
  });
  const disconnect = useMutation({
    mutationFn: async () => {
      unwrapChatgpt(await commands.chatgptLogout());
    },
    onSuccess: refresh,
  });
  const cancel = useMutation({
    mutationFn: () => commands.chatgptCancelLogin(),
  });
  const error =
    disconnect.error ??
    (login.error &&
    !((login.error as Error & { code?: string }).code === "cancelled")
      ? login.error
      : null) ??
    account.error;

  return (
    <AccordionItem
      value={CHATGPT_PROVIDER}
      className={cn([
        "bg-muted rounded-[22px] border-2",
        account.data ? "border-border border-solid" : "border-dashed",
      ])}
    >
      <AccordionTrigger className="gap-2 px-4 hover:no-underline">
        <div className="flex items-center gap-2">
          <ProviderIconSlot>
            <OpenAI size={16} />
          </ProviderIconSlot>
          <span>
            <Trans>ChatGPT subscription</Trans>
          </span>
          <ProviderBadge badge={t`Beta`} />
        </div>
      </AccordionTrigger>
      <AccordionContent className="flex flex-col gap-3 px-4">
        <p className="text-muted-foreground text-sm">
          <Trans>
            Sign in with ChatGPT to generate summaries. Uses your account's
            Codex allowance; no API key required.
          </Trans>
        </p>
        {account.data && (
          <p className="text-sm">
            {account.data.email} · {account.data.planType}
          </p>
        )}
        {error && (
          <p role="alert" className="text-destructive text-sm">
            {error.message}
          </p>
        )}
        <div className="flex gap-2">
          {login.isPending ? (
            <>
              <span
                role="status"
                className="text-muted-foreground self-center text-sm"
              >
                {browserOpened ? (
                  <Trans>Finish signing in in your browser…</Trans>
                ) : (
                  <Trans>Opening your browser…</Trans>
                )}
              </span>
              <Button
                variant="outline"
                disabled={cancel.isPending}
                onClick={() => cancel.mutate()}
              >
                <Trans>Cancel</Trans>
              </Button>
            </>
          ) : account.data ? (
            <Button
              variant="outline"
              disabled={disconnect.isPending}
              onClick={() => disconnect.mutate()}
            >
              <Trans>Disconnect</Trans>
            </Button>
          ) : (
            <Button
              variant="outline"
              disabled={account.isLoading}
              onClick={() => {
                disconnect.reset();
                login.mutate();
              }}
            >
              <Trans>Sign in with ChatGPT</Trans>
            </Button>
          )}
        </div>
      </AccordionContent>
    </AccordionItem>
  );
}
