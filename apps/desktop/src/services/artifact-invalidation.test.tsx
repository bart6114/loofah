import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const listen = vi.hoisted(() => vi.fn(() => Promise.resolve(() => {})));

vi.mock("~/types/tauri.gen", () => ({
  events: { indexChanged: { listen } },
}));
vi.mock("@hypr/plugin-fs-sync", () => ({ commands: {} }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (p: string) => p }));

import { ArtifactInvalidationSync } from "./artifact-invalidation";

type IndexChangedHandler = (event: {
  payload: { entity: string; ids: string[] };
}) => void;

function mountWithSeededCache() {
  const queryClient = new QueryClient();
  queryClient.setQueryData(["audio", "s1", "url"], "file:///old/audio.mp3");
  queryClient.setQueryData(["audio", "s1", "peaks"], [0.1]);
  queryClient.setQueryData(["audio", "s1", "exist"], true);
  queryClient.setQueryData(["session", "s1", "attachment-paths"], new Map());
  queryClient.setQueryData(["audio", "s2", "url"], "file:///other/audio.mp3");
  queryClient.setQueryData(["session", "s1", "unrelated"], "keep");

  render(
    <QueryClientProvider client={queryClient}>
      <ArtifactInvalidationSync />
    </QueryClientProvider>,
  );
  const calls = listen.mock.calls as unknown[][];
  const handler = calls[calls.length - 1][0] as IndexChangedHandler;
  return { queryClient, handler };
}

afterEach(() => {
  cleanup();
});

it("invalidates exactly the affected session's path-backed queries on a artifact event", () => {
  const { queryClient, handler } = mountWithSeededCache();

  act(() => handler({ payload: { entity: "artifacts", ids: ["s1"] } }));

  for (const key of [
    ["audio", "s1", "url"],
    ["audio", "s1", "peaks"],
    ["audio", "s1", "exist"],
    ["session", "s1", "attachment-paths"],
  ]) {
    expect(queryClient.getQueryState(key)?.isInvalidated, key.join("/")).toBe(
      true,
    );
  }
  expect(queryClient.getQueryState(["audio", "s2", "url"])?.isInvalidated).toBe(
    false,
  );
  expect(
    queryClient.getQueryState(["session", "s1", "unrelated"])?.isInvalidated,
  ).toBe(false);
});

it("ignores unrelated index events", () => {
  const { queryClient, handler } = mountWithSeededCache();

  act(() => handler({ payload: { entity: "tags", ids: ["s1"] } }));

  expect(queryClient.getQueryState(["audio", "s1", "url"])?.isInvalidated).toBe(
    false,
  );
});

it("invalidates deletion and restoration in every mounted window cache", () => {
  const main = mountWithSeededCache();
  const standalone = mountWithSeededCache();

  for (const lifecycle of ["deleted", "restored"]) {
    act(() => main.handler({ payload: { entity: "sessions", ids: ["s1"] } }));
    for (const { queryClient } of [main, standalone]) {
      for (const key of [
        ["audio", "s1", "url"],
        ["session", "s1", "attachment-paths"],
      ]) {
        expect(queryClient.getQueryState(key)?.isInvalidated, lifecycle).toBe(
          true,
        );
        queryClient.setQueryData(key, lifecycle);
      }
    }
  }
});
