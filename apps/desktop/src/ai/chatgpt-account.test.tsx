import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  CHATGPT_ACCOUNT_KEY,
  CHATGPT_PROVIDER,
  refreshChatgptConnection,
  unwrapChatgpt,
} from "./chatgpt-account";
import { useLLMConnection } from "./hooks/useLLMConnection";

const mocks = vi.hoisted(() => ({ account: vi.fn(), models: vi.fn() }));
vi.mock("~/types/tauri.gen", () => ({
  commands: { chatgptAccount: mocks.account, chatgptModels: mocks.models },
}));
vi.mock("~/settings/providers", () => ({ useAiProvider: () => undefined }));
vi.mock("~/shared/config", () => ({
  useConfigValues: () => ({
    current_llm_provider: "chatgpt_subscription",
    current_llm_model: "test-model",
  }),
}));

const account = { email: "test@example.com", planType: "plus" };

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return { client, wrapper };
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.account.mockResolvedValue({ status: "ok", data: account });
  mocks.models.mockResolvedValue({
    status: "ok",
    data: [
      {
        model: "test-model",
        displayName: "Test",
        isDefault: true,
        inputModalities: ["text"],
      },
    ],
  });
});
afterEach(cleanup);

describe("ChatGPT connection after sign-in", () => {
  it("rechecks a cached signed-out result when opening a summary", async () => {
    const { client, wrapper } = setup();
    client.setQueryData(CHATGPT_ACCOUNT_KEY, null);
    const { result } = renderHook(() => useLLMConnection(), { wrapper });
    await waitFor(() => expect(result.current.status.status).toBe("success"));
    expect(result.current.conn?.modelId).toBe("test-model");
    expect(mocks.account).toHaveBeenCalled();
  });

  it("publishes the connected account and models even with no mounted observers", async () => {
    const { client, wrapper } = setup();
    client.setQueryData(CHATGPT_ACCOUNT_KEY, null);
    client.setQueryData(["models", CHATGPT_PROVIDER], { models: [] });
    await refreshChatgptConnection(client);
    expect(client.getQueryData(CHATGPT_ACCOUNT_KEY)).toEqual(account);
    const { result } = renderHook(() => useLLMConnection(), { wrapper });
    expect(result.current.status.status).toBe("success");
    expect(result.current.conn?.modelId).toBe("test-model");
  });

  it("prevents an earlier signed-out request from overwriting the successful login", async () => {
    const { client } = setup();
    let finishOldRequest!: (value: unknown) => void;
    mocks.account.mockImplementationOnce(
      () => new Promise((resolve) => (finishOldRequest = resolve)),
    );
    const oldRequest = client
      .fetchQuery({
        queryKey: CHATGPT_ACCOUNT_KEY,
        queryFn: async () => unwrapChatgpt(await mocks.account()),
      })
      .catch(() => {});
    await refreshChatgptConnection(client);
    await act(async () => {
      finishOldRequest({ status: "ok", data: null });
      await oldRequest;
    });
    expect(client.getQueryData(CHATGPT_ACCOUNT_KEY)).toEqual(account);
  });
});
