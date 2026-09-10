import { Trans } from "@lingui/react/macro";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { Button } from "@hypr/ui/components/ui/button";

import {
  CHATGPT_ACCOUNT_KEY,
  CHATGPT_PROVIDER,
  listChatgptModels,
  unwrapChatgpt,
  useChatgptAccount,
} from "~/ai/chatgpt-account";
import { setSettingValues } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import { commands } from "~/types/tauri.gen";

export function ChatgptSettings() {
  const account = useChatgptAccount();
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
      unwrapChatgpt(await commands.chatgptLogin());
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
    <div className="bg-card flex flex-col gap-3 rounded-lg border p-4">
      <div className="flex items-center justify-between gap-3">
        <h4 className="text-sm font-medium">
          <Trans>ChatGPT subscription</Trans>
        </h4>
        <span className="text-muted-foreground text-xs">
          <Trans>Beta</Trans>
        </span>
      </div>
      <p className="text-muted-foreground text-sm">
        <Trans>
          Sign in with ChatGPT to generate summaries. Uses your account's Codex
          allowance; no API key required.
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
            <span className="text-muted-foreground self-center text-sm">
              <Trans>Finish signing in in your browser…</Trans>
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
    </div>
  );
}
