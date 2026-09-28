import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { expect, it, vi } from "vitest";

import { useRefreshSessionNote, useRefreshEnhancedNote } from "./queries";

it("confirms a post-save snapshot without allowing an older query to overwrite it", async () => {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const queryKey = ["session", "saved-note"];
  const record = (title: string) => ({
    title,
    raw_md: JSON.stringify({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: title }],
        },
      ],
    }),
  });
  let finishOldRead!: (value: ReturnType<typeof record>) => void;
  const read = vi
    .fn()
    .mockResolvedValueOnce(record("Original"))
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishOldRead = resolve;
        }),
    )
    .mockResolvedValueOnce(record("Newer external title"));
  await client.fetchQuery({ queryKey, queryFn: read });
  const oldRead = client
    .fetchQuery({ queryKey, queryFn: read, staleTime: 0 })
    .catch(() => undefined);
  const rendered = renderHook(() => useRefreshSessionNote("saved-note"), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  });
  let confirmed!: Awaited<ReturnType<typeof rendered.result.current>>;
  await act(async () => {
    confirmed = await rendered.result.current();
  });
  expect(confirmed()).toMatchObject({
    content: [{ content: [{ text: "Newer external title" }] }],
  });
  finishOldRead(record("Outdated"));
  await oldRead;
  expect(client.getQueryData(queryKey)).toEqual(record("Newer external title"));
  client.setQueryData(queryKey, record("Changed again before delivery"));
  expect(confirmed()).toMatchObject({
    content: [{ content: [{ text: "Changed again before delivery" }] }],
  });
  rendered.unmount();
  client.clear();
});

it("returns a live summary reader using the latest content and session title", async () => {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const sessionKey = ["session", "summary-session"];
  const noteKey = ["enhanced-doc", "summary-note"];
  const content = (text: string) =>
    JSON.stringify({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text }] }],
    });
  await client.fetchQuery({
    queryKey: sessionKey,
    queryFn: async () => ({ title: "Saved title" }),
  });
  await client.fetchQuery({
    queryKey: noteKey,
    queryFn: async () => ({ content: content("Saved body") }),
  });
  const rendered = renderHook(
    () => useRefreshEnhancedNote("summary-note", "summary-session"),
    {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  const readContent = await rendered.result.current();
  client.setQueryData(sessionKey, { title: "Latest title" });
  client.setQueryData(noteKey, { content: content("Latest body") });
  expect(readContent()).toMatchObject({
    content: [
      { type: "heading", content: [{ text: "Latest title" }] },
      { type: "paragraph", content: [{ text: "Latest body" }] },
    ],
  });
  rendered.unmount();
  client.clear();
});

it("refreshes and follows the exact active generation when older summary caches coexist", async () => {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const sessionKey = ["session", "generation-session"];
  const baseKey = ["enhanced-doc", "generation-note"];
  const oldKey = [...baseKey, "old-generation"];
  const activeKey = [...baseKey, "active-generation"];
  const content = (text: string) =>
    JSON.stringify({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text }] }],
    });
  const baseRead = vi.fn(async () => ({ content: content("Old base") }));
  const oldRead = vi.fn(async () => ({ content: content("Old generation") }));
  const activeRead = vi.fn(async () => ({
    content: content("Active generation"),
  }));
  await client.fetchQuery({
    queryKey: sessionKey,
    queryFn: async () => ({ title: "Title" }),
  });
  await client.fetchQuery({ queryKey: baseKey, queryFn: baseRead });
  await client.fetchQuery({ queryKey: oldKey, queryFn: oldRead });
  await client.fetchQuery({ queryKey: activeKey, queryFn: activeRead });
  const rendered = renderHook(
    () =>
      useRefreshEnhancedNote(
        "generation-note",
        "generation-session",
        "active-generation",
      ),
    {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  const readContent = await rendered.result.current();
  expect(baseRead).toHaveBeenCalledOnce();
  expect(oldRead).toHaveBeenCalledOnce();
  expect(activeRead).toHaveBeenCalledTimes(2);
  expect(readContent()).toMatchObject({
    content: [
      { type: "heading" },
      { content: [{ text: "Active generation" }] },
    ],
  });
  client.setQueryData(activeKey, { content: content("Later active edit") });
  client.setQueryData(baseKey, { content: content("Unrelated stale cache") });
  expect(readContent()).toMatchObject({
    content: [
      { type: "heading" },
      { content: [{ text: "Later active edit" }] },
    ],
  });
  rendered.unmount();
  client.clear();
});
