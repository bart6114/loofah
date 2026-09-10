import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ChatgptSettings } from "./chatgpt";

const mocks = vi.hoisted(() => ({
  account: vi.fn(),
  login: vi.fn(),
  logout: vi.fn(),
  cancel: vi.fn(),
  models: vi.fn(),
  settings: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    chatgptAccount: mocks.account,
    chatgptLogin: mocks.login,
    chatgptLogout: mocks.logout,
    chatgptCancelLogin: mocks.cancel,
    chatgptModels: mocks.models,
  },
}));
vi.mock("~/settings/queries", () => ({ setSettingValues: mocks.settings }));
vi.mock("~/shared/config", () => ({
  useConfigValues: () => ({
    current_llm_provider: "chatgpt_subscription",
    current_llm_model: "saved-model",
  }),
}));

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <ChatgptSettings />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.account.mockResolvedValue({ status: "ok", data: null });
  mocks.models.mockResolvedValue({
    status: "ok",
    data: [
      {
        model: "default-model",
        displayName: "Default",
        isDefault: true,
        inputModalities: ["text", "image"],
      },
      {
        model: "saved-model",
        displayName: "Saved",
        isDefault: false,
        inputModalities: ["text", "image"],
      },
    ],
  });
  mocks.settings.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("ChatGPT settings", () => {
  it("signs in and preserves the saved available model", async () => {
    mocks.login.mockImplementation(async () => {
      mocks.account.mockResolvedValue({
        status: "ok",
        data: { email: "test@example.com", planType: "plus" },
      });
      return { status: "ok", data: null };
    });
    setup();
    const button = await screen.findByRole("button", {
      name: "Sign in with ChatGPT",
    });
    await waitFor(() => expect(button.hasAttribute("disabled")).toBe(false));
    fireEvent.click(button);
    await screen.findByText("test@example.com · plus");
    await waitFor(() =>
      expect(mocks.settings).toHaveBeenCalledWith({
        current_llm_provider: "chatgpt_subscription",
        current_llm_model: "saved-model",
      }),
    );
  });

  it("disconnects without choosing another provider", async () => {
    mocks.account.mockResolvedValue({
      status: "ok",
      data: { email: "test@example.com", planType: "plus" },
    });
    mocks.logout.mockImplementation(async () => {
      mocks.account.mockResolvedValue({ status: "ok", data: null });
      return { status: "ok", data: null };
    });
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    await screen.findByRole("button", { name: "Sign in with ChatGPT" });
    expect(mocks.settings).not.toHaveBeenCalled();
  });

  it("shows Keychain failures without requesting an API key", async () => {
    mocks.login.mockResolvedValue({
      status: "error",
      error: {
        code: "keychain",
        message: "Unlock your login Keychain.",
        retryable: false,
      },
    });
    setup();
    const button = await screen.findByRole("button", {
      name: "Sign in with ChatGPT",
    });
    await waitFor(() => expect(button.hasAttribute("disabled")).toBe(false));
    fireEvent.click(button);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Unlock your login Keychain.",
    );
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});
