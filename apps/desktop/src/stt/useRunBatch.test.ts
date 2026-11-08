import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  canRunBatchTranscription,
  getBatchProvider,
  getSessionSpeakerCount,
} from "./useRunBatch";
import { useRunBatch } from "./useRunBatch";

const {
  sessionTranscriptsMock,
  startTranscriptionMock,
  finishLocalWorkMock,
  useListenerMock,
  useSessionMock,
  useSTTConnectionMock,
  useConfigValueMock,
  isSupportedLanguagesBatchMock,
  sonnerToastWarningMock,
  createTranscriptMock,
  appendTranscriptWordsAndHintsMock,
  idMock,
  applicationClientMock,
} = vi.hoisted(() => ({
  sessionTranscriptsMock: vi.fn(),
  startTranscriptionMock: vi.fn(),
  finishLocalWorkMock: vi.fn(),
  useListenerMock: vi.fn(),
  useSessionMock: vi.fn(),
  useSTTConnectionMock: vi.fn(),
  useConfigValueMock: vi.fn(),
  isSupportedLanguagesBatchMock: vi.fn(),
  sonnerToastWarningMock: vi.fn(),
  createTranscriptMock: vi.fn(),
  appendTranscriptWordsAndHintsMock: vi.fn(),
  idMock: vi.fn(),
  applicationClientMock: vi.fn(),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionTranscripts: sessionTranscriptsMock,
    applicationFinishLocalSessionWork: finishLocalWorkMock,
  },
}));

vi.mock("~/shared/application/client", () => ({
  getApplicationClient: applicationClientMock,
}));

vi.mock("./contexts", () => ({
  useListener: useListenerMock,
}));

