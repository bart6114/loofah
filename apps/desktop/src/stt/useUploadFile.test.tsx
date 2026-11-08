import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { isAudioUploadFile, useUploadFile } from "./useUploadFile";

import { enqueueSessionAudioOperation } from "~/session/audio-operations";

const {
  audioSourceMetadataMock,
  audioImportDataMock,
  audioImportMock,
  audioImportListenMock,
  downloadDirMock,
  selectFileMock,
  parseSubtitleMock,
  createTranscriptMock,
  enhanceMock,
  autoEnhanceMock,
  getEnhancerServiceMock,
  summaryOwner,
  handleBatchFailedMock,
  handleBatchStartedMock,
  updateBatchProgressMock,
  clearBatchSessionMock,
  runBatchMock,
  useSessionMock,
  updateSessionMock,
  useTabsMock,
  updateSessionTabStateMock,
} = vi.hoisted(() => ({
  audioSourceMetadataMock: vi.fn(),
  audioImportDataMock: vi.fn(),
  audioImportMock: vi.fn(),
  audioImportListenMock: vi.fn(),
  downloadDirMock: vi.fn(),
  selectFileMock: vi.fn(),
  parseSubtitleMock: vi.fn(),
  createTranscriptMock: vi.fn(),
  enhanceMock: vi.fn(),
  autoEnhanceMock: vi.fn(),
  getEnhancerServiceMock: vi.fn(),
  summaryOwner: { scope: { client: { scope: "local" } }, current: vi.fn() },
  handleBatchFailedMock: vi.fn(),
  handleBatchStartedMock: vi.fn(),
  updateBatchProgressMock: vi.fn(),
  clearBatchSessionMock: vi.fn(),
  runBatchMock: vi.fn(),
  useSessionMock: vi.fn(),
  updateSessionMock: vi.fn(),
  useTabsMock: vi.fn(),
  updateSessionTabStateMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/path", () => ({
  downloadDir: downloadDirMock,
  resolveResource: vi.fn((path: string) =>
    Promise.resolve(`/resources/${path}`),
  ),
  sep: vi.fn().mockReturnValue("/"),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: selectFileMock,
}));

vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: {
    audioImport: audioImportMock,
    audioImportData: audioImportDataMock,
    audioSourceMetadata: audioSourceMetadataMock,
  },
  events: {
    audioImportEvent: {
      listen: audioImportListenMock,
    },
  },
}));

vi.mock("@hypr/plugin-transcription", () => ({
  commands: { parseSubtitle: parseSubtitleMock },
}));

vi.mock("./contexts", () => ({
  useListener: (selector: (state: unknown) => unknown) =>
    selector({
      handleBatchStarted: handleBatchStartedMock,
      handleBatchFailed: handleBatchFailedMock,
      updateBatchProgress: updateBatchProgressMock,
      clearBatchSession: clearBatchSessionMock,
    }),
}));

vi.mock("./useRunBatch", () => ({
  isStoppedTranscriptionError: vi.fn(() => false),
  useRunBatch: vi.fn(() => runBatchMock),
}));

vi.mock("~/services/enhancer", () => ({
  getEnhancerService: getEnhancerServiceMock,
}));

vi.mock("~/shared/application/use-vault-read-scope", () => ({
  useVaultReadScope: () => summaryOwner,
}));

vi.mock("~/session/queries", () => ({
  useSession: useSessionMock,
  useUpdateSession: () => updateSessionMock,
}));

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: useTabsMock,
}));

vi.mock("~/stt/queries", () => ({
  createTranscript: createTranscriptMock,
}));

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

