import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  secret: vi.fn(async () => ({ status: "ok", data: "local-secret" })),
  account: vi.fn(async () => ({ status: "ok", data: null })),
  models: vi.fn(async () => ({ status: "ok", data: [] })),
  provider: "openai",
}));
vi.mock("@hypr/plugin-store2", () => ({
  commands: { getSecret: mocks.secret },
}));
vi.mock("@hypr/plugin-settings", () => ({
  commands: {
    getConfig: vi.fn(async () => ({
      status: "ok",
      data: {
        current_llm_provider: mocks.provider,
        current_llm_model: "gpt-4o",
        ai_providers: {
          "llm:openai": { type: "llm", base_url: "https://api.openai.com/v1" },
        },
      },
    })),
  },
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: { chatgptAccount: mocks.account, chatgptModels: mocks.models },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import { useLanguageModel, useLLMConnection } from "./useLLMConnection";

import { resetConfigStoreForTests } from "~/shared/config/store";

const caches: QueryClient[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  mocks.provider = "openai";
  resetConfigStoreForTests();
});
afterEach(() => {
  cleanup();
  for (const cache of caches.splice(0)) cache.clear();
});
function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  caches.push(client);
  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}
it("does not load provider keys or expose a model when disabled, including cached local configuration", async () => {
  const active = renderHook(
    ({ enabled }) => ({
      connection: useLLMConnection(enabled),
      model: useLanguageModel(enabled),
    }),
    { initialProps: { enabled: false }, wrapper: wrapper() },
  );
  await waitFor(() =>
    expect(active.result.current.connection.status).toEqual({
      status: "pending",
      reason: "missing_provider",
    }),
  );
  expect(mocks.secret).not.toHaveBeenCalled();
  expect(mocks.account).not.toHaveBeenCalled();
  expect(mocks.models).not.toHaveBeenCalled();
  expect(active.result.current.model).toBeNull();
  active.rerender({ enabled: true });
  await waitFor(() =>
    expect(active.result.current.connection.conn?.apiKey).toBe("local-secret"),
  );
  expect(mocks.secret).toHaveBeenCalledOnce();
  active.rerender({ enabled: false });
  expect(active.result.current.connection.conn).toBeNull();
  expect(active.result.current.model).toBeNull();
});
it("does not query a cached ChatGPT account or list models for disabled controls", async () => {
  mocks.provider = "chatgpt_subscription";
  const active = renderHook(() => useLLMConnection(false), {
    wrapper: wrapper(),
  });
  await waitFor(() => expect(active.result.current.conn).toBeNull());
  expect(mocks.account).not.toHaveBeenCalled();
  expect(mocks.models).not.toHaveBeenCalled();
  expect(mocks.secret).not.toHaveBeenCalled();
});
