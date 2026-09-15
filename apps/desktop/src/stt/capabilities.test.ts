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

test("an explicit after-recording choice overrides live capability without dropping languages", async () => {
  expect(
    await getLiveTranscriptionConfig({
      provider: "fmtr",
      model: "soniqo-parakeet-streaming",
      languages: ["en"],
      timing: "batch",
    }),
  ).toEqual({ languages: ["en"], transcriptionMode: "batch" });
});

test("an explicit live choice still respects model and meeting language restrictions", async () => {
  for (const [model, languages, mode] of [
    ["soniqo-parakeet-streaming", ["en"], "live"],
    ["soniqo-parakeet-streaming", ["en", "nl"], "batch"],
    ["QuantizedSmallEn", ["en"], "live"],
    ["QuantizedSmallEn", ["en", "nl"], "batch"],
  ] as const) {
    expect(
      await getLiveTranscriptionConfig({
        provider: "fmtr",
        model,
        languages,
        timing: "live",
      }),
    ).toEqual({ languages: [...languages], transcriptionMode: mode });
  }
});

test("Whisper Large V3 transcribes live and preserves Dutch and English", () => {
  expect(isSupportedLocalSttModel("whisper-large-v3")).toBe(true);
  expect(
    getOnDeviceTranscriptionConfig("whisper-large-v3", ["nl", "en"]),
  ).toEqual({
    languages: ["nl", "en"],
    transcriptionMode: "live",
  });
});

test("Whisper preserves an explicit after-recording preference", async () => {
  expect(
    await getLiveTranscriptionConfig({
      provider: "fmtr",
      model: "whisper-large-v3",
      languages: ["nl", "en"],
      timing: "batch",
    }),
  ).toEqual({ languages: ["nl", "en"], transcriptionMode: "batch" });
});

test("Whisper respects backend language coverage before starting live capture", async () => {
  isSupportedLanguagesLiveMock.mockResolvedValue({ status: "ok", data: false });
  expect(
    await getLiveTranscriptionConfig({
      provider: "fmtr",
      model: "QuantizedSmall",
      languages: ["unsupported"],
      timing: "live",
    }),
  ).toEqual({ languages: ["unsupported"], transcriptionMode: "batch" });
});

describe("getOnDeviceTranscriptionMode", () => {
  test("uses live mode for realtime local models", () => {
    expect(getOnDeviceTranscriptionMode("soniqo-parakeet-streaming")).toBe(
      "live",
    );
  });

  test("uses batch mode for non-realtime local models", () => {
    expect(getOnDeviceTranscriptionMode("soniqo-omnilingual")).toBe("batch");
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
    expect(isSupportedLocalSttModel("soniqo-parakeet-batch")).toBe(true);
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
    expect(isConfiguredSttModel("fmtr", "soniqo-omnilingual")).toBe(true);
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
    await isSupportedLanguagesLive("fmtr", "soniqo-parakeet-batch", ["en"]);

    expect(isSupportedLanguagesLiveMock.mock.calls[0]).toEqual([
      "fmtr",
      "soniqo-parakeet-batch",
      ["en"],
    ]);

    await isSupportedLanguagesBatch("fmtr", "soniqo-parakeet-batch", ["en"]);

    expect(isSupportedLanguagesBatchMock.mock.calls[0]).toEqual([
      "fmtr",
      "soniqo-parakeet-batch",
      ["en"],
    ]);
  });
});

describe("getTranscriptionLanguages", () => {
  test.each([undefined, null])(
    "defaults to English when meeting languages are %s",
    (languages) => {
      expect(getTranscriptionLanguages(languages)).toEqual(["en"]);
    },
  );

  test("preserves explicit meeting languages", () => {
    expect(getTranscriptionLanguages(["nl", "fr"])).toEqual(["nl", "fr"]);
  });

  test("preserves an explicit empty selection", () => {
    expect(getTranscriptionLanguages([])).toEqual([]);
  });

  test("deduplicates regional variants by base language", () => {
    expect(getTranscriptionLanguages(["en-US", "en", "ko"])).toEqual([
      "en-US",
      "ko",
    ]);
  });
});

test.each([
  "am-parakeet-v2",
  "am-parakeet-v3",
  "am-whisper-large-v3",
  "soniqo-qwen3-small",
  "soniqo-qwen3-large",
  "aufklarer/Qwen3-ASR-0.6B-MLX-4bit",
  "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
  "HyprLLM",
  "soniqo-unknown",
  "whisper-unknown",
  "QuantizedUnknown",
])("rejects unsupported model %s", (model) => {
  expect(isSupportedLocalSttModel(model)).toBe(false);
  expect(isConfiguredSttModel("fmtr", model)).toBe(false);
});

test.each(["am", "argmax"])("rejects retired provider %s", async (provider) => {
  expect(isConfiguredSttModel(provider, "am-parakeet-v3")).toBe(false);
  expect(
    await isSupportedLanguagesLive(provider, "am-parakeet-v3", ["en"]),
  ).toBe(false);
  expect(
    await isSupportedLanguagesBatch(provider, "am-parakeet-v3", ["en"]),
  ).toBe(false);
});

test.each([undefined, "live", "batch"])(
  "missing or unconfigured local providers record only with timing %s",
  async (timing) => {
    for (const [provider, model] of [
      [undefined, undefined],
      [null, null],
      ["", ""],
      ["fmtr", undefined],
      ["fmtr", "unsupported-local-model"],
    ]) {
      expect(
        await getLiveTranscriptionConfig({
          provider,
          model,
          languages: ["en", "nl"],
          timing,
        }),
      ).toEqual({ languages: ["en", "nl"], transcriptionMode: "batch" });
    }
    expect(isSupportedLanguagesLiveMock).not.toHaveBeenCalled();
  },
);