describe("useUploadFile", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    summaryOwner.current.mockReturnValue(true);
    autoEnhanceMock.mockResolvedValue({ type: "queued" });
    getEnhancerServiceMock.mockReturnValue({
      scope: summaryOwner.scope,
      enhance: enhanceMock,
      queueAutoEnhanceIfSummaryEmpty: autoEnhanceMock,
    });

    audioImportDataMock.mockResolvedValue({
      status: "ok",
      data: "/vault/sessions/session-1/audio.wav",
    });
    audioImportMock.mockResolvedValue({
      status: "ok",
      data: "/vault/sessions/session-1/audio.wav",
    });
    audioImportListenMock.mockResolvedValue(vi.fn());
    audioSourceMetadataMock.mockResolvedValue({
      status: "ok",
      data: {
        createdAt: "2026-03-26T12:00:00.000Z",
        modifiedAt: null,
        durationMs: 30_000,
      },
    });
    downloadDirMock.mockResolvedValue("/downloads");
    selectFileMock.mockResolvedValue("/tmp/replacement.wav");
    runBatchMock.mockResolvedValue(undefined);
    createTranscriptMock.mockResolvedValue(undefined);
    enhanceMock.mockResolvedValue({ type: "started", noteId: "note-1" });
    useSessionMock.mockReturnValue({
      id: "session-1",
      user_id: "user-1",
      raw_md: "",
      event_json: "",
    });
    updateSessionMock.mockResolvedValue(undefined);
    useTabsMock.mockImplementation((selector) =>
      selector({
        tabs: [],
        updateSessionTabState: updateSessionTabStateMock,
      }),
    );
  });

  test.each(["audio", "transcript"] as const)(
    "refuses a late %s picker after its host is unavailable",
    async (kind) => {
      let finish!: (path: string) => void;
      let allowed = true;
      selectFileMock.mockImplementationOnce(
        () =>
          new Promise((done) => {
            finish = done;
          }),
      );
      const check = () => {
        if (!allowed) throw new Error("retired");
      };
      const { result } = renderHook(() => useUploadFile("session-1", check), {
        wrapper: createWrapper(),
      });
      const pending =
        kind === "audio"
          ? result.current.uploadAudio()
          : result.current.uploadTranscript();
      await waitFor(() => expect(selectFileMock).toHaveBeenCalledOnce());
      allowed = false;
      await act(async () => {
        finish(kind === "audio" ? "/tmp/file.wav" : "/tmp/file.vtt");
        await pending;
      });
      expect(audioSourceMetadataMock).not.toHaveBeenCalled();
      expect(audioImportMock).not.toHaveBeenCalled();
      expect(parseSubtitleMock).not.toHaveBeenCalled();
      expect(createTranscriptMock).not.toHaveBeenCalled();
      expect(enhanceMock).not.toHaveBeenCalled();
    },
  );

  test.each(["metadata", "listener", "queue"])(
    "rechecks the captured owner after waiting for %s",
    async (boundary) => {
      let allowed = true;
      let finish!: () => void;
      const blocker = new Promise<void>((done) => {
        finish = done;
      });
      if (boundary === "metadata")
        audioSourceMetadataMock.mockImplementationOnce(async () => {
          await blocker;
          return {
            status: "ok",
            data: {
              createdAt: "2026-03-26T12:00:00.000Z",
              modifiedAt: null,
              durationMs: 30000,
            },
          };
        });
      if (boundary === "listener")
        audioImportListenMock.mockImplementationOnce(async () => {
          await blocker;
          return vi.fn();
        });
      if (boundary === "queue")
        void enqueueSessionAudioOperation("session-1", () => blocker);
      const check = () => {
        if (!allowed) throw new Error("retired");
      };
      const { result } = renderHook(() => useUploadFile("session-1", check), {
        wrapper: createWrapper(),
      });
      const pending = result.current.processAudioPath("/tmp/file.wav");
      await waitFor(() =>
        expect(
          boundary === "metadata"
            ? audioSourceMetadataMock
            : boundary === "listener"
              ? audioImportListenMock
              : handleBatchStartedMock,
        ).toHaveBeenCalledOnce(),
      );
      allowed = false;
      await act(async () => {
        finish();
        await pending;
      });
      expect(audioImportMock).not.toHaveBeenCalled();
      expect(runBatchMock).not.toHaveBeenCalled();
      if (boundary === "metadata")
        expect(updateSessionMock).not.toHaveBeenCalled();
    },
  );

  test("lets an admitted copy settle but skips transcription after retirement", async () => {
    let allowed = true;
    let finish!: () => void;
    audioImportMock.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finish = () => done({ status: "ok", data: "/vault/copied.wav" });
        }),
    );
    const { result } = renderHook(
      () =>
        useUploadFile("session-1", () => {
          if (!allowed) throw new Error("retired");
        }),
      { wrapper: createWrapper() },
    );
    const pending = result.current.processAudioPath("/tmp/file.wav");
    await waitFor(() => expect(audioImportMock).toHaveBeenCalledOnce());
    allowed = false;
    await act(async () => {
      finish();
      await pending;
    });
    expect(runBatchMock).not.toHaveBeenCalled();
    expect(clearBatchSessionMock).toHaveBeenCalledWith("session-1");
    expect(handleBatchFailedMock).not.toHaveBeenCalled();
    expect(enhanceMock).not.toHaveBeenCalled();
  });

  test("refuses a parsed subtitle returned after retirement", async () => {
    let allowed = true;
    let finish!: () => void;
    parseSubtitleMock.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finish = () =>
            done({
              status: "ok",
              data: { tokens: [{ text: "Late", start_time: 0, end_time: 1 }] },
            });
        }),
    );
    const { result } = renderHook(
      () =>
        useUploadFile("session-1", () => {
          if (!allowed) throw new Error("retired");
        }),
      { wrapper: createWrapper() },
    );
    const pending = result.current.processFile("/tmp/file.vtt", "transcript");
    await waitFor(() => expect(parseSubtitleMock).toHaveBeenCalledOnce());
    allowed = false;
    await act(async () => {
      finish();
      await pending;
    });
    expect(createTranscriptMock).not.toHaveBeenCalled();
    expect(enhanceMock).not.toHaveBeenCalled();
  });

  test("infers the session date for an ordinary audio upload", async () => {
    const { result } = renderHook(() => useUploadFile("session-1"), {
      wrapper: createWrapper(),
    });

    act(() => {
      void result.current.uploadAudio();
    });

    await waitFor(() => {
      expect(updateSessionMock).toHaveBeenCalledWith({
        created_at: "2026-03-26T11:59:30.000Z",
      });
    });
    expect(audioSourceMetadataMock).toHaveBeenCalledWith(
      "/tmp/replacement.wav",
    );
  });

  test("imports pathless dropped audio using file bytes", async () => {
    const { result } = renderHook(() => useUploadFile("session-1"), {
      wrapper: createWrapper(),
    });
    const file = new File([new Uint8Array([1, 2, 3])], "drop.wav", {
      type: "audio/wav",
      lastModified: 1_700_000_000_000,
    });
    Object.defineProperty(file, "arrayBuffer", {
      value: vi.fn().mockResolvedValue(new Uint8Array([1, 2, 3]).buffer),
    });

    act(() => {
      void result.current.processAudioFile(file);
    });

    await waitFor(() => expect(runBatchMock).toHaveBeenCalled());
    expect(audioImportDataMock).toHaveBeenCalledWith(
      "session-1",
      [1, 2, 3],
      "drop.wav",
      "audio/wav",
    );
    expect(audioImportMock).not.toHaveBeenCalled();
    expect(runBatchMock).toHaveBeenCalledWith(
      "/vault/sessions/session-1/audio.wav",
      { imported: true },
    );
    expect(handleBatchFailedMock).not.toHaveBeenCalled();
  });

  test.each(["webm", "aac"])(
    "imports pathless .%s drops without MIME",
    async (extension) => {
      const { result } = renderHook(() => useUploadFile("session-1"), {
        wrapper: createWrapper(),
      });
      const file = new File([new Uint8Array([1, 2, 3])], `drop.${extension}`, {
        type: "",
        lastModified: 1_700_000_000_000,
      });
      Object.defineProperty(file, "arrayBuffer", {
        value: vi.fn().mockResolvedValue(new Uint8Array([1, 2, 3]).buffer),
      });

      expect(isAudioUploadFile(file)).toBe(true);

      act(() => {
        void result.current.processAudioFile(file);
      });

      await waitFor(() => {
        expect(audioImportDataMock).toHaveBeenCalled();
      });
      expect(audioImportDataMock).toHaveBeenCalledWith(
        "session-1",
        [1, 2, 3],
        `drop.${extension}`,
        null,
      );
    },
  );

  test("imports copied Voice Memos audio using its QuickTime MIME", async () => {
    const { result } = renderHook(() => useUploadFile("session-1"), {
      wrapper: createWrapper(),
    });
    const file = new File([new Uint8Array([1, 2, 3])], "Brian Shin.qta", {
      type: "audio/quicktime",
      lastModified: 1_700_000_000_000,
    });
    Object.defineProperty(file, "arrayBuffer", {
      value: vi.fn().mockResolvedValue(new Uint8Array([1, 2, 3]).buffer),
    });

    act(() => {
      void result.current.processAudioFile(file);
    });

    await waitFor(() => expect(runBatchMock).toHaveBeenCalled());
    expect(audioImportDataMock).toHaveBeenCalledWith(
      "session-1",
      [1, 2, 3],
      "Brian Shin.qta",
      "audio/quicktime",
    );
  });

  test.each(["audio", "transcript"])(
    "does not dispatch imported %s to a replacement service with the same scope string",
    async (kind) => {
      getEnhancerServiceMock.mockReturnValue({
        scope: { client: { scope: "local" } },
        enhance: enhanceMock,
        queueAutoEnhanceIfSummaryEmpty: autoEnhanceMock,
      });
      parseSubtitleMock.mockResolvedValue({
        status: "ok",
        data: { tokens: [{ text: "Hello", start_time: 0, end_time: 500 }] },
      });
      const { result } = renderHook(() => useUploadFile("session-1"), {
        wrapper: createWrapper(),
      });
      await act(async () => {
        if (kind === "audio")
          await result.current.processAudioPath("/tmp/file.wav");
        else await result.current.processFile("/tmp/file.vtt", "transcript");
      });
      expect(getEnhancerServiceMock).toHaveBeenCalled();
      expect(enhanceMock).not.toHaveBeenCalled();
      expect(autoEnhanceMock).not.toHaveBeenCalled();
    },
  );

  test("does not dispatch a delayed imported transcript after its owner retires", async () => {
    let finish!: () => void;
    createTranscriptMock.mockImplementationOnce(
      () =>
        new Promise<void>((done) => {
          finish = done;
        }),
    );
    parseSubtitleMock.mockResolvedValue({
      status: "ok",
      data: { tokens: [{ text: "Hello", start_time: 0, end_time: 500 }] },
    });
    const { result } = renderHook(() => useUploadFile("session-1"), {
      wrapper: createWrapper(),
    });
    const pending = result.current.processFile("/tmp/file.vtt", "transcript");
    await waitFor(() => expect(createTranscriptMock).toHaveBeenCalled());
    summaryOwner.current.mockReturnValue(false);
    await act(async () => {
      finish();
      await pending;
    });
    expect(enhanceMock).not.toHaveBeenCalled();
    expect(autoEnhanceMock).not.toHaveBeenCalled();
  });

  test("persists imported subtitles before enhancing", async () => {
    let resolveWrite: (() => void) | undefined;
    createTranscriptMock.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveWrite = resolve;
        }),
    );
    parseSubtitleMock.mockResolvedValue({
      status: "ok",
      data: {
        tokens: [{ text: "Hello", start_time: 0, end_time: 500 }],
      },
    });
    const { result } = renderHook(() => useUploadFile("session-1"), {
      wrapper: createWrapper(),
    });

    act(() => {
      void result.current.processFile("/tmp/session.vtt", "transcript");
    });

    await waitFor(() => {
      expect(createTranscriptMock).toHaveBeenCalledTimes(1);
    });
    expect(enhanceMock).not.toHaveBeenCalled();

    await act(async () => {
      resolveWrite?.();
    });
    await waitFor(() => {
      expect(enhanceMock).toHaveBeenCalledWith("session-1");
    });
    expect(createTranscriptMock).toHaveBeenCalledWith(
      expect.objectContaining({
        sessionId: "session-1",
        ownerUserId: "user-1",
        source: "subtitle_import",
        words: [
          expect.objectContaining({
            text: "Hello",
            start_ms: 0,
            end_ms: 500,
          }),
        ],
      }),
    );
    expect(createTranscriptMock.mock.invocationCallOrder[0]).toBeLessThan(
      enhanceMock.mock.invocationCallOrder[0],
    );
  });
});
