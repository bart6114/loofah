import type { QueryKey } from "@tanstack/react-query";

import { startOfDay, TZDate } from "@hypr/utils";

import { ApplicationRequestError } from "~/shared/application/errors";
import type {
  TimelineFacets,
  TimelineHeader,
  TimelinePage,
  TimelineView,
} from "~/shared/application/protocol.gen";
import type { VaultScope } from "~/shared/application/vault-scope";

export const PAGE_SIZE = 200;
export const tagBucketId = (path: string) => JSON.stringify(["tag", path]);
export const UNTAGGED_ID = JSON.stringify(["untagged"]);
export const UNDATED_ID = JSON.stringify(["undated"]);

export function timelineCutoff(now: number, timezone?: string) {
  const later = now + 48 * 60 * 60 * 1000;
  return startOfDay(
    timezone ? new TZDate(later, timezone) : new Date(later),
  ).getTime();
}

export type TimelineStream = {
  view: TimelineView;
  limit: number;
  pages: readonly TimelinePage[];
  pending: boolean;
  error: Error | null;
};
export type TimelineViewport = {
  sessionId: string;
  bucketId: string;
  offset: number;
  revision: string | null;
};

export class TimelineController {
  private snapshot;
  private readonly listeners = new Set<() => void>();
  private readonly queryKeys = new Set<QueryKey>();
  private readonly viewport = new Map<string, TimelineViewport>();
  private readonly queue: { key: string | null; run: () => Promise<void> }[] =
    [];
  private running = 0;
  private disposed = false;
  private started = false;
  private stops: (() => void)[] = [];
  private probeTimer: ReturnType<typeof setTimeout> | null = null;
  private probing = false;
  private dirty = false;
  private readonly requests = new Map<string, object>();
  private configured = false;

  constructor(
    readonly scope: VaultScope,
    readonly ownerId: string,
    now = Date.now(),
  ) {
    this.snapshot = {
      epoch: 0,
      now,
      cutoff: timelineCutoff(now),
      timezone: undefined as string | undefined,
      day: "",
      groupBy: "date" as "date" | "tag",
      expanded: [] as string[],
      allFuture: false,
      undatedOpen: false,
      revision: null as string | null,
      streams: new Map<string, TimelineStream>(),
      facets: null as TimelineFacets | null,
      facetsPending: false,
      facetsError: null as Error | null,
      notice: null as "refreshed" | "anchor_missing" | null,
    };
  }
  getSnapshot = () => this.snapshot;
  subscribe = (changed: () => void) => {
    this.listeners.add(changed);
    return () => {
      this.listeners.delete(changed);
    };
  };
  private publish(patch: Partial<typeof this.snapshot>) {
    if (this.disposed) return;
    this.snapshot = { ...this.snapshot, ...patch };
    for (const changed of this.listeners) changed();
  }
  private active(epoch = this.snapshot.epoch) {
    return (
      !this.disposed &&
      epoch === this.snapshot.epoch &&
      !["closed", "access_denied", "login_required", "incompatible"].includes(
        this.scope.getSnapshot().state,
      )
    );
  }
  start() {
    if (this.started || this.disposed) return;
    this.started = true;
    this.stops.push(
      this.scope.onChange((change) => {
        if (
          change.kind === "resync_required" ||
          change.kind === "unknown" ||
          ("entity" in change &&
            ["sessions", "session_headers", "tags"].includes(change.entity))
        )
          this.scheduleProbe();
      }),
    );
    let previous = this.scope.getSnapshot().state;
    this.stops.push(
      this.scope.subscribe(() => {
        const next = this.scope.getSnapshot().state;
        if (!this.active()) this.dispose();
        else if (next === "connected" && previous !== next) {
          this.scheduleProbe();
          if (this.snapshot.streams.has("nearest")) this.ensureNearest();
        }
        previous = next;
      }),
    );
    const focused = () => {
      this.scheduleProbe();
      if (this.snapshot.streams.has("nearest")) this.ensureNearest();
    };
    window.addEventListener("focus", focused);
    this.stops.push(() => window.removeEventListener("focus", focused));
  }
  configure(
    timezone: string | undefined,
    day: string,
    groupBy: "date" | "tag",
    initialExpanded: readonly string[],
  ) {
    if (!this.active()) return;
    if (!this.configured) {
      this.configured = true;
      this.publish({
        timezone,
        day,
        groupBy,
        cutoff: timelineCutoff(this.snapshot.now, timezone),
        expanded: initialExpanded.flatMap((path) =>
          path === "Untagged"
            ? [tagBucketId(path), UNTAGGED_ID]
            : [tagBucketId(path)],
        ),
      });
      return;
    }
    if (
      timezone !== this.snapshot.timezone ||
      (this.snapshot.day && day !== this.snapshot.day)
    ) {
      const now = Date.now();
      this.reset({ now, cutoff: timelineCutoff(now, timezone), timezone, day });
    } else if (this.snapshot.day !== day) this.publish({ day });
    if (groupBy !== this.snapshot.groupBy) this.publish({ groupBy });
  }
  toggleExpanded(id: string) {
    const expanded = this.snapshot.expanded.includes(id)
      ? this.snapshot.expanded.filter((value) => value !== id)
      : [...this.snapshot.expanded, id];
    this.publish({ expanded });
  }
  setExpanded(expanded: string[]) {
    this.publish({ expanded });
  }
  showFuture() {
    if (this.snapshot.allFuture) return;
    this.requests.delete("future");
    const streams = new Map(this.snapshot.streams);
    streams.delete("future");
    this.publish({ allFuture: true, streams });
  }
  toggleUndated() {
    this.publish({ undatedOpen: !this.snapshot.undatedOpen });
  }
  saveViewport(key: string, value: TimelineViewport) {
    if (this.active()) this.viewport.set(key, value);
  }
  readViewport(key: string) {
    return this.viewport.get(key);
  }
  viewportUnavailable() {
    this.publish({ notice: "anchor_missing" });
  }
  clearNotice() {
    this.publish({ notice: null });
  }

