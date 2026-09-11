import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getConfig: vi.fn(),
  setConfigValues: vi.fn(),
  getPreferredLanguages: vi.fn(),
}));

vi.mock("@hypr/plugin-settings", () => ({
  commands: {
    getConfig: mocks.getConfig,
    setConfigValues: mocks.setConfigValues,
  },
}));

vi.mock("@hypr/plugin-detect", () => ({
  commands: {
    getPreferredLanguages: mocks.getPreferredLanguages,
  },
}));

import {
  getStoredSettingValues,
  initializeApplicationSettings,
  setSettingValues,
  updateSettingValue,
} from "./queries";

import { resetConfigStoreForTests } from "~/shared/config/store";

function appConfig(overrides: Record<string, unknown> = {}) {
  return {
    autostart: false,
    auto_stop_meetings: true,
    floating_bar_enabled: true,
    floating_bar_opacity: 0.78,
    live_caption_opacity: 0.3,
    live_caption_width: 440,
    live_caption_line_count: 1,
    live_caption_position: "topCenter",
    live_caption_minimized: true,
    show_app_in_dock: true,
    show_tray_icon: true,
    theme: "system",
    notification_detect: true,
    respect_dnd: false,
    cloud_sync_enabled: true,
    ai_language: "en",
    spoken_languages: [],
    personalization_dictionary_terms: [],
    custom_summary_instructions: "",
    custom_summary_instructions_token_aware: false,
    auto_summary_prompt: "",
    ignored_platforms: [],
    included_platforms: [],
    mic_active_threshold: 15,
    ai_providers: {},
    ...overrides,
  };
}

describe("config-backed settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetConfigStoreForTests();
    mocks.getConfig.mockResolvedValue({ status: "ok", data: appConfig() });
    mocks.setConfigValues.mockResolvedValue({ status: "ok", data: null });
    mocks.getPreferredLanguages.mockResolvedValue({
      status: "error",
      error: "unavailable",
    });
  });

  it("treats config defaults as unset values", async () => {
    const stored = await getStoredSettingValues();

    expect(stored.hasValues.size).toBe(0);
    expect(stored.values.theme).toBeUndefined();
  });

  it("ignores retired audio settings from existing vaults", async () => {
    mocks.getConfig.mockResolvedValue({
      status: "ok",
      data: appConfig({ audio_retention: "none", save_recordings: false }),
    });

    const stored = await getStoredSettingValues();

    expect(stored.values).not.toHaveProperty("audio_retention");
    expect(stored.values).not.toHaveProperty("save_recordings");
    expect([...stored.hasValues]).not.toContain("audio_retention");
    expect([...stored.hasValues]).not.toContain("save_recordings");
  });

  it("exposes explicit config values, stringifying array keys", async () => {
    mocks.getConfig.mockResolvedValue({
      status: "ok",
      data: appConfig({
        theme: "dark",
        spoken_languages: ["en", "ko"],
        current_stt_provider: "fmtr",
      }),
    });

    const stored = await getStoredSettingValues();

    expect(stored.values.theme).toBe("dark");
    expect(stored.values.spoken_languages).toBe('["en","ko"]');
    expect(stored.values.current_stt_provider).toBe("fmtr");
    expect(stored.hasValues.has("theme")).toBe(true);
  });

  it("writes schema-typed JSON values in a single config call", async () => {
    await setSettingValues({
      theme: "dark",
      notification_detect: false,
      spoken_languages: '["en","ko"]',
    });

    expect(mocks.setConfigValues).toHaveBeenCalledTimes(1);
    expect(mocks.setConfigValues).toHaveBeenCalledWith({
      theme: "dark",
      notification_detect: false,
      spoken_languages: ["en", "ko"],
    });
  });

  it("surfaces backend type rejections to the caller", async () => {
    mocks.setConfigValues.mockResolvedValue({
      status: "error",
      error: "invalid type",
    });

    await expect(setSettingValues({ theme: "dark" })).rejects.toThrow(
      "invalid type",
    );
  });

  it("persists OS language defaults only when no explicit values exist", async () => {
    mocks.getPreferredLanguages.mockResolvedValue({
      status: "ok",
      data: ["ko", "en"],
    });

    await initializeApplicationSettings();

    expect(mocks.setConfigValues).toHaveBeenCalledTimes(1);
    expect(mocks.setConfigValues).toHaveBeenCalledWith({
      ai_language: "ko",
      spoken_languages: ["ko", "en"],
    });
  });

  it("keeps explicitly configured languages over OS preferences", async () => {
    mocks.getConfig.mockResolvedValue({
      status: "ok",
      data: appConfig({ ai_language: "fr", spoken_languages: ["fr"] }),
    });
    mocks.getPreferredLanguages.mockResolvedValue({
      status: "ok",
      data: ["ko", "en"],
    });

    await initializeApplicationSettings();

    expect(mocks.setConfigValues).not.toHaveBeenCalled();
  });

  it("does not invent a model for a stale/legacy transcription provider — STT is on-device only", async () => {
    // "deepgram" is a stale value from before STT went on-device only; there
    // is no more built-in per-provider default to repair it with, so
    // initialization should leave it untouched (no write happens at all).
    mocks.getConfig.mockResolvedValue({
      status: "ok",
      data: appConfig({ current_stt_provider: "deepgram" }),
    });

    await initializeApplicationSettings();

    expect(mocks.setConfigValues).not.toHaveBeenCalled();
  });

  it("updates against the latest config value", async () => {
    mocks.getConfig.mockResolvedValue({
      status: "ok",
      data: appConfig({ personalization_dictionary_terms: ["Vertex"] }),
    });

    const next = await updateSettingValue(
      "personalization_dictionary_terms",
      (current) => JSON.stringify([...JSON.parse(current ?? "[]"), "Erebor"]),
    );

    expect(next).toBe(JSON.stringify(["Vertex", "Erebor"]));
    expect(mocks.setConfigValues).toHaveBeenCalledWith({
      personalization_dictionary_terms: ["Vertex", "Erebor"],
    });
  });

  it("falls back to the schema default when updating an unset value", async () => {
    const next = await updateSettingValue(
      "personalization_dictionary_terms",
      (current) => JSON.stringify([...JSON.parse(current ?? "[]"), "Erebor"]),
    );

    expect(next).toBe(JSON.stringify(["Erebor"]));
  });
});
