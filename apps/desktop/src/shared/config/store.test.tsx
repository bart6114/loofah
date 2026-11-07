import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { AppConfig } from "@hypr/plugin-settings";

const mocks = vi.hoisted(() => ({
  get: vi.fn(),
  set: vi.fn(),
  listen: vi.fn(),
}));
vi.mock("@hypr/plugin-settings", () => ({
  commands: { getConfig: mocks.get, setConfigValues: mocks.set },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

import {
  fetchConfig,
  initConfigStore,
  resetConfigStoreForTests,
  useConfigStoreState,
  writeConfigValues,
} from "./store";

let receive!: (event: { payload: AppConfig }) => void;
const config = (ai_language: string) => ({ ai_language }) as AppConfig;
const ok = (ai_language: string) => ({
  status: "ok",
  data: config(ai_language),
});
beforeEach(() => {
  vi.resetAllMocks();
  resetConfigStoreForTests();
  mocks.listen.mockImplementation(
    async (_name: string, callback: typeof receive) => {
      receive = callback;
      return () => {};
    },
  );
  mocks.get.mockResolvedValue(ok("en"));
  mocks.set.mockResolvedValue({ status: "ok", data: null });
});
afterEach(cleanup);

it("does not overwrite a newer canonical event when an older setter finishes", async () => {
  const { result } = renderHook(useConfigStoreState);
  await act(async () => {
    await initConfigStore();
  });
  let finish!: (value: unknown) => void;
  mocks.set.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const pending = writeConfigValues({ ai_language: "nl" });
  act(() => receive({ payload: config("fr") }));
  await act(async () => {
    finish({ status: "ok", data: null });
    await pending;
  });
  expect(result.current.snapshot?.config.ai_language).toBe("fr");
});

it("does not let a slow initial read overwrite an event received during bootstrap", async () => {
  let finish!: (value: unknown) => void;
  mocks.get.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { result } = renderHook(useConfigStoreState);
  await waitFor(() => expect(mocks.get).toHaveBeenCalledTimes(1));
  act(() => receive({ payload: config("fr") }));
  await act(async () => {
    finish(ok("en"));
    await initConfigStore();
  });
  expect(result.current.snapshot?.config.ai_language).toBe("fr");
});

it("returns the latest event snapshot when an in-flight read fails after that event", async () => {
  const { result } = renderHook(useConfigStoreState);
  await act(async () => {
    await initConfigStore();
  });
  let finish!: (value: unknown) => void;
  mocks.get.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const pending = fetchConfig();
  act(() => receive({ payload: config("de") }));
  await act(async () => {
    finish({ status: "error", error: "late failure" });
    expect((await pending).ai_language).toBe("de");
  });
  expect(result.current.error).toBeNull();
});

it("keeps canonical state until a write is confirmed by its native event", async () => {
  const { result } = renderHook(useConfigStoreState);
  await act(async () => {
    await initConfigStore();
  });
  await act(async () => {
    await writeConfigValues({ ai_language: "nl" });
  });
  expect(result.current.snapshot?.config.ai_language).toBe("en");
  act(() => receive({ payload: config("nl") }));
  expect(result.current.snapshot?.config.ai_language).toBe("nl");
});

it("does not let out-of-order concurrent reads replace a later requested snapshot", async () => {
  const { result } = renderHook(useConfigStoreState);
  await act(async () => {
    await initConfigStore();
  });
  let old!: (value: unknown) => void;
  let newer!: (value: unknown) => void;
  mocks.get
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          old = resolve;
        }),
    )
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          newer = resolve;
        }),
    );
  const first = fetchConfig();
  const second = fetchConfig();
  await act(async () => {
    newer(ok("fr"));
    await second;
    old(ok("nl"));
    expect((await first).ai_language).toBe("fr");
  });
  expect(result.current.snapshot?.config.ai_language).toBe("fr");
});

it("keeps a newer event if the initial native invoke rejects", async () => {
  let fail!: (error: Error) => void;
  mocks.get.mockImplementationOnce(
    () =>
      new Promise((_resolve, reject) => {
        fail = reject;
      }),
  );
  const { result } = renderHook(useConfigStoreState);
  await waitFor(() => expect(mocks.get).toHaveBeenCalledTimes(1));
  act(() => receive({ payload: config("fr") }));
  await act(async () => {
    fail(new Error("native invoke failed"));
    await initConfigStore();
  });
  expect(result.current.snapshot?.config.ai_language).toBe("fr");
  expect(result.current.error).toBeNull();
});

it("returns a newer event when an explicit native read invoke rejects", async () => {
  const { result } = renderHook(useConfigStoreState);
  await act(async () => {
    await initConfigStore();
  });
  let fail!: (error: Error) => void;
  mocks.get.mockImplementationOnce(
    () =>
      new Promise((_resolve, reject) => {
        fail = reject;
      }),
  );
  const pending = fetchConfig();
  act(() => receive({ payload: config("de") }));
  await act(async () => {
    fail(new Error("native invoke failed"));
    expect((await pending).ai_language).toBe("de");
  });
  expect(result.current.snapshot?.config.ai_language).toBe("de");
  expect(result.current.error).toBeNull();
});