vi.mock("./useSTTConnection", () => ({
  useSTTConnection: useSTTConnectionMock,
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    warning: sonnerToastWarningMock,
  },
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

vi.mock("~/stt/capabilities", async (importOriginal) => {
  const actual = await importOriginal<typeof import("~/stt/capabilities")>();
  return {
    ...actual,
    isSupportedLanguagesBatch: isSupportedLanguagesBatchMock,
  };
});

vi.mock("~/stt/queries", () => ({
  appendTranscriptWordsAndHints: appendTranscriptWordsAndHintsMock,
  createTranscript: createTranscriptMock,
}));

test("routes Whisper Large V3 through progressive local batch transcription", () => {
  expect(getBatchProvider("fmtr", "whisper-large-v3")).toBe("whispercpp");
});

describe("getBatchProvider", () => {
  test.each([
    "soniqo-parakeet-batch",
    "onnx-parakeet-streaming",
    "onnx-parakeet-batch",
  ])("routes %s through the native Parakeet batch engine", (model) => {
    expect(getBatchProvider("fmtr", model)).toBe("soniqo");
  });

  test("rejects retired models", () => {
    expect(getBatchProvider("fmtr", "am-parakeet-v3")).toBeNull();
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
        "QuantizedTiny",
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
  test.each([true, undefined])(
    "preserves imported provenance through batch and re-transcription (%s)",
    async (imported) => {
      sessionTranscriptsMock.mockResolvedValue({
        status: "ok",
        data: [{ words: [{ metadata: { capture_source: "import" } }] }],
      });
      startTranscriptionMock.mockImplementation(async (_params, options) => {
        options.handlePersist(
          [{ text: "I will send it", start_ms: 0, end_ms: 100, channel: 0 }],
          [],
        );
      });
      const { result } = renderHook(() => useRunBatch("session-1"));
      await act(async () => {
        await result.current("/tmp/import.wav", { imported });
      });
      expect(
        JSON.parse(createTranscriptMock.mock.calls[0][0].words[0].metadata),
      ).toMatchObject({ capture_source: "import" });
    },
  );

  beforeEach(() => {
    vi.clearAllMocks();
    finishLocalWorkMock.mockResolvedValue({ status: "ok", data: null });
    applicationClientMock.mockReturnValue({
      store: { sessionTranscripts: sessionTranscriptsMock },
    });
    sessionTranscriptsMock.mockResolvedValue({ status: "ok", data: [] });

    let nextId = 0;
    idMock.mockImplementation(() => `generated-${++nextId}`);
    createTranscriptMock.mockResolvedValue(undefined);
    appendTranscriptWordsAndHintsMock.mockResolvedValue(undefined);
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
      key === "ai_language"
        ? "en"
        : key === "meeting_languages"
          ? undefined
          : [],
    );
  });

  test.each(["languages", "prior transcripts"])(
    "does not admit transcription after losing its host during %s",
    async (boundary) => {
      let allowed = true;
      let finish!: () => void;
      const blocker = new Promise<void>((done) => {
        finish = done;
      });
      if (boundary === "languages")
        isSupportedLanguagesBatchMock.mockImplementationOnce(async () => {
          await blocker;
          return true;
        });
      else
        sessionTranscriptsMock.mockImplementationOnce(async () => {
          await blocker;
          return { status: "ok", data: [] };
        });
      const { result } = renderHook(() =>
        useRunBatch("session-1", () => {
          if (!allowed) throw new Error("retired");
        }),
      );
      const pending = result.current("/tmp/audio.wav");
      const rejected = expect(pending).rejects.toThrow("retired");
      await waitFor(() =>
        expect(
          boundary === "languages"
            ? isSupportedLanguagesBatchMock
            : sessionTranscriptsMock,
        ).toHaveBeenCalledOnce(),
      );
      allowed = false;
      finish();
      await rejected;
      expect(startTranscriptionMock).not.toHaveBeenCalled();
    },
  );

  test("drains admitted persistence using the original store after host closure and replacement", async () => {
    let allowed = true;
    let finishAppend!: () => void;
    const originalStore = { sessionTranscripts: sessionTranscriptsMock };
    applicationClientMock.mockReturnValue({ store: originalStore });
    appendTranscriptWordsAndHintsMock.mockImplementationOnce(
      () =>
        new Promise<void>((done) => {
          finishAppend = done;
        }),
    );
    startTranscriptionMock.mockImplementation(async (_params, options) => {
      allowed = false;
      applicationClientMock.mockReturnValue({
        store: { sessionTranscripts: vi.fn() },
      });
      options.handlePersist(
        [{ text: "first", start_ms: 0, end_ms: 100, channel: 0 }],
        [],
      );
      options.handlePersist(
        [{ text: "second", start_ms: 100, end_ms: 200, channel: 0 }],
        [],
      );
    });
    const { result, unmount } = renderHook(() =>
      useRunBatch("session-1", () => {
        if (!allowed) throw new Error("retired");
      }),
    );
    const completed = vi.fn();
    const pending = result.current("/tmp/audio.wav").then(completed);
    await waitFor(() =>
      expect(appendTranscriptWordsAndHintsMock).toHaveBeenCalledOnce(),
    );
    unmount();
    expect(createTranscriptMock.mock.calls[0][1]).toEqual({
      store: originalStore,
    });
    expect(appendTranscriptWordsAndHintsMock.mock.calls[0][3]).toEqual({
      store: originalStore,
    });
    expect(completed).not.toHaveBeenCalled();
    finishAppend();
    await pending;
    expect(completed).toHaveBeenCalledOnce();
  });

  test("waits for streamed persists before completing transcription", async () => {
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
    const completed = vi.fn();
    const run = result.current("/tmp/session.wav").then(completed);

    await waitFor(() => {
      expect(appendTranscriptWordsAndHintsMock).toHaveBeenCalledTimes(1);
    });

    expect(completed).not.toHaveBeenCalled();
    resolveAppend?.();
    await act(async () => await run);

    expect(createTranscriptMock).toHaveBeenCalledTimes(1);
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
  });

  test("defaults Whisper batch to English without inheriting unsupported legacy languages", async () => {
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "whisper-large-v3",
        baseUrl: "http://localhost:8080",
        apiKey: "",
      },
    });
    useConfigValueMock.mockImplementation((key) => {
      if (key === "ai_language") return "ga";
      if (key === "spoken_languages") return ["zu"];
      if (key === "meeting_languages") return undefined;
      return [];
    });
    isSupportedLanguagesBatchMock.mockImplementation(
      async (_provider, _model, languages) =>
        languages.every((language: string) => language === "en"),
    );
    startTranscriptionMock.mockResolvedValue(undefined);

    const { result } = renderHook(() => useRunBatch("session-1"));
    await act(async () => {
      await result.current("/tmp/session.wav");
    });

    expect(isSupportedLanguagesBatchMock).toHaveBeenCalledWith(
      "whispercpp",
      "whisper-large-v3",
      ["en"],
    );
    expect(startTranscriptionMock).toHaveBeenCalledWith(
      expect.objectContaining({
        provider: "whispercpp",
        model: "whisper-large-v3",
        languages: ["en"],
      }),
      expect.any(Object),
    );
    expect(sonnerToastWarningMock).not.toHaveBeenCalled();
  });

  test("uses explicit meeting languages independently of summary and legacy spoken languages", async () => {
    useConfigValueMock.mockImplementation((key) => {
      if (key === "ai_language") return "en";
      if (key === "meeting_languages") return ["nl"];
      if (key === "spoken_languages") return ["fr"];
      return [];
    });
    startTranscriptionMock.mockResolvedValue(undefined);

    const { result } = renderHook(() => useRunBatch("session-1"));
    await act(async () => {
      await result.current("/tmp/session.wav");
    });

    expect(isSupportedLanguagesBatchMock).toHaveBeenCalledWith(
      "soniqo",
      "soniqo-parakeet-batch",
      ["nl"],
    );
    expect(startTranscriptionMock).toHaveBeenCalledWith(
      expect.objectContaining({ languages: ["nl"] }),
      expect.any(Object),
    );
  });

  test("uses the selected streaming model for supported file transcription", async () => {
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-streaming",
        baseUrl: "soniqo://local",
        apiKey: "",
      },
    });
    const { result } = renderHook(() => useRunBatch("session-1"));
    await result.current("/tmp/session.wav", { languages: ["en"] });
    expect(startTranscriptionMock).toHaveBeenCalledWith(
      expect.objectContaining({
        model: "soniqo-parakeet-streaming",
        provider: "soniqo",
        languages: ["en"],
      }),
      expect.any(Object),
    );
  });

  test("rejects unsupported languages without switching the selected model", async () => {
    useSTTConnectionMock.mockReturnValue({
      conn: {
        provider: "fmtr",
        model: "soniqo-parakeet-streaming",
        baseUrl: "soniqo://local",
        apiKey: "",
      },
    });
    isSupportedLanguagesBatchMock.mockResolvedValue(false);
    const { result } = renderHook(() => useRunBatch("session-1"));
    await expect(result.current("/tmp/session.wav")).rejects.toThrow(
      "soniqo-parakeet-streaming cannot transcribe the selected meeting languages",
    );
    expect(startTranscriptionMock).not.toHaveBeenCalled();
    expect(sonnerToastWarningMock).not.toHaveBeenCalled();
  });

  test("requires a selected model instead of choosing a fallback when no connection exists", async () => {
    useSTTConnectionMock.mockReturnValue({ conn: null });
    const { result } = renderHook(() => useRunBatch("session-1"));
    await expect(result.current("/tmp/session.wav")).rejects.toThrow(
      "Select a transcription model in Settings",
    );
    expect(startTranscriptionMock).not.toHaveBeenCalled();
  });
  test.each([50, 150, 500])(
    "waits for original durable batch writes before terminal acknowledgement at %sms",
    async (delay) => {
      let finish!: () => void;
      createTranscriptMock.mockReturnValueOnce(
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
      );
      let admitted: { requestId: string } | undefined;
      startTranscriptionMock.mockImplementationOnce(
        async (_params, options) => {
          admitted = options;
          options.handlePersist(
            [{ text: "retained", start_ms: 0, end_ms: 1, channel: 0 }],
            [],
          );
          options.onTerminal();
        },
      );
      let available = true;
      const { result, unmount } = renderHook(() =>
        useRunBatch("session-1", () => {
          if (!available) throw new Error("retired");
        }),
      );
      const work = result.current("/tmp/audio.wav");
      await waitFor(() => expect(createTranscriptMock).toHaveBeenCalledOnce());
      available = false;
      unmount();
      await new Promise((resolve) => setTimeout(resolve, delay));
      expect(finishLocalWorkMock).not.toHaveBeenCalled();
      finish();
      await work;
      expect(finishLocalWorkMock).toHaveBeenCalledExactlyOnceWith(
        "session-1",
        admitted?.requestId,
      );
    },
  );
  test.each(["unknown-start", "failed-write", "void-custom"])(
    "never acknowledges unproven batch durability: %s",
    async (boundary) => {
      if (boundary === "failed-write")
        createTranscriptMock.mockRejectedValueOnce(new Error("write lost"));
      startTranscriptionMock.mockImplementationOnce(
        async (_params, options) => {
          options.handlePersist(
            [{ text: "retained", start_ms: 0, end_ms: 1, channel: 0 }],
            [],
          );
          if (boundary !== "unknown-start") options.onTerminal();
          if (boundary === "unknown-start")
            throw new Error("start response lost");
        },
      );
      const { result } = renderHook(() => useRunBatch("session-1"));
      await result
        .current(
          "/tmp/audio.wav",
          boundary === "void-custom" ? { handlePersist: () => {} } : undefined,
        )
        .catch(() => {});
      expect(finishLocalWorkMock).not.toHaveBeenCalled();
    },
  );
  test("drains all returned custom persistence promises, even out of order", async () => {
    let first!: () => void, second!: () => void;
    const handlePersist = vi
      .fn()
      .mockReturnValueOnce(
        new Promise<void>((resolve) => {
          first = resolve;
        }),
      )
      .mockReturnValueOnce(
        new Promise<void>((resolve) => {
          second = resolve;
        }),
      );
    startTranscriptionMock.mockImplementationOnce(async (_params, options) => {
      options.handlePersist([], []);
      options.handlePersist([], []);
      options.onTerminal();
    });
    const { result } = renderHook(() => useRunBatch("session-1"));
    const work = result.current("/tmp/audio.wav", { handlePersist });
    await waitFor(() => expect(handlePersist).toHaveBeenCalledTimes(2));
    second();
    await Promise.resolve();
    expect(finishLocalWorkMock).not.toHaveBeenCalled();
    first();
    await work;
    expect(finishLocalWorkMock).toHaveBeenCalledOnce();
  });
  test.each([false, true])(
    "acknowledges drained partial persistence after an actual stopped producer, preserving stop even if acknowledgement fails (%s)",
    async (acknowledgementFails) => {
      if (acknowledgementFails)
        finishLocalWorkMock.mockRejectedValueOnce(
          new Error("ack response lost"),
        );
      startTranscriptionMock.mockImplementationOnce(
        async (_params, options) => {
          options.handlePersist(
            [{ text: "partial", start_ms: 0, end_ms: 1, channel: 0 }],
            [],
          );
          options.onTerminal();
          throw new Error("Transcription stopped.");
        },
      );
      const { result } = renderHook(() => useRunBatch("session-1"));
      await expect(result.current("/tmp/audio.wav")).rejects.toThrow(
        "Transcription stopped.",
      );
      expect(createTranscriptMock).toHaveBeenCalledOnce();
      expect(finishLocalWorkMock).toHaveBeenCalledOnce();
    },
  );
  test("still drains a pending custom write when a different write has failed", async () => {
    let finish!: () => void;
    const handlePersist = vi
      .fn()
      .mockReturnValueOnce(
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
      )
      .mockImplementationOnce(async () => {
        throw new Error("write failed");
      });
    startTranscriptionMock.mockImplementationOnce(async (_params, options) => {
      options.handlePersist([], []);
      options.handlePersist([], []);
      options.onTerminal();
    });
    const { result } = renderHook(() => useRunBatch("session-1"));
    const completed = vi.fn();
    const work = result
      .current("/tmp/audio.wav", { handlePersist })
      .catch(completed);
    await waitFor(() => expect(handlePersist).toHaveBeenCalledTimes(2));
    expect(completed).not.toHaveBeenCalled();
    finish();
    await work;
    expect(completed).toHaveBeenCalledWith(
      expect.objectContaining({ message: "write failed" }),
    );
    expect(finishLocalWorkMock).not.toHaveBeenCalled();
  });
  test("keeps successful saved transcription when only its completion acknowledgement is lost", async () => {
    finishLocalWorkMock.mockRejectedValueOnce(new Error("reply lost"));
    startTranscriptionMock.mockImplementationOnce(async (_params, options) => {
      options.handlePersist(
        [{ text: "saved", start_ms: 0, end_ms: 1, channel: 0 }],
        [],
      );
      options.onTerminal();
    });
    const { result } = renderHook(() => useRunBatch("session-1"));
    await expect(result.current("/tmp/audio.wav")).resolves.toBeUndefined();
    expect(createTranscriptMock).toHaveBeenCalledOnce();
    expect(startTranscriptionMock).toHaveBeenCalledOnce();
    expect(finishLocalWorkMock).toHaveBeenCalledOnce();
    expect(sonnerToastWarningMock).toHaveBeenCalledWith(
      "Transcription completion could not be confirmed.",
      expect.anything(),
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
  "onnx-unknown",
  "whisper-unknown",
  "QuantizedUnknown",
])("rejects unsupported model %s", (model) => {
  expect(getBatchProvider("fmtr", model)).toBeNull();
  expect(canRunBatchTranscription({ provider: "fmtr", model })).toBe(false);
});
