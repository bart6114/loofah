import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { generateText } from "ai";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  AI_GENERATION_MAX_RETRIES,
  createLanguageModel,
  SUMMARY_MAX_OUTPUT_TOKENS,
} from "@hypr/ai-providers";

import { SummaryProviderBridge } from "./SummaryProviderBridge";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-http", () => ({ fetch: vi.fn() }));
vi.mock("ai", () => ({ generateText: vi.fn() }));
vi.mock("@hypr/ai-providers", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@hypr/ai-providers")>()),
  createLanguageModel: vi.fn(),
}));

const handlers = new Map<string, (event: { payload: unknown }) => void>();
const unlisteners = new Map<string, ReturnType<typeof vi.fn>>();
const model = {};
const request = {
  id: "request-1",
  instructions: "Shared desktop instructions in Dutch.",
  input: "# Notes\nComplete notes.\n# Transcript\nComplete transcript.",
  connection: {
    providerId: "openrouter" as const,
    modelId: "provider/model",
    baseUrl: "https://openrouter.ai/api/v1",
    apiKey: "secret/key",
  },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function emit(name: string, payload: unknown) {
  act(() => handlers.get(name)?.({ payload }));
}

async function mountReady() {
  const view = render(<SummaryProviderBridge />);
  await waitFor(() => {
    expect(invoke).toHaveBeenCalledWith("mobile_summary_bridge_ready", {
      ready: true,
    });
  });
  return view;
}

beforeEach(() => {
  vi.resetAllMocks();
  handlers.clear();
  unlisteners.clear();
  vi.mocked(invoke).mockResolvedValue(undefined);
  vi.mocked(createLanguageModel).mockReturnValue(model as never);
  vi.mocked(generateText).mockResolvedValue({
    text: "Complete summary",
    finishReason: "stop",
  } as never);
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    handlers.set(name, handler as (event: { payload: unknown }) => void);
    const unlisten = vi.fn();
    unlisteners.set(name, unlisten);
    return unlisten;
  });
});

afterEach(async () => {
  cleanup();
  await act(async () => {});
});

