import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConfigureProviders } from "./configure";
import { LlmSettingsProvider } from "./context";
import { SelectProviderAndModel } from "./select";

const mocks = vi.hoisted(() => ({
  config: { current_llm_provider: "", current_llm_model: "" },
  health: { status: "success", message: undefined } as {
    status: string;
    message?: string;
  },
  save: vi.fn(),
}));
vi.mock("~/shared/config", () => ({ useConfigValues: () => mocks.config }));
vi.mock("~/settings/queries", () => ({
  setSettingValues: mocks.save,
  useSettingsReady: () => true,
}));
vi.mock("~/settings/providers", () => ({
  useAiProvidersState: () => ({
    isReady: true,
    providers: { "llm:openai": { api_key: "saved-key" } },
  }),
}));
vi.mock("./health", () => ({ useConnectionHealth: () => mocks.health }));
vi.mock("~/ai/chatgpt-account", () => ({
  CHATGPT_PROVIDER: "chatgpt_subscription",
  useChatgptAccount: () => ({ data: null }),
  listChatgptModels: vi.fn(),
}));
vi.mock("./chatgpt", () => ({
  ChatgptSettings: () => <div>ChatGPT sign-in details</div>,
}));
vi.mock("~/settings/ai/shared", () => ({
  providerRowId: (kind: string, id: string) => `${kind}:${id}`,
  ProviderIconSlot: ({ children }: { children: ReactNode }) => (
    <span>{children}</span>
  ),
  AppProviderIcon: () => null,
  StyledStreamdown: ({ children }: { children: ReactNode }) => (
    <div>{children}</div>
  ),
  NonHyprProviderCard: ({ config }: { config: { displayName: string } }) => (
    <div data-testid="provider-details">
      {config.displayName} connection details
    </div>
  ),
}));
vi.mock("~/settings/ai/shared/model-combobox", () => ({
  ModelCombobox: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (value: string) => void;
  }) => (
    <button
      type="button"
      aria-label="Model picker"
      onClick={() => onChange("chosen-model")}
    >
      {value || "Choose model"}
    </button>
  ),
}));

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <LlmSettingsProvider>
        <SelectProviderAndModel />
        <ConfigureProviders />
      </LlmSettingsProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  mocks.config = { current_llm_provider: "", current_llm_model: "" };
  mocks.health = { status: "success" };
  mocks.save.mockReset();
  mocks.save.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("summary connection settings", () => {
  it("keeps summaries off even when credentials and local providers are available", async () => {
    setup();
    expect(screen.getByText("Summaries are off")).toBeTruthy();
    expect(
      screen.getByText(/Recording and transcription still work/),
    ).toBeTruthy();
    expect(screen.queryByTestId("provider-details")).toBeNull();
    await Promise.resolve();
    expect(mocks.save).not.toHaveBeenCalled();
  });

  it("keeps a failed provider selected and puts recovery beside its status", () => {
    mocks.config = {
      current_llm_provider: "ollama",
      current_llm_model: "saved-model",
    };
    mocks.health = { status: "error", message: "Server is not running" };
    setup();
    expect(screen.getByText("Ollama", { selector: "span" })).toBeTruthy();
    expect(screen.getByText("Selected")).toBeTruthy();
    expect(screen.getByText("Connection unavailable")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Check connection" }),
    ).toBeTruthy();
    expect(screen.queryByText("Ready for summaries")).toBeNull();
    expect(screen.queryByTestId("provider-details")).toBeNull();
    expect(mocks.save).not.toHaveBeenCalled();
  });

  it("only exposes the chosen setup route and activates after an explicit model choice", async () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Set up summaries" }));
    expect(screen.queryByTestId("provider-details")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Local AI app/ }));
    expect(screen.getByText(/Requires a separate app/)).toBeTruthy();
    expect(screen.getAllByTestId("provider-details")).toHaveLength(1);
    expect(screen.getByTestId("provider-details").textContent).toContain(
      "LM Studio",
    );
    expect(screen.queryByText("ChatGPT sign-in details")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Model picker" }));
    expect(mocks.save).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", { name: "Use this connection" }),
    );
    await waitFor(() =>
      expect(mocks.save).toHaveBeenCalledWith({
        current_llm_provider: "lmstudio",
        current_llm_model: "chosen-model",
      }),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("provider-details")).toBeNull(),
    );
  });

  it("clears only the selected connection when turning summaries off", async () => {
    mocks.config = {
      current_llm_provider: "openai",
      current_llm_model: "saved-model",
    };
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Turn off summaries" }));
    await waitFor(() =>
      expect(mocks.save).toHaveBeenCalledWith(
        { current_llm_provider: "", current_llm_model: "" },
        expect.anything(),
      ),
    );
  });
});
