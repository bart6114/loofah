import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, renderHook } from "@testing-library/react";
import { StrictMode, type ReactNode } from "react";
import { afterEach, expect, it } from "vitest";

import { MainHostContext, type MainCaptureActivity } from "./host-context";
import { useLegacySessionActivity } from "./use-legacy-activity";

import { VaultHostContext } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";
import {
  createListenerStore,
  type ListenerStore,
} from "~/store/zustand/listener";
import { scopedFixture } from "~/store/zustand/tabs/scoped-test-utils";

const features = [
  "legacy_desktop_transcript_controls",
  "legacy_desktop_session_controls",
  "legacy_desktop_enhance",
];
function fixture() {
  const f = scopedFixture("same-scope");
  Object.assign(f.scope.client, { features, networkMode: "always" });
  const listener = createListenerStore();
  listener.setState((state) => ({
    live: {
      ...state.live,
      sessionId: "same-id",
      status: "active",
      loading: false,
    },
  }));
  const activity: MainCaptureActivity = {
    scope: f.scope,
    captureState: () => "idle",
    subscribeCapture: () => () => {},
  };
  return { ...f, listener, activity };
}
function Host({
  scope,
  listener,
  activity,
  children,
}: {
  scope: VaultScope;
  listener: ListenerStore;
  activity?: MainCaptureActivity;
  children: ReactNode;
}) {
  return (
    <StrictMode>
      <VaultHostContext.Provider value={{ scope, error: null, retry() {} }}>
        <MainHostContext.Provider
          value={{ scope, listenerStore: listener, activity }}
        >
          {children}
        </MainHostContext.Provider>
      </VaultHostContext.Provider>
    </StrictMode>
  );
}
afterEach(() => {
  cleanup();
  onlineManager.setOnline(true);
});

it("preserves captured Local activity offline without login or a device bridge", () => {
  const f = fixture();
  onlineManager.setOnline(false);
  const { result } = renderHook(() => useLegacySessionActivity("same-id"), {
    wrapper: ({ children }) => <Host {...f}>{children}</Host>,
  });
  expect(result.current.isAvailable()).toBe(true);
  expect(result.current.mode).toBe("active");
  expect(result.current.liveSessionId).toBe("same-id");
});

it.each(["absent", "different-object", "unsupported", "reader"])(
  "does not borrow identical-ID Local activity with %s admission",
  (kind) => {
    const f = fixture();
    if (kind === "unsupported") Object.assign(f.scope.client, { features: [] });
    if (kind === "reader") Object.assign(f.scope.client, { canEdit: false });
    const activity =
      kind === "absent"
        ? undefined
        : kind === "different-object"
          ? { ...f.activity, scope: fixture().scope }
          : f.activity;
    const { result } = renderHook(() => useLegacySessionActivity("same-id"), {
      wrapper: ({ children }) => (
        <Host {...f} activity={activity}>
          {children}
        </Host>
      ),
    });
    expect(result.current.isAvailable()).toBe(false);
    expect(result.current.mode).toBe("inactive");
    expect(result.current.liveSessionId).toBeNull();
  },
);

it("revokes escaped callbacks on session change, terminal access and teardown", () => {
  const f = fixture();
  const { result, rerender, unmount } = renderHook(
    ({ id }) => useLegacySessionActivity(id),
    {
      initialProps: { id: "same-id" },
      wrapper: ({ children }) => <Host {...f}>{children}</Host>,
    },
  );
  const first = result.current.isAvailable;
  rerender({ id: "other-id" });
  expect(first()).toBe(false);
  const second = result.current.isAvailable;
  act(() => f.state("access_denied"));
  expect(second()).toBe(false);
  expect(result.current.mode).toBe("inactive");
  unmount();
  expect(second()).toBe(false);
});

it("retires the old activity source when the same Scope receives a new capture owner", () => {
  const f = fixture();
  let activity = f.activity;
  const { result, rerender, unmount } = renderHook(
    () => useLegacySessionActivity("same-id"),
    {
      wrapper: ({ children }) => (
        <Host {...f} activity={activity}>
          {children}
        </Host>
      ),
    },
  );
  const old = result.current.isAvailable;
  activity = { ...activity, captureState: () => "unknown" };
  rerender();
  expect(old()).toBe(false);
  expect(result.current.isAvailable()).toBe(true);
  const current = result.current.isAvailable;
  unmount();
  expect(current()).toBe(false);
});

it("hides old same-ID activity when a new Scope object is supplied without a host remount", () => {
  const first = fixture(),
    second = fixture();
  let f = first;
  const { result, rerender } = renderHook(
    () => useLegacySessionActivity("same-id"),
    { wrapper: ({ children }) => <Host {...f}>{children}</Host> },
  );
  const old = result.current.isAvailable;
  f = second;
  rerender();
  expect(old()).toBe(false);
  expect(result.current.isAvailable()).toBe(false);
  expect(result.current.mode).toBe("inactive");
});

it("keeps mode and edit-lock readers out of amplitude-only recording updates", () => {
  const f = fixture();
  let renders = 0;
  const { result } = renderHook(
    () => {
      renders++;
      return useLegacySessionActivity("same-id");
    },
    { wrapper: ({ children }) => <Host {...f}>{children}</Host> },
  );
  const before = renders;
  act(() =>
    f.listener.setState((state) => ({
      live: { ...state.live, amplitude: { mic: 0.8, speaker: 0.5 } },
    })),
  );
  expect(renders).toBe(before);
  expect(result.current.mode).toBe("active");
});
