import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import type {
  ProcessingPolicy,
  ProcessingSettingsSnapshot,
  ProcessingSettingsSaveOutcome,
  SaveProcessingSettingsRequest,
} from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultProcessingSettings } from "./vault-processing-settings";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});

async function fixture(canEdit = true, custom = false) {
  let receive!: (event: VaultClientEvent) => void;
  let current: ProcessingSettingsSnapshot = {
    revision: "a".repeat(64),
    settings: {
      summary_model: null,
      ai_language: "en",
      summary_prompt: "",
      auto_apply_high_confidence_tags: true,
    },
  };
  const policy: ProcessingPolicy = {
    allow_custom_templates: custom,
    providers: [
      {
        id: custom ? "custom" : "ollama",
        models: custom ? [] : ["approved-one", "approved-two"],
        base_url: custom ? "" : "https://models.company.example/v1",
        allow_custom_model: custom,
        allow_custom_endpoint: custom,
        requires_key: custom,
      },
    ],
  };
  const calls = {
    processingPolicy: vi.fn(async () => policy),
    processingSettings: vi.fn(async () => structuredClone(current)),
    saveProcessingSettings: vi.fn(
      async (
        input: SaveProcessingSettingsRequest,
      ): Promise<ProcessingSettingsSaveOutcome> => {
        current = { settings: input.settings, revision: "b".repeat(64) };
        return { kind: "saved" as const, snapshot: structuredClone(current) };
      },
    ),
  };
  const client = {
    ...calls,
    scope: "settings-test",
    canEdit,
    features: ["processing_settings"],
    networkMode: "always",
    subscribe: async (callback: (event: VaultClientEvent) => void) => {
      receive = callback;
      callback({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, staleTime: Infinity, networkMode: "always" },
    mutations: { retry: false, networkMode: "always" },
  });
  await scope.start();
  render(<VaultProcessingSettings scope={scope} />);
  fireEvent.click(screen.getByRole("button", { name: "Processing settings" }));
  await screen.findByLabelText("Summary provider");
  return {
    scope,
    calls,
    emit: (event: VaultClientEvent) => receive(event),
    setCurrent: (value: ProcessingSettingsSnapshot) => {
      current = value;
    },
  };
}

it("offers approved models and saves the vault projection through the shared client", async () => {
  const { calls } = await fixture();
  fireEvent.change(screen.getByLabelText("Summary provider"), {
    target: { value: "ollama" },
  });
  expect(screen.queryByLabelText("Provider endpoint")).toBeNull();
  fireEvent.change(screen.getByLabelText("Summary model"), {
    target: { value: "approved-two" },
  });
  fireEvent.change(screen.getByLabelText("Summary language"), {
    target: { value: "nl" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await screen.findByText("Settings saved.");
  expect(calls.saveProcessingSettings).toHaveBeenCalledWith({
    expected_revision: "a".repeat(64),
    settings: {
      summary_model: {
        provider_id: "ollama",
        model_id: "approved-two",
        base_url: "https://models.company.example/v1",
      },
      ai_language: "nl",
      summary_prompt: "",
      auto_apply_high_confidence_tags: true,
    },
  });
});

it("keeps typed settings on conflict and loads the current snapshot only after explicit action", async () => {
  const { calls } = await fixture();
  calls.saveProcessingSettings.mockImplementationOnce(async () => ({
    kind: "conflict",
    snapshot: {
      revision: "c".repeat(64),
      settings: {
        summary_model: null,
        ai_language: "de",
        summary_prompt: "",
        auto_apply_high_confidence_tags: true,
      },
    },
  }));
  fireEvent.change(screen.getByLabelText("Summary language"), {
    target: { value: "nl" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  const reload = await screen.findByRole("button", {
    name: "Load current settings",
  });
  expect(screen.getByLabelText("Summary language")).toHaveProperty(
    "value",
    "nl",
  );
  expect(
    screen.getByRole("button", { name: "Save settings" }).matches(":disabled"),
  ).toBe(true);
  fireEvent.click(reload);
  expect(screen.getByLabelText("Summary language")).toHaveProperty(
    "value",
    "de",
  );
  fireEvent.change(screen.getByLabelText("Summary language"), {
    target: { value: "fr" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await screen.findByText("Settings saved.");
  expect(calls.saveProcessingSettings.mock.calls[1][0].expected_revision).toBe(
    "c".repeat(64),
  );
});

it("preserves the form during disconnect and removes private settings on access loss", async () => {
  const { scope, calls, emit } = await fixture();
  fireEvent.change(screen.getByLabelText("Summary language"), {
    target: { value: "nl" },
  });
  act(() => emit({ kind: "connection", state: "reconnecting", error: null }));
  expect(screen.getByLabelText("Summary language")).toHaveProperty(
    "value",
    "nl",
  );
  expect(
    screen.getByRole("button", { name: "Save settings" }).matches(":disabled"),
  ).toBe(true);
  expect(calls.saveProcessingSettings).not.toHaveBeenCalled();
  act(() => emit({ kind: "connection", state: "access_denied", error: null }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(
    scope.queryClient.getQueryData(scope.processingSettingsOptions().queryKey),
  ).toBeUndefined();
});

it("uses host policy to preserve custom local endpoints and templates", async () => {
  const { calls } = await fixture(true, true);
  fireEvent.change(screen.getByLabelText("Summary provider"), {
    target: { value: "custom" },
  });
  fireEvent.change(screen.getByLabelText("Summary model"), {
    target: { value: "my-private-model" },
  });
  fireEvent.change(screen.getByLabelText("Provider endpoint"), {
    target: { value: "http://192.168.1.9:8000/v1" },
  });
  fireEvent.change(screen.getByLabelText("Custom summary template"), {
    target: { value: "Decisions in {{ language }}" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await screen.findByText("Settings saved.");
  expect(
    calls.saveProcessingSettings.mock.calls[0][0].settings.summary_model
      ?.base_url,
  ).toBe("http://192.168.1.9:8000/v1");
});

it("keeps failed saves available for review without automatic retries", async () => {
  const { calls } = await fixture();
  calls.saveProcessingSettings.mockRejectedValueOnce(
    new Error("Connection lost; save outcome unknown"),
  );
  fireEvent.change(screen.getByLabelText("Summary language"), {
    target: { value: "nl" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await screen.findByText("Connection lost; save outcome unknown");
  expect(screen.getByLabelText("Summary language")).toHaveProperty(
    "value",
    "nl",
  );
  expect(calls.saveProcessingSettings).toHaveBeenCalledTimes(1);
});

it("lets readers inspect settings without submitting changes", async () => {
  const { calls } = await fixture(false);
  expect(screen.getByLabelText("Summary provider").matches(":disabled")).toBe(
    true,
  );
  expect(
    screen.getByRole("button", { name: "Save settings" }).matches(":disabled"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  expect(calls.saveProcessingSettings).not.toHaveBeenCalled();
});

it("does not repopulate private caches when a save reply arrives after access loss", async () => {
  const { scope, calls, emit } = await fixture();
  let finish!: (outcome: ProcessingSettingsSaveOutcome) => void;
  calls.saveProcessingSettings.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  fireEvent.change(screen.getByLabelText("Summary language"), {
    target: { value: "nl" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
  await waitFor(() =>
    expect(calls.saveProcessingSettings).toHaveBeenCalledTimes(1),
  );
  act(() => emit({ kind: "connection", state: "access_denied", error: null }));
  await act(async () =>
    finish({
      kind: "saved",
      snapshot: {
        revision: "b".repeat(64),
        settings: {
          summary_model: null,
          ai_language: "nl",
          summary_prompt: "",
          auto_apply_high_confidence_tags: true,
        },
      },
    }),
  );
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(
    scope.queryClient.getQueryData(scope.processingSettingsOptions().queryKey),
  ).toBeUndefined();
});

it("does not discard typed changes when a background refresh fails", async () => {
  const { scope, calls } = await fixture();
  fireEvent.change(screen.getByLabelText("Summary language"), {
    target: { value: "nl" },
  });
  calls.processingSettings.mockRejectedValueOnce(
    new Error("Refresh disconnected"),
  );
  await act(async () => {
    await scope.queryClient.invalidateQueries({
      queryKey: scope.processingSettingsOptions().queryKey,
    });
  });
  await screen.findByText("Refresh disconnected");
  expect(screen.getByLabelText("Summary language")).toHaveProperty(
    "value",
    "nl",
  );
});
