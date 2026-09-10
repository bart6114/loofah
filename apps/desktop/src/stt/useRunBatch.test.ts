import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  canRunBatchTranscription,
  getBatchProvider,
  getSessionSpeakerCount,
} from "./useRunBatch";
import { useRunBatch } from "./useRunBatch";

const {
  startTranscriptionMock,
  useListenerMock,
  useSessionMock,
  useSTTConnectionMock,
  useConfigValueMock,
  isSupportedLanguagesBatchMock,
  sonnerToastWarningMock,
  deleteProcessedAudioForRetentionMock,
  createTranscriptMock,
  appendTranscriptWordsAndHintsMock,
  queueTagSuggestionsMock,
  idMock,
} = vi.hoisted(() => ({
  startTranscriptionMock: vi.fn(),
  useListenerMock: vi.fn(),
  useSessionMock: vi.fn(),
  useSTTConnectionMock: vi.fn(),
  useConfigValueMock: vi.fn(),
  isSupportedLanguagesBatchMock: vi.fn(),
  sonnerToastWarningMock: vi.fn(),
  deleteProcessedAudioForRetentionMock: vi.fn(),
  createTranscriptMock: vi.fn(),
  appendTranscriptWordsAndHintsMock: vi.fn(),
  queueTagSuggestionsMock: vi.fn(),
  idMock: vi.fn(),
}));

vi.mock("./contexts", () => ({
  useListener: useListenerMock,
}));

vi.mock("./useKeywords", () => ({
  getSessionKeywords: vi.fn(async () => []),
  useKeywords: vi.fn(() => []),
}));

vi.mock("./useSTTConnection", () => ({
  useSTTConnection: useSTTConnectionMock,
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    warning: sonnerToastWarningMock,
  },
}));

vi.mock("~/services/audio-retention", () => ({
  deleteProcessedAudioForRetention: deleteProcessedAudioForRetentionMock,
  normalizeAudioRetention: (value: unknown) =>
    typeof value === "string" ? value : "forever",
}));

vi.mock("~/session/queries", () => ({
  useSession: useSessionMock,
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: useConfigValueMock,
}));

vi.mock("~/shared/utils", () => ({
  id: idMock,
}));

vi.mock("~/stt/capabilities", () => {
  const baseLanguageCode = (language: string) =>
    language.split(/[-_]/)[0]?.toLowerCase() ?? "";

  return {
    getTranscriptionLanguages: (
      mainLanguage: string | null | undefined,
      spokenLanguages: readonly string[] | null | undefined,
    ) => {
      const seen = new Set<string>();
      const languages: string[] = [];

      for (const language of [mainLanguage, ...(spokenLanguages ?? [])]) {
        if (!language) {
          continue;
        }

        const baseCode = baseLanguageCode(language);
        if (!baseCode || seen.has(baseCode)) {
          continue;
        }

        seen.add(baseCode);
        languages.push(language);
      }

      return languages;
    },
    isSupportedLanguagesBatch: isSupportedLanguagesBatchMock,
  };
});

vi.mock("~/stt/queries", () => ({
  appendTranscriptWordsAndHints: appendTranscriptWordsAndHintsMock,
  createTranscript: createTranscriptMock,
}));

vi.mock("~/tags/suggestions", () => ({
  queueTagSuggestions: queueTagSuggestionsMock,
}));

test("routes Whisper Large V3 through progressive local batch transcription", () => {
  expect(getBatchProvider("fmtr", "whisper-large-v3")).toBe("whispercpp");
});

describe("getBatchProvider", () => {
  test("maps local soniqo models to the soniqo batch provider", () => {
    expect(getBatchProvider("fmtr", "soniqo-parakeet-batch")).toBe("soniqo");
  });

  test("maps local Argmax models to the am batch provider", () => {
    expect(getBatchProvider("fmtr", "am-parakeet-v3")).toBe("am");
  });

  test.each([
    "whisper-large-v3",
    "QuantizedLargeTurbo",
    "QuantizedSmall",
    "QuantizedSmallEn",
    "QuantizedBase",
    "QuantizedBaseEn",
    "QuantizedTiny",
    "QuantizedTinyEn",
  ])("routes %s through the Whisper batch engine", (model) => {
    expect(getBatchProvider("fmtr", model)).toBe("whispercpp");
  });

  test("returns null for any non-on-device provider — STT is on-device only", () => {
    expect(getBatchProvider("deepgram", "nova-3-general")).toBeNull();
    expect(getBatchProvider("custom", "whisper-large-v3")).toBeNull();
  });
});

