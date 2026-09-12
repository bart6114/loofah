import { beforeEach, describe, expect, test, vi } from "vitest";

const { isSupportedLanguagesBatchMock, isSupportedLanguagesLiveMock } =
  vi.hoisted(() => ({
    isSupportedLanguagesBatchMock: vi.fn(),
    isSupportedLanguagesLiveMock: vi.fn(),
  }));

vi.mock("@hypr/plugin-transcription", () => ({
  commands: {
    isSupportedLanguagesBatch: isSupportedLanguagesBatchMock,
    isSupportedLanguagesLive: isSupportedLanguagesLiveMock,
  },
}));

import {
  getLiveTranscriptionConfig,
  getOnDeviceTranscriptionConfig,
  getOnDeviceTranscriptionMode,
  getTranscriptionLanguages,
  isConfiguredSttModel,
  isSupportedLanguagesBatch,
  isSupportedLanguagesLive,
  isSupportedLocalSttModel,
} from "./capabilities";

beforeEach(() => {
  vi.clearAllMocks();
  isSupportedLanguagesLiveMock.mockResolvedValue({
    status: "ok",
    data: true,
  });
  isSupportedLanguagesBatchMock.mockResolvedValue({
    status: "ok",
    data: true,
  });
});

test("Whisper Large V3 records first and preserves Dutch and English", () => {
  expect(isSupportedLocalSttModel("whisper-large-v3")).toBe(true);
  expect(
    getOnDeviceTranscriptionConfig("whisper-large-v3", ["nl", "en"]),
  ).toEqual({
    languages: ["nl", "en"],
    transcriptionMode: "batch",
  });
});

describe("getOnDeviceTranscriptionMode", () => {
  test("uses live mode for realtime local models", () => {
    expect(getOnDeviceTranscriptionMode("soniqo-parakeet-streaming")).toBe(
      "live",
    );
  });

  test("uses batch mode for non-realtime local models", () => {
    expect(getOnDeviceTranscriptionMode("soniqo-qwen3-small")).toBe("batch");
  });

  test("demotes to batch when the realtime local model does not support a configured language", () => {
    expect(
      getOnDeviceTranscriptionMode("soniqo-parakeet-streaming", ["ko"]),
    ).toBe("batch");
  });

  test("demotes European non-English languages to batch — Parakeet-EOU streaming is English-only", () => {
    expect(
      getOnDeviceTranscriptionMode("soniqo-parakeet-streaming", ["de"]),
    ).toBe("batch");
  });
});

describe("isSupportedLocalSttModel", () => {
  test("accepts shipped local STT model families", () => {
    expect(isSupportedLocalSttModel("soniqo-parakeet-streaming")).toBe(true);
    expect(isSupportedLocalSttModel("am-parakeet-v3")).toBe(true);
    expect(isSupportedLocalSttModel("QuantizedSmallEn")).toBe(true);
  });

  test("rejects cloud, local LLM, and removed local model ids", () => {
    expect(isSupportedLocalSttModel("cloud")).toBe(false);
    expect(isSupportedLocalSttModel("Llama3p2_3bQ4")).toBe(false);
    expect(isSupportedLocalSttModel("removed-local-model")).toBe(false);
  });
});

describe("isConfiguredSttModel", () => {
  test("requires an on-device model id for the on-device provider — no cloud model exists anymore", () => {
    expect(isConfiguredSttModel("fmtr", "cloud")).toBe(false);
    expect(isConfiguredSttModel("fmtr", "soniqo-qwen3-small")).toBe(true);
    expect(isConfiguredSttModel("fmtr", "removed-local-model")).toBe(false);
  });

  test("treats any other provider string as configured (defensive default for unknown/legacy providers)", () => {
    expect(
      isConfiguredSttModel("some-legacy-provider", "whisper-large-v3"),
    ).toBe(true);
  });
});

