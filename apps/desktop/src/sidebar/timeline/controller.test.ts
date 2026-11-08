import { onlineManager } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TimelineController, tagBucketId } from "./controller";
import { deferred, header, page, timelineFixture } from "./test-fixture";

import { ApplicationRequestError } from "~/shared/application/errors";
import type { TimelinePage } from "~/shared/application/protocol.gen";

const cleanup: (() => void)[] = [];
afterEach(() => {
  for (const stop of cleanup.splice(0)) stop();
  onlineManager.setOnline(true);
  vi.useRealTimers();
});
async function fixture(mode: "always" | "online" = "always") {
  const f = timelineFixture(mode);
  await f.start();
  const owner = new TimelineController(f.scope, "owner", 1000);
  owner.start();
  cleanup.push(() => {
    owner.dispose();
    f.scope.dispose();
  });
  return { ...f, owner };
}
async function flush() {
  for (let i = 0; i < 12; i++) await Promise.resolve();
}
const conflict = () =>
  new ApplicationRequestError({
    code: "conflict",
    message: "changed",
    retryable: false,
    request_id: null,
    current_revision: "r2",
    mutation_outcome: "not_committed",
  });

describe("bounded scoped timeline", () => {
  it("constructs without reads or subscriptions", () => {
    const f = timelineFixture();
    const c = new TimelineController(f.scope, "owner");
    expect(f.client.subscribe).not.toHaveBeenCalled();
    expect(f.client.timelineHeaders).not.toHaveBeenCalled();
    c.dispose();
    f.scope.dispose();
  });
  it("loads only one 200-row page per explicit intent in a 20,000-header vault", async () => {
    const { owner, client } = await fixture();
    client.timelineHeaders.mockImplementation(async (input) => {
      const start = Number(input.cursor ?? 0);
      return page(
        Array.from({ length: 200 }, (_, i) => header(String(start + i))),
        { next_cursor: String(start + 200), total_count: 20000 },
      );
    });
    owner.ensure("past", { kind: "date", before_ms: 1001 });
    await flush();
    expect(client.timelineHeaders).toHaveBeenCalledTimes(1);
    expect(
      owner.getSnapshot().streams.get("past")?.pages[0].items,
    ).toHaveLength(200);
    owner.more("past");
    await flush();
    expect(client.timelineHeaders).toHaveBeenCalledTimes(2);
    expect(client.timelineHeaders.mock.calls[1][0]).toMatchObject({
      cursor: "200",
      expected_revision: "r1",
      limit: 200,
    });
  });
  it.each([50, 150, 500])(
    "bounds in-flight expanded direct buckets at realistic %ims latency",
    async (latency) => {
      vi.useFakeTimers();
      const { owner, client } = await fixture();
      let active = 0,
        max = 0;
      client.timelineHeaders.mockImplementation(async () => {
        active++;
        max = Math.max(max, active);
        await new Promise((done) => setTimeout(done, latency));
        active--;
        return page([header("x")]);
      });
      for (let i = 0; i < 30; i++)
        owner.ensure(tagBucketId(String(i)), { kind: "tag", path: String(i) });
      expect(client.timelineHeaders).toHaveBeenCalledTimes(2);
      expect(max).toBe(2);
      await vi.advanceTimersByTimeAsync(latency);
      expect(client.timelineHeaders).toHaveBeenCalledTimes(4);
      expect(max).toBe(2);
      owner.dispose();
      await vi.runAllTimersAsync();
      expect(client.timelineHeaders).toHaveBeenCalledTimes(4);
    },
  );
  it("keeps local requests offline and pauses remote reads until online", async () => {
    onlineManager.setOnline(false);
    const local = await fixture("always"),
      remote = await fixture("online");
    local.owner.ensure("past", { kind: "date", before_ms: 1001 });
    remote.owner.ensure("past", { kind: "date", before_ms: 1001 });
    await flush();
    expect(local.client.timelineHeaders).toHaveBeenCalledOnce();
    expect(remote.client.timelineHeaders).not.toHaveBeenCalled();
    onlineManager.setOnline(true);
    await flush();
    expect(remote.client.timelineHeaders).toHaveBeenCalledOnce();
  });
  it("cannot dispatch a paused request after retirement", async () => {
    onlineManager.setOnline(false);
    const { owner, client } = await fixture("online");
    owner.ensure("past", { kind: "date", before_ms: 1001 });
    await flush();
    owner.dispose();
    onlineManager.setOnline(true);
    await flush();
    expect(client.timelineHeaders).not.toHaveBeenCalled();
  });
  it.each(["access_denied", "login_required", "closed"] as const)(
    "hides loaded content and rejects late replies on %s",
    async (state) => {
      const { owner, client, emit } = await fixture();
      const pending = deferred<TimelinePage>();
      client.timelineHeaders.mockReturnValueOnce(pending.promise);
      owner.ensure("past", { kind: "date", before_ms: 1001 });
      emit({ kind: "connection", state, error: null });
      pending.resolve(page([header("private")]));
      await flush();
      expect(owner.getSnapshot().streams.size).toBe(0);
      expect(owner.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
    },
  );
  it("does not admit same-ID replies into a replacement Scope with the same scope string", async () => {
    const old = await fixture(),
      next = await fixture();
    const pending = deferred<TimelinePage>();
    old.client.timelineHeaders.mockReturnValueOnce(pending.promise);
    next.client.timelineHeaders.mockResolvedValue(
      page([header("same-id")], { revision: "new" }),
    );
    old.owner.ensure("past", { kind: "date", before_ms: 1001 });
    old.owner.dispose();
    next.owner.ensure("past", { kind: "date", before_ms: 1001 });
    pending.resolve(page([header("same-id")], { revision: "old" }));
    await flush();
    expect(next.owner.getSnapshot().revision).toBe("new");
    expect(old.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  });
  it("coalesces no-op change probes without dropping loaded pages or viewport", async () => {
    vi.useFakeTimers();
    const { owner, client, emit } = await fixture();
    owner.ensure("past", { kind: "date", before_ms: 1001 });
    await flush();
    const original = owner.getSnapshot().streams;
    owner.saveViewport("date", {
      sessionId: "x",
      bucketId: "Today",
      offset: 12,
      revision: "r1",
    });
    for (let i = 0; i < 20; i++)
      emit({
        kind: "change",
        change: {
          kind: "index_changed",
          entity: "session_headers",
          ids: ["x"],
        },
      });
    await vi.advanceTimersByTimeAsync(75);
    expect(client.timelineHeaders).toHaveBeenCalledTimes(2);
    expect(owner.getSnapshot().streams).toBe(original);
    expect(owner.readViewport("date")?.offset).toBe(12);
  });
  it("probes a change received during the first unversioned reply", async () => {
    vi.useFakeTimers();
    const { owner, client, emit } = await fixture();
    const pending = deferred<TimelinePage>();
    client.timelineHeaders
      .mockReturnValueOnce(pending.promise)
      .mockRejectedValueOnce(conflict());
    owner.ensure("past", { kind: "date", before_ms: 1001 });
    emit({
      kind: "change",
      change: { kind: "index_changed", entity: "session_headers", ids: ["x"] },
    });
    pending.resolve(page());
    await flush();
    await vi.advanceTimersByTimeAsync(75);
    expect(owner.getSnapshot().revision).toBeNull();
    expect(owner.getSnapshot().epoch).toBe(1);
    expect(owner.getSnapshot().streams.size).toBe(0);
  });
  it("resets all cursors on a facet/page revision mismatch and retains only the viewport hint", async () => {
    const { owner, client } = await fixture();
    owner.ensure("past", { kind: "date", before_ms: 1001 });
    await flush();
    owner.saveViewport("date", {
      sessionId: "gone",
      bucketId: "Today",
      offset: 10,
      revision: "r1",
    });
    client.timelineFacets.mockResolvedValue({
      revision: "r2",
      nodes: [],
      untagged_count: 0,
    });
    owner.ensureFacets();
    await flush();
    expect(owner.getSnapshot().streams.size).toBe(0);
    expect(owner.getSnapshot().notice).toBe("refreshed");
    expect(owner.readViewport("date")?.sessionId).toBe("gone");
  });
  it("shows errors without allocating retries until explicitly requested", async () => {
    const { owner, client } = await fixture();
    client.timelineHeaders.mockRejectedValueOnce(new Error("disconnected"));
    owner.ensure(
      "nearest",
      { kind: "upcoming", after_ms: 1000, before_ms: null },
      1,
    );
    await flush();
    expect(owner.getSnapshot().streams.get("nearest")?.error?.message).toBe(
      "disconnected",
    );
    owner.retry("nearest");
    await flush();
    expect(client.timelineHeaders.mock.calls[1][0].limit).toBe(1);
  });
  it("does not replace the full-future stream with an older cutoff reply", async () => {
    const { owner, client } = await fixture();
    const pending = deferred<TimelinePage>();
    client.timelineHeaders.mockReturnValueOnce(pending.promise);
    owner.ensure("future", {
      kind: "upcoming",
      after_ms: 1000,
      before_ms: 2000,
    });
    owner.showFuture();
    owner.ensure("future", {
      kind: "upcoming",
      after_ms: 1000,
      before_ms: null,
    });
    await flush();
    pending.resolve(page([header("old")]));
    await flush();
    expect(owner.getSnapshot().streams.get("future")?.view).toEqual({
      kind: "upcoming",
      after_ms: 1000,
      before_ms: null,
    });
    expect(owner.getSnapshot().streams.get("future")?.pages[0].items).toEqual(
      [],
    );
  });
});

it("drops an offscreen queued tag backlog while retaining at most admitted work", async () => {
  const { owner, client } = await fixture();
  const pending = deferred<TimelinePage>();
  client.timelineHeaders.mockReturnValue(pending.promise);
  owner.ensureVisible(
    Array.from({ length: 40 }, (_, i) => ({
      key: tagBucketId(String(i)),
      view: { kind: "tag" as const, path: String(i) },
    })),
  );
  expect(client.timelineHeaders).toHaveBeenCalledTimes(2);
  owner.ensureVisible([
    { key: tagBucketId("last"), view: { kind: "tag", path: "last" } },
  ]);
  pending.resolve(page());
  await flush();
  expect(client.timelineHeaders).toHaveBeenCalledTimes(3);
  expect(client.timelineHeaders.mock.calls[2][0].view).toEqual({
    kind: "tag",
    path: "last",
  });
  expect(owner.getSnapshot().streams.size).toBe(1);
});
