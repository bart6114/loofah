import { vi } from "vitest";

import type { TabsDeviceState } from "./device-state";

import type { SessionSummaryQuery } from "~/shared/application/protocol.gen";
import type { VaultScope } from "~/shared/application/vault-scope";

export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

export function scopedFixture(id: string) {
  let snapshot = { state: "connected", error: null };
  const listeners = new Set<() => void>();
  const summaries = vi.fn(
    async (input: SessionSummaryQuery, _options?: { signal?: AbortSignal }) =>
      input.kind === "ids"
        ? input.ids
            .filter((id) => id !== "missing")
            .map((id) => ({ id, title: id, created_at: "2026-01-01" }))
        : [],
  );
  const scope = {
    client: {
      scope: id,
      features: ["session_summaries", "search"],
      canEdit: true,
      sessionSummaries: summaries,
    },
    getSnapshot: () => snapshot,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    run: <T>(operation: () => Promise<T>) => operation(),
  } as unknown as VaultScope;
  return {
    scope,
    summaries,
    listeners,
    state: (state: string) => {
      snapshot = { state, error: null };
      for (const listener of listeners) listener();
    },
  };
}

export function deviceFixture() {
  const recents = new Map<string, string[]>();
  const pins = new Map<string, string>();
  const listeners = new Map<string, Set<() => void>>();
  const stop = vi.fn();
  const device = {
    recentSessions: vi.fn(async (scope: string) => recents.get(scope) ?? []),
    rememberSession: vi.fn(async (scope: string, id: string) => {
      const ids = [
        id,
        ...(recents.get(scope) ?? []).filter((entry) => entry !== id),
      ].slice(0, 10);
      recents.set(scope, ids);
      return ids;
    }),
    pinnedTabs: vi.fn(async (scope: string) => pins.get(scope) ?? null),
    setPinnedTabs: vi.fn(async (scope: string, value: string) => {
      pins.set(scope, value);
    }),
    subscribe: vi.fn(async (scope: string, listener: () => void) => {
      const entries = listeners.get(scope) ?? new Set<() => void>();
      entries.add(listener);
      listeners.set(scope, entries);
      return () => {
        entries.delete(listener);
        stop(scope);
      };
    }),
  } satisfies TabsDeviceState;
  return {
    device,
    recents,
    pins,
    stop,
    emit: (scope: string) => {
      for (const listener of listeners.get(scope) ?? []) listener();
    },
  };
}