describe("canRunBatchTranscription", () => {
  test("requires an STT connection", () => {
    expect(canRunBatchTranscription(null)).toBe(false);
  });

  test("allows on-device connections that map to a batch provider", () => {
    expect(
      canRunBatchTranscription({
        provider: "fmtr",
        model: "soniqo-parakeet-streaming",
      }),
    ).toBe(true);
    expect(
      canRunBatchTranscription(
        { provider: "fmtr", model: "soniqo-parakeet-streaming" },
        "am-parakeet-v3",
      ),
    ).toBe(true);
  });

  test("rejects connections without a batch provider mapping", () => {
    expect(
      canRunBatchTranscription({
        provider: "deepgram",
        model: "nova-3-general",
      }),
    ).toBe(false);
    expect(
      canRunBatchTranscription(
        { provider: "custom", model: "whisper-large-v3" },
        "whisper-large-v3",
      ),
    ).toBe(false);
  });
});

describe("useRunBatch", () => {
  beforeEach(() => {
    vi.clearAllMocks();

    let nextId = 0;
    idMock.mockImplementation(() => `generated-${++nextId}`);
    createTranscriptMock.mockResolvedValue(undefined);
    appendTranscriptWordsAndHintsMock.mockResolvedValue(undefined);
    queueTagSuggestionsMock.mockResolvedValue(undefined);
    deleteProcessedAudioForRetentionMock.mockResolvedValue(undefined);
    isSupportedLanguagesBatchMock.mockResolvedValue(true);
    useListenerMock.mockImplementation((selector) =>
      selector({ startTranscription: startTranscriptionMock }),
    );
    useSessionMock.mockReturnValue({
      id: "session-1",
      user_id: "user-1",
      raw_md: "Existing memo",
    });
    // STT is on-device only: useSTTConnection() only ever returns a
    // "fmtr" + local-model connection (or null) — never a hosted one.
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-batch",
        baseUrl: "soniqo://local",
        apiKey: "",
      },
    });
    useConfigValueMock.mockImplementation((key) =>
      key === "ai_language" ? "en" : [],
    );
  });

  test("waits for streamed persists before retention", async () => {
    let resolveAppend: (() => void) | undefined;
    appendTranscriptWordsAndHintsMock.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveAppend = resolve;
        }),
    );
    startTranscriptionMock.mockImplementation(async (_params, options) => {
      options.handlePersist(
        [{ text: "hello", start_ms: 0, end_ms: 100, channel: 0 }],
        [],
      );
      options.handlePersist(
        [{ text: "world", start_ms: 100, end_ms: 200, channel: 0 }],
        [],
      );
    });

    const { result } = renderHook(() => useRunBatch("session-1"));
    const run = result.current("/tmp/session.wav");

    await waitFor(() => {
      expect(appendTranscriptWordsAndHintsMock).toHaveBeenCalledTimes(1);
    });
    expect(deleteProcessedAudioForRetentionMock).not.toHaveBeenCalled();

    resolveAppend?.();
    await act(async () => await run);

    expect(createTranscriptMock).toHaveBeenCalledTimes(1);
    expect(queueTagSuggestionsMock).toHaveBeenCalledWith("session-1");
    expect(deleteProcessedAudioForRetentionMock).toHaveBeenCalledTimes(1);
    expect(
      appendTranscriptWordsAndHintsMock.mock.invocationCallOrder[0],
    ).toBeLessThan(
      deleteProcessedAudioForRetentionMock.mock.invocationCallOrder[0],
    );
  });

  test("does not save for custom batch persist handlers", async () => {
    const handlePersist = vi.fn();
    startTranscriptionMock.mockImplementation(async (_params, options) => {
      options.handlePersist(
        [{ text: "custom", start_ms: 0, end_ms: 100, channel: 0 }],
        [],
      );
    });

    const { result } = renderHook(() => useRunBatch("session-1"));

    await act(async () => {
      await result.current("/tmp/session.wav", { handlePersist });
    });

    expect(handlePersist).toHaveBeenCalledTimes(1);
    expect(createTranscriptMock).not.toHaveBeenCalled();
    expect(appendTranscriptWordsAndHintsMock).not.toHaveBeenCalled();
  });

  test("flushes default batch persists before rethrowing transcription errors", async () => {
    startTranscriptionMock.mockImplementation(async (_params, options) => {
      options.handlePersist(
        [{ text: "partial", start_ms: 0, end_ms: 100, channel: 0 }],
        [],
      );
      throw new Error("provider failed");
    });

    const { result } = renderHook(() => useRunBatch("session-1"));

    await expect(
      act(async () => {
        await result.current("/tmp/session.wav");
      }),
    ).rejects.toThrow("provider failed");

    expect(createTranscriptMock).toHaveBeenCalledTimes(1);
    expect(deleteProcessedAudioForRetentionMock).not.toHaveBeenCalled();
  });

  test("passes selected transcription languages to batch transcription", async () => {
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-batch",
        baseUrl: "soniqo://local",
        apiKey: "",
      },
    });
    useConfigValueMock.mockImplementation((key) =>
      key === "ai_language" ? "de" : ["en"],
    );
    startTranscriptionMock.mockResolvedValue(undefined);

    const { result } = renderHook(() => useRunBatch("session-1"));

    await act(async () => {
      await result.current("/tmp/session.wav");
    });

    expect(startTranscriptionMock).toHaveBeenCalledWith(
      expect.objectContaining({
        provider: "soniqo",
        model: "soniqo-parakeet-batch",
        languages: ["de", "en"],
      }),
      expect.any(Object),
    );
  });

  test("falls back to local Soniqo batch when the selected on-device model is not batch-capable", async () => {
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-streaming",
        baseUrl: "soniqo://local",
        apiKey: "",
      },
    });
    isSupportedLanguagesBatchMock.mockResolvedValue(false);
    startTranscriptionMock.mockResolvedValue(undefined);

    const { result } = renderHook(() => useRunBatch("session-1"));

    await act(async () => {
      await result.current("/tmp/session.wav");
    });

    expect(startTranscriptionMock).toHaveBeenCalledWith(
      expect.objectContaining({
        provider: "soniqo",
        model: "soniqo-parakeet-batch",
        base_url: "soniqo://local",
        api_key: "",
      }),
      expect.any(Object),
    );
    expect(sonnerToastWarningMock).toHaveBeenCalledWith(
      "Using a batch transcription provider",
      expect.objectContaining({
        description:
          "soniqo-parakeet-streaming is not available for batch transcription. Using Soniqo batch transcription instead.",
      }),
    );
  });

  // STT is on-device only: there is no cloud/hosted fallback left, so an
  // absent connection always resolves to the local Soniqo batch target.
  test("always falls back to the local Soniqo target when there is no STT connection", async () => {
    useSTTConnectionMock.mockReturnValue({ conn: null });
    startTranscriptionMock.mockResolvedValue(undefined);

    const { result } = renderHook(() => useRunBatch("session-1"));

    await act(async () => {
      await result.current("/tmp/session.wav");
    });

    expect(startTranscriptionMock).toHaveBeenCalledWith(
      expect.objectContaining({
        provider: "soniqo",
        model: "soniqo-parakeet-batch",
        base_url: "soniqo://local",
        api_key: "",
      }),
      expect.any(Object),
    );
  });
});

describe("getSessionSpeakerCount", () => {
  test("counts distinct session participants plus the current user", () => {
    expect(
      getSessionSpeakerCount(["human-a", "human-a", "human-b"], "self"),
    ).toBe(3);
  });

  test("returns undefined until at least two speakers are known", () => {
    expect(getSessionSpeakerCount(["human-a"], null)).toBe(undefined);
  });
});
