import { describe, expect, it } from "vitest";

import { formatDebugInformation, releaseChannel } from "./debug-information";

const defaults: Parameters<typeof formatDebugInformation>[0] = {
  device: {
    appVersion: "0.41.4",
    buildHash: "abc123",
    identifier: "io.loofah.dev",
    platform: "macos",
    arch: "aarch64",
    osVersion: "macOS 15.6",
    locale: "en",
  },
  settings: { values: {}, hasValues: new Set() },
  languages: ["en"],
  timing: "live",
  availability: "Not applicable",
  serverStatus: "Not configured",
  batchProvider: null,
  activity: {
    status: "inactive",
    starting: false,
    liveActive: null,
    requestedLive: null,
    stalled: false,
    degraded: null,
    batchCount: 0,
  },
};

function configured(overrides: Partial<typeof defaults> = {}): typeof defaults {
  return {
    ...defaults,
    settings: {
      values: {
        current_stt_provider: "fmtr",
        current_stt_model: "soniqo-parakeet-streaming",
        transcription_timing: "live",
        current_llm_provider: "openai",
        current_llm_model: "gpt-4.1",
      },
      hasValues: new Set([
        "current_stt_provider",
        "current_stt_model",
        "transcription_timing",
        "current_llm_provider",
        "current_llm_model",
      ]),
    },
    availability: "Downloaded",
    serverStatus: "ready",
    serverModel: "soniqo-parakeet-streaming",
    liveMode: "live",
    batchProvider: "soniqo",
    batchSupported: true,
    ...overrides,
  };
}

describe("debug information", () => {
  it("names the Whisper model and engine without losing the saved model ID", () => {
    const input = configured({
      batchProvider: "whispercpp",
      modelDisplayName: "Whisper Small (Multilingual)",
    });
    input.settings.values.current_stt_model = "QuantizedSmall";
    const text = formatDebugInformation(input);
    expect(text).toContain(
      "Transcription model: Whisper Small (Multilingual) [QuantizedSmall]",
    );
    expect(text).toContain("Transcription engine: Whisper (whisper.cpp)");
  });

  it("distinguishes unset saved settings from the applied defaults and idle runtime", () => {
    const text = formatDebugInformation(defaults);
    expect(text).toContain("Transcription timing: Not set");
    expect(text).toContain("Transcription timing: live");
    expect(text).toContain("Transcription languages: en");
    expect(text).toContain("No live transcription active");
    expect(text).toContain("No batch transcription active");
    expect(text).toContain("Intelligence: Disabled / setup incomplete");
  });

  it("shows selected and resolved engine models even while idle", () => {
    const text = formatDebugInformation(configured());
    expect(text).toContain("Transcription provider: fmtr");
    expect(text).toContain("Live target: fmtr / soniqo-parakeet-streaming");
    expect(text).toContain("Batch target: soniqo / soniqo-parakeet-streaming");
    expect(text).toContain("Intelligence: openai / gpt-4.1");
    expect(text).toContain("No live transcription active");
  });

  it("explains language fallback and does not substitute a different model", () => {
    const text = formatDebugInformation(
      configured({
        languages: ["fr"],
        liveMode: "batch",
        batchSupported: false,
      }),
    );
    expect(text).toContain("Live target: None");
    expect(text).toContain("Batch target: None");
    expect(text).toContain("Batch language support: Unsupported");
    expect(text).toContain(
      "Selected model/languages do not support live transcription",
    );
  });

  it("does not report an unavailable model as a usable target", () => {
    const text = formatDebugInformation(
      configured({
        availability: "Not downloaded",
        serverStatus: "unreachable",
      }),
    );
    expect(text).toContain("Live target: None");
    expect(text).toContain("Batch target: None");
    expect(text).toContain("Local model not downloaded; server unreachable");
  });

  it("reports active fallback without leaking raw service errors or unrelated settings", () => {
    const input = configured();
    input.settings.values.custom_summary_instructions = "private notes";
    Object.assign(input.settings.values, {
      api_key: "sk-secret",
      token: "secret-token",
      transcript: "private transcript",
    });
    input.activity = {
      ...defaults.activity,
      status: "active",
      liveActive: false,
      requestedLive: true,
      degraded: {
        type: "stream_error",
        message:
          "https://user:sk-secret@localhost/?token=secret-token private transcript",
      },
      batchCount: 2,
    };
    const text = formatDebugInformation(input);
    expect(text).toContain("Recording only; no live transcription active");
    expect(text).toContain("Live fallback: Transcription stream failed");
    expect(text).toContain("Batch: 2 active transcription(s)");
    for (const secret of [
      "sk-secret",
      "secret-token",
      "private notes",
      "private transcript",
      "https://",
    ])
      expect(text).not.toContain(secret);
  });

  it("identifies each release channel", () => {
    expect(releaseChannel("io.loofah.dev")).toBe("dev");
    expect(releaseChannel("io.loofah.staging")).toBe("staging");
    expect(releaseChannel("io.loofah.app")).toBe("stable");
    expect(releaseChannel("")).toBe("unknown");
  });
});
