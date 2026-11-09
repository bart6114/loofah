import { act, cleanup, renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  start: vi.fn(),
  config: vi.fn(),
  sidebar: vi.fn(),
  connection: { value: undefined as any },
  forbiddenWrite: vi.fn(),
}));
vi.mock("./contexts", () => ({
  useListener: (select: any) => select({ start: mocks.start }),
}));
vi.mock("./useSTTConnection", () => ({
  useSTTConnection: () => ({ conn: mocks.connection.value }),
}));
vi.mock("~/contexts/shell", () => ({
  useShell: () => ({ leftsidebar: { setExpanded: mocks.sidebar } }),
}));
vi.mock("~/session/queries", () => ({
  useSession: () => ({ user_id: "local-self" }),
}));
vi.mock("~/shared/config", () => ({
  useConfigValue: (key: string) =>
    key === "meeting_languages" ? ["en"] : "live",
}));
vi.mock("~/stt/capabilities", () => ({
  getTranscriptionLanguages: (value: unknown) => value,
  getLiveTranscriptionConfig: mocks.config,
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionAppendTranscript: mocks.forbiddenWrite,
    sessionFinishTranscript: mocks.forbiddenWrite,
    applicationFinishLocalSessionWork: mocks.forbiddenWrite,
  },
}));
vi.mock("~/ai/task-window-sync", () => ({
  requestMainAutoEnhance: mocks.forbiddenWrite,
}));
vi.mock("~/session/attachments", () => ({
  catalogLocalSessionAudio: mocks.forbiddenWrite,
}));

import { useStartListening } from "./useStartListening";

import { useSummaryActivity } from "~/session/summary-activity";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";

const scopes: VaultScope[] = [];
let finish: (() => void) | undefined;
afterEach(() => {
  finish?.();
  finish = undefined;
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});
beforeEach(() => {
  vi.clearAllMocks();
  mocks.connection.value = {
    provider: "fmtr",
    model: "QuantizedBase",
    baseUrl: "http://127.0.0.1:1234",
    apiKey: "never-forward",
  };
  mocks.config.mockResolvedValue({
    languages: ["en"],
    transcriptionMode: "live",
  });
  mocks.start.mockImplementation(async (_: unknown, options: any) => {
    finish = options.onStopped;
    return true;
  });
});
async function fixture() {
  let event!: (value: VaultClientEvent) => void;
  const scope = new VaultScope({
    scope: "local:original",
    networkMode: "always",
    subscribe: async (receive: typeof event) => {
      event = receive;
      receive({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
    close: async () => {},
  } as unknown as VaultClient);
  scopes.push(scope);
  await scope.start();
  return { scope, event };
}
function host(scope: VaultScope, children: ReactNode) {
  return createElement(
    VaultHostContext.Provider,
    { value: { scope, error: null, retry() {} } },
    children,
  );
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("hands one canonical request UUID to Native without renderer persistence or post-processing", async () => {
  const { scope } = await fixture();
  const { result } = renderHook(() => useStartListening("note", () => true), {
    wrapper: ({ children }) => host(scope, children),
  });
  await act(() => result.current());
  const [params, options] = mocks.start.mock.calls[0]!;
  expect(options.requestId).toMatch(
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
  expect(params).toMatchObject({
    session_id: "note",
    model: "QuantizedBase",
    api_key: "",
    self_human_id: "local-self",
  });
  expect(options).not.toHaveProperty("onData");
  expect(options).not.toHaveProperty("onPersist");
  expect(mocks.sidebar).toHaveBeenCalledWith(false);
  expect(useSummaryActivity.getState().sessions.note).toBe("recording");
  act(() => finish?.());
  expect(useSummaryActivity.getState().sessions.note).toBeUndefined();
  expect(mocks.forbiddenWrite).not.toHaveBeenCalled();
});

test("same-string scope replacement during model preparation cannot dispatch into the new owner", async () => {
  const original = await fixture();
  const replacement = await fixture();
  let scope = original.scope;
  const pending = deferred<{
    languages: string[];
    transcriptionMode: string;
  }>();
  mocks.config.mockReturnValueOnce(pending.promise);
  const { result, rerender } = renderHook(
    () => useStartListening("note", () => true),
    { wrapper: ({ children }) => host(scope, children) },
  );
  let operation!: Promise<void>;
  act(() => {
    operation = result.current();
  });
  scope = replacement.scope;
  rerender();
  await act(async () => {
    pending.resolve({ languages: ["en"], transcriptionMode: "live" });
    await operation;
  });
  expect(mocks.start).not.toHaveBeenCalled();
  expect(useSummaryActivity.getState().sessions.note).toBeUndefined();
});

test.each(["closed", "access_denied", "login_required"] as const)(
  "%s retires an in-flight preparation",
  async (state) => {
    const owner = await fixture();
    const pending = deferred<{ languages: string[] }>();
    mocks.config.mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useStartListening("note", () => true), {
      wrapper: ({ children }) => host(owner.scope, children),
    });
    let operation!: Promise<void>;
    act(() => {
      operation = result.current();
    });
    act(() => owner.event({ kind: "connection", state, error: null }));
    await act(async () => {
      pending.resolve({ languages: ["en"] });
      await operation;
    });
    expect(mocks.start).not.toHaveBeenCalled();
  },
);

test("late stop releases only its own activity and never starts renderer follow-up work", async () => {
  const owner = await fixture();
  const { result, unmount } = renderHook(
    () => useStartListening("note", () => true),
    { wrapper: ({ children }) => host(owner.scope, children) },
  );
  await act(() => result.current());
  const first = finish!;
  await act(() => result.current());
  const second = finish!;
  expect(mocks.start.mock.calls[0]![1].requestId).not.toBe(
    mocks.start.mock.calls[1]![1].requestId,
  );
  unmount();
  owner.scope.dispose();
  first();
  expect(useSummaryActivity.getState().sessions.note).toBe("recording");
  second();
  expect(useSummaryActivity.getState().sessions.note).toBeUndefined();
  expect(mocks.forbiddenWrite).not.toHaveBeenCalled();
});

test("an unconfigured Local model can still record and lets Native decide follow-up eligibility", async () => {
  mocks.connection.value = undefined;
  const { scope } = await fixture();
  const { result } = renderHook(() => useStartListening("note", () => true), {
    wrapper: ({ children }) => host(scope, children),
  });
  await act(() => result.current());
  expect(mocks.start.mock.calls[0]![0]).toMatchObject({
    model: "",
    base_url: "",
    transcription_mode: "batch",
    api_key: "",
  });
});

test("a rejected start clears activity without pretending persistence succeeded", async () => {
  mocks.start.mockResolvedValueOnce(false);
  const { scope } = await fixture();
  const { result } = renderHook(() => useStartListening("note", () => true), {
    wrapper: ({ children }) => host(scope, children),
  });
  await act(() => result.current());
  expect(useSummaryActivity.getState().sessions.note).toBeUndefined();
  expect(mocks.sidebar).not.toHaveBeenCalled();
  expect(mocks.forbiddenWrite).not.toHaveBeenCalled();
});