describe("mobile summary provider bridge", () => {
  it("announces readiness only after both listeners are installed", async () => {
    const abort = deferred<() => void>();
    vi.mocked(listen).mockImplementationOnce(async () => vi.fn<() => void>());
    vi.mocked(listen).mockImplementationOnce(() => abort.promise);
    render(<SummaryProviderBridge />);
    await waitFor(() => expect(listen).toHaveBeenCalledTimes(2));
    expect(invoke).not.toHaveBeenCalled();
    await act(async () => abort.resolve(vi.fn()));
    expect(invoke).toHaveBeenCalledWith("mobile_summary_bridge_ready", {
      ready: true,
    });
  });

  it("uses shared model, transport, full instructions and source, and generation limits", async () => {
    await mountReady();
    emit("mobile-summary-request", request);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_summary_complete", {
        requestId: request.id,
        text: "Complete summary",
        error: null,
      }),
    );
    expect(createLanguageModel).toHaveBeenCalledWith(request.connection, {
      fetch: tauriFetch,
    });
    expect(generateText).toHaveBeenCalledWith({
      model,
      system: request.instructions,
      prompt: request.input,
      maxOutputTokens: SUMMARY_MAX_OUTPUT_TOKENS,
      maxRetries: AI_GENERATION_MAX_RETRIES,
      abortSignal: expect.any(AbortSignal),
    });
  });

  it("uses the title request limit independently of the summary limit", async () => {
    await mountReady();
    emit("mobile-summary-request", { ...request, maxOutputTokens: 128 });
    await waitFor(() => expect(generateText).toHaveBeenCalled());
    expect(generateText).toHaveBeenCalledWith(
      expect.objectContaining({ maxOutputTokens: 128 }),
    );
  });

  it.each([
    ["", "stop", "returned no text"],
    ["Partial summary", "length", "output limit"],
  ])(
    "rejects unusable output %s with finish reason %s",
    async (text, finishReason, message) => {
      vi.mocked(generateText).mockResolvedValue({
        text,
        finishReason,
      } as never);
      await mountReady();
      emit("mobile-summary-request", request);
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith("mobile_summary_complete", {
          requestId: request.id,
          text: null,
          error: expect.stringContaining(message),
        }),
      );
    },
  );

  it("redacts raw and encoded keys from provider failures", async () => {
    vi.mocked(generateText).mockRejectedValue(
      new Error("Failure secret/key and secret%2Fkey"),
    );
    await mountReady();
    emit("mobile-summary-request", request);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_summary_complete", {
        requestId: request.id,
        text: null,
        error: "Failure [redacted] and [redacted]",
      }),
    );
  });

  it("preserves native transport string failures and redacts credentials", async () => {
    vi.mocked(generateText).mockRejectedValue(
      "HTTP request denied for secret/key and secret%2Fkey",
    );
    await mountReady();
    emit("mobile-summary-request", request);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_summary_complete", {
        requestId: request.id,
        text: null,
        error: "HTTP request denied for [redacted] and [redacted]",
      }),
    );
  });

  it("aborts only the active request and ignores its late generation result", async () => {
    const generation = deferred<never>();
    vi.mocked(generateText).mockReturnValue(generation.promise);
    await mountReady();
    emit("mobile-summary-request", request);
    const signal = vi.mocked(generateText).mock.calls[0][0].abortSignal!;
    emit("mobile-summary-abort", "stale-request");
    expect(signal.aborted).toBe(false);
    emit("mobile-summary-abort", request.id);
    expect(signal.aborted).toBe(true);
    await act(async () =>
      generation.resolve({
        text: "Late result",
        finishReason: "stop",
      } as never),
    );
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([name]) => name === "mobile_summary_complete"),
    ).toHaveLength(0);
  });

  it("unmount removes listeners, aborts generation, and prevents late callbacks", async () => {
    const generation = deferred<never>();
    vi.mocked(generateText).mockReturnValue(generation.promise);
    const view = await mountReady();
    emit("mobile-summary-request", request);
    const signal = vi.mocked(generateText).mock.calls[0][0].abortSignal!;
    view.unmount();
    expect(signal.aborted).toBe(true);
    for (const unlisten of unlisteners.values())
      expect(unlisten).toHaveBeenCalledOnce();
    emit("mobile-summary-request", { ...request, id: "after-unmount" });
    await act(async () => generation.reject(new Error("Late failure")));
    expect(generateText).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("mobile_summary_bridge_ready", {
      ready: false,
    });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([name]) => name === "mobile_summary_complete"),
    ).toHaveLength(0);
  });

  it("cleans up a listener that resolves after unmount without announcing readiness", async () => {
    const pending = deferred<() => void>();
    const unlisten = vi.fn();
    vi.mocked(listen).mockImplementationOnce(() => pending.promise);
    const view = render(<SummaryProviderBridge />);
    view.unmount();
    await act(async () => pending.resolve(unlisten));
    expect(unlisten).toHaveBeenCalledOnce();
    expect(listen).toHaveBeenCalledOnce();
    expect(invoke).not.toHaveBeenCalledWith("mobile_summary_bridge_ready", {
      ready: true,
    });
  });

  it("removes both listeners when the abort listener resolves after unmount", async () => {
    const pending = deferred<() => void>();
    const requestUnlisten = vi.fn<() => void>();
    const abortUnlisten = vi.fn<() => void>();
    vi.mocked(listen).mockImplementationOnce(async () => requestUnlisten);
    vi.mocked(listen).mockImplementationOnce(() => pending.promise);
    const view = render(<SummaryProviderBridge />);
    await waitFor(() => expect(listen).toHaveBeenCalledTimes(2));
    view.unmount();
    expect(requestUnlisten).toHaveBeenCalledOnce();
    await act(async () => pending.resolve(abortUnlisten));
    expect(abortUnlisten).toHaveBeenCalledOnce();
    expect(invoke).not.toHaveBeenCalledWith("mobile_summary_bridge_ready", {
      ready: true,
    });
  });

  it("keeps one active generation and ignores a replaced request result", async () => {
    const first = deferred<never>();
    const second = deferred<never>();
    vi.mocked(generateText)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    await mountReady();
    emit("mobile-summary-request", request);
    const signal = vi.mocked(generateText).mock.calls[0][0].abortSignal!;
    emit("mobile-summary-request", { ...request, id: "request-2" });
    expect(signal.aborted).toBe(true);
    await act(async () =>
      first.resolve({ text: "Stale summary", finishReason: "stop" } as never),
    );
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([name]) => name === "mobile_summary_complete"),
    ).toHaveLength(0);
    await act(async () =>
      second.resolve({
        text: "Current summary",
        finishReason: "stop",
      } as never),
    );
    expect(invoke).toHaveBeenCalledWith("mobile_summary_complete", {
      requestId: "request-2",
      text: "Current summary",
      error: null,
    });
  });

  it("resets readiness after a pending ready call settles during unmount", async () => {
    const pending = deferred<void>();
    vi.mocked(invoke).mockImplementation((name, args) =>
      name === "mobile_summary_bridge_ready" &&
      (args as { ready: boolean }).ready
        ? (pending.promise as never)
        : (Promise.resolve() as never),
    );
    const view = await mountReady();
    view.unmount();
    await act(async () => pending.resolve());
    expect(invoke).toHaveBeenLastCalledWith("mobile_summary_bridge_ready", {
      ready: false,
    });
  });
});