  private updateStream(key: string, value: TimelineStream) {
    const streams = new Map(this.snapshot.streams);
    streams.set(key, value);
    this.publish({ streams });
  }
  private enqueue(job: () => Promise<void>, key: string | null = null) {
    this.queue.push({ key, run: job });
    this.drain();
  }
  private drain() {
    while (!this.disposed && this.running < 2 && this.queue.length) {
      const job = this.queue.shift()!;
      this.running++;
      void job
        .run()
        .catch(() => {})
        .finally(() => {
          this.running--;
          this.drain();
        });
    }
  }
  private conflict(error: unknown) {
    return (
      error instanceof ApplicationRequestError &&
      error.detail.code === "conflict"
    );
  }
  private options(
    view: TimelineView,
    cursor: string | null,
    limit: number,
    revision: string | null,
  ) {
    const options = this.scope.timelineHeadersOptions({
      view,
      limit,
      cursor,
      expected_revision: revision,
    });
    const queryKey = [
      ...options.queryKey,
      "owner",
      this.ownerId,
      String(this.snapshot.epoch),
    ];
    this.queryKeys.add(queryKey);
    return { ...options, queryKey };
  }
  private acceptRevision(revision: string) {
    if (this.snapshot.revision && this.snapshot.revision !== revision) {
      this.reset();
      return false;
    }
    if (!this.snapshot.revision) {
      this.publish({ revision });
      if (this.dirty) this.scheduleProbe();
    }
    return true;
  }
  ensure(key: string, view: TimelineView, limit = PAGE_SIZE) {
    if (this.snapshot.streams.has(key)) return;
    this.load(key, view, false, limit);
  }
  ensureVisible(entries: { key: string; view: TimelineView }[]) {
    const visible = new Set(entries.map((entry) => entry.key));
    let streams = this.snapshot.streams;
    for (const [key, stream] of streams) {
      if (
        !key.startsWith("[") ||
        visible.has(key) ||
        !stream.pending ||
        stream.pages.length
      )
        continue;
      this.requests.delete(key);
      if (streams === this.snapshot.streams) streams = new Map(streams);
      streams.delete(key);
    }
    for (let index = this.queue.length - 1; index >= 0; index--) {
      const key = this.queue[index]!.key;
      if (key?.startsWith("[") && !visible.has(key))
        this.queue.splice(index, 1);
    }
    if (streams !== this.snapshot.streams) this.publish({ streams });
    for (const entry of entries) this.ensure(entry.key, entry.view);
  }
  more(key: string) {
    const stream = this.snapshot.streams.get(key);
    if (
      stream &&
      !stream.pending &&
      stream.pages[stream.pages.length - 1]?.next_cursor
    )
      this.load(key, stream.view, true, stream.limit);
  }
  retry(key: string) {
    const stream = this.snapshot.streams.get(key);
    if (stream && !stream.pending)
      this.load(key, stream.view, stream.pages.length > 0, stream.limit);
  }
  private load(
    key: string,
    view: TimelineView,
    append: boolean,
    limit = PAGE_SIZE,
  ) {
    if (!this.active() || !this.scope.client.features.includes("timeline_read"))
      return;
    const epoch = this.snapshot.epoch;
    const previous = this.snapshot.streams.get(key);
    if (previous?.pending) return;
    const pages = append ? (previous?.pages ?? []) : [];
    const cursor = append
      ? (pages[pages.length - 1]?.next_cursor ?? null)
      : null;
    const request = {};
    this.requests.set(key, request);
    const current = () =>
      this.active(epoch) && this.requests.get(key) === request;
    this.updateStream(key, { view, limit, pages, pending: true, error: null });
    this.enqueue(async () => {
      if (!current()) return;
      try {
        const page = await this.scope.queryClient.fetchQuery(
          this.options(view, cursor, limit, this.snapshot.revision),
        );
        if (!current() || !this.acceptRevision(page.revision)) return;
        this.updateStream(key, {
          view,
          limit,
          pages: [...pages, page],
          pending: false,
          error: null,
        });
      } catch (error) {
        if (!current()) return;
        if (this.conflict(error)) this.reset();
        else
          this.updateStream(key, {
            view,
            limit,
            pages,
            pending: false,
            error: error instanceof Error ? error : new Error(String(error)),
          });
      }
    }, key);
  }
  ensureFacets() {
    if (
      !this.active() ||
      this.snapshot.facets ||
      this.snapshot.facetsPending ||
      this.snapshot.facetsError ||
      !this.scope.client.features.includes("timeline_read")
    )
      return;
    const epoch = this.snapshot.epoch;
    this.publish({ facetsPending: true });
    this.enqueue(async () => {
      if (!this.active(epoch)) return;
      try {
        const options = this.scope.timelineFacetsOptions({
          expected_revision: this.snapshot.revision,
        });
        const queryKey = [
          ...options.queryKey,
          "owner",
          this.ownerId,
          String(epoch),
        ];
        this.queryKeys.add(queryKey);
        const facets = await this.scope.queryClient.fetchQuery({
          ...options,
          queryKey,
        });
        if (!this.active(epoch) || !this.acceptRevision(facets.revision))
          return;
        this.publish({ facets, facetsPending: false, facetsError: null });
      } catch (error) {
        if (!this.active(epoch)) return;
        if (this.conflict(error)) this.reset();
        else
          this.publish({
            facetsPending: false,
            facetsError:
              error instanceof Error ? error : new Error(String(error)),
          });
      }
    });
  }
  retryFacets() {
    this.publish({ facetsError: null });
    this.ensureFacets();
  }
  ensureNearest() {
    const nearest = this.snapshot.streams.get("nearest");
    if (nearest?.pending) return;
    const item = nearest?.pages[0]?.items[0];
    if (
      nearest?.pages.length &&
      (!item || (item.timestamp_ms !== null && item.timestamp_ms > Date.now()))
    )
      return;
    this.load(
      "nearest",
      {
        kind: "upcoming",
        after_ms: Date.now(),
        before_ms: this.snapshot.cutoff,
      },
      false,
      1,
    );
  }
  private scheduleProbe() {
    this.dirty = true;
    if (
      !this.active() ||
      !this.snapshot.revision ||
      this.probeTimer ||
      this.probing
    )
      return;
    this.probeTimer = setTimeout(() => {
      this.probeTimer = null;
      void this.probe();
    }, 75);
  }
  async probe() {
    if (!this.active() || !this.snapshot.revision || this.probing) return;
    this.probing = true;
    this.dirty = false;
    const epoch = this.snapshot.epoch;
    try {
      const options = this.options(
        { kind: "date", before_ms: this.snapshot.now + 1 },
        null,
        1,
        this.snapshot.revision,
      );
      const page = await this.scope.queryClient.fetchQuery({
        ...options,
        staleTime: 0,
      });
      if (this.active(epoch)) this.acceptRevision(page.revision);
    } catch (error) {
      if (this.active(epoch) && this.conflict(error)) this.reset();
    } finally {
      this.probing = false;
      if (this.dirty) this.scheduleProbe();
    }
  }
  private cancelQueries() {
    for (const queryKey of this.queryKeys) {
      void this.scope.queryClient.cancelQueries({ queryKey, exact: true });
      this.scope.queryClient.removeQueries({ queryKey, exact: true });
    }
    this.queryKeys.clear();
  }
  private reset(patch: Partial<typeof this.snapshot> = {}) {
    this.cancelQueries();
    this.queue.length = 0;
    this.requests.clear();
    this.dirty = false;
    this.publish({
      epoch: this.snapshot.epoch + 1,
      revision: null,
      streams: new Map(),
      facets: null,
      facetsPending: false,
      facetsError: null,
      notice: "refreshed",
      ...patch,
    });
  }
  dispose() {
    if (this.disposed) return;
    this.cancelQueries();
    if (this.probeTimer) clearTimeout(this.probeTimer);
    for (const stop of this.stops.splice(0)) stop();
    this.queue.length = 0;
    this.viewport.clear();
    this.requests.clear();
    this.publish({ streams: new Map(), facets: null, revision: null });
    this.disposed = true;
    for (const changed of this.listeners) changed();
    this.listeners.clear();
  }
}

export function streamItems(
  stream: TimelineStream | undefined,
): readonly TimelineHeader[] {
  return stream?.pages.flatMap((page) => page.items) ?? [];
}