describe("getOnDeviceTranscriptionConfig", () => {
  test("stays live for English-only configs, including regional variants", () => {
    expect(
      getOnDeviceTranscriptionConfig("soniqo-parakeet-streaming", ["en-BE"]),
    ).toEqual({
      languages: ["en-BE"],
      transcriptionMode: "live",
    });
  });

  test("stays live with no configured languages", () => {
    expect(
      getOnDeviceTranscriptionConfig("soniqo-parakeet-streaming", []),
    ).toEqual({
      languages: [],
      transcriptionMode: "live",
    });
  });

  test("demotes to batch with the full language list when a secondary language is unsupported for streaming", () => {
    expect(
      getOnDeviceTranscriptionConfig("soniqo-parakeet-streaming", ["en", "nl"]),
    ).toEqual({
      languages: ["en", "nl"],
      transcriptionMode: "batch",
    });
  });

  test("demotes to batch when the main language is unsupported for streaming", () => {
    expect(
      getOnDeviceTranscriptionConfig("soniqo-parakeet-streaming", ["de", "en"]),
    ).toEqual({
      languages: ["de", "en"],
      transcriptionMode: "batch",
    });
  });

  test("demotes languages the batch model does not cover either — batch still beats gibberish streaming", () => {
    expect(
      getOnDeviceTranscriptionConfig("soniqo-parakeet-streaming", ["ko"]),
    ).toEqual({
      languages: ["ko"],
      transcriptionMode: "batch",
    });
  });
});

describe("getLiveTranscriptionConfig", () => {
  // These use a non-on-device provider string to exercise the fallback branch
  // that runs when `provider`/`model` are not a recognized on-device pair
  // (e.g. a stale config from before STT went on-device only).
  test("keeps all languages when the selected provider supports them live", async () => {
    const config = await getLiveTranscriptionConfig({
      provider: "deepgram",
      model: "nova-3-general",
      languages: ["en", "es"],
    });

    expect(config).toEqual({
      languages: ["en", "es"],
      transcriptionMode: undefined,
    });
    expect(isSupportedLanguagesLiveMock).toHaveBeenCalledTimes(1);
  });

  test("falls back to the main language when additional languages are unsupported live", async () => {
    isSupportedLanguagesLiveMock.mockImplementation(
      (_provider, _model, languages) =>
        Promise.resolve({
          status: "ok",
          data: languages.length === 1 && languages[0] === "en",
        }),
    );

    await expect(
      getLiveTranscriptionConfig({
        provider: "deepgram",
        model: "nova-3-general",
        languages: ["en", "ko"],
      }),
    ).resolves.toEqual({
      languages: ["en"],
      transcriptionMode: undefined,
    });
  });

  test("passes the provider through untouched — STT is on-device only, no Deepgram-compatibility mapping left", async () => {
    await isSupportedLanguagesLive("fmtr", "am-parakeet-v3", ["en"]);

    expect(isSupportedLanguagesLiveMock.mock.calls[0]).toEqual([
      "fmtr",
      "am-parakeet-v3",
      ["en"],
    ]);

    await isSupportedLanguagesBatch("fmtr", "am-parakeet-v3", ["en"]);

    expect(isSupportedLanguagesBatchMock.mock.calls[0]).toEqual([
      "fmtr",
      "am-parakeet-v3",
      ["en"],
    ]);
  });
});

describe("getTranscriptionLanguages", () => {
  test("explicit meeting languages stay independent of summary language", () => {
    expect(getTranscriptionLanguages("en", ["fr"], ["nl"])).toEqual(["nl"]);
    expect(getTranscriptionLanguages("de", ["fr"], ["nl"])).toEqual(["nl"]);
  });
  test("explicit empty selection does not reinstate the summary language", () => {
    expect(getTranscriptionLanguages("en", ["fr"], [])).toEqual([]);
  });
  test("prefers the main language before additional spoken languages", () => {
    expect(getTranscriptionLanguages("en", ["ko"])).toEqual(["en", "ko"]);
  });

  test("deduplicates regional variants by base language", () => {
    expect(getTranscriptionLanguages("en-US", ["en", "ko"])).toEqual([
      "en-US",
      "ko",
    ]);
  });
});
