import type { TabsDeviceState } from "./device-state";
import {
  deserializePinnedTabs,
  getPinnedTabIds,
  serializePinnedTabs,
  type PinnedTab,
} from "./pinned-persistence";
import { getDefaultState, isSameTab, uniqueIdfromTab } from "./schema";
import { createTabsStore } from "./store";

import type { VaultScope } from "~/shared/application/vault-scope";

const MAX_PINNED_TABS = 128;
const MAX_PINNED_BYTES = 256 * 1024;
const terminalStates = new Set([
  "access_denied",
  "login_required",
  "closed",
  "incompatible",
]);

export class ScopedTabsController {
  readonly store;
  private disposed = false;
  private applying = false;
  private recentVersion = 0;
  private pinnedVersion = 0;
  private tail: Promise<void> = Promise.resolve();
  private starting: Promise<void> | null = null;
  private refreshing: Promise<void> | null = null;
  private stopStore: (() => void) | null = null;
  private stopScope: (() => void) | null = null;
  private stopDevice: (() => void) | null = null;
  private pinAbort: AbortController | null = null;
  private readVersion = 0;
  private pinsToSave: { value: string; version: number } | null = null;
  private savingPins = false;
  private pinnedBaseline: PinnedTab[] | null = null;
  private readonly pendingPinChanges = new Map<string, PinnedTab | null>();
  private confirmedPinnedVersion = 0;

  constructor(
    readonly scope: VaultScope,
    private readonly deviceState: TabsDeviceState,
    private readonly managePins: boolean,
    private readonly onError: (error: unknown) => void = console.error,
  ) {
    this.store = createTabsStore(scope.client.scope, () => this.active());
    this.stopStore = this.store.subscribe((next, previous) => {
      if (this.applying || !this.active()) return;
      if (next.recentlyOpenedSessionIds !== previous.recentlyOpenedSessionIds) {
        const id = next.recentlyOpenedSessionIds[0];
        const version = ++this.recentVersion;
        if (id)
          void this.enqueue(async () => {
            const ids = await this.deviceState.rememberSession(
              this.store.scope,
              id,
            );
            if (this.active() && version === this.recentVersion)
              this.apply(() =>
                this.store.setState({
                  recentlyOpenedSessionIds: [...new Set(ids)].slice(0, 10),
                }),
              );
          }).catch(this.onError);
      }
      if (this.managePins && next.tabs !== previous.tabs) {
        const before = getPinnedTabIds(previous.tabs);
        const after = getPinnedTabIds(next.tabs);
        if (
          after.length !== before.length ||
          after.some((id, index) => id !== before[index])
        ) {
          ++this.pinnedVersion;
          this.pinAbort?.abort();
          const beforeSet = new Set(before);
          const afterSet = new Set(after);
          for (const key of before)
            if (!afterSet.has(key)) this.pendingPinChanges.set(key, null);
          for (const pin of deserializePinnedTabs(
            serializePinnedTabs(next.tabs),
          )) {
            const key = uniqueIdfromTab(getDefaultState(pin));
            if (!beforeSet.has(key)) this.pendingPinChanges.set(key, pin);
          }
          if (this.pinnedBaseline) {
            this.pinnedBaseline = this.mergePendingPins(this.pinnedBaseline);
            this.savePins();
          }
        }
      }
    });
  }

  private active() {
    return (
      !this.disposed && !terminalStates.has(this.scope.getSnapshot().state)
    );
  }

  private mergePendingPins(baseline: PinnedTab[]) {
    const pins = new Map(
      baseline.map((pin) => [uniqueIdfromTab(getDefaultState(pin)), pin]),
    );
    for (const [key, pin] of this.pendingPinChanges) {
      if (pin) pins.set(key, pin);
      else pins.delete(key);
    }
    this.pendingPinChanges.clear();
    return [...pins.values()];
  }

  private savePins() {
    if (!this.pinnedBaseline) return;
    this.pinsToSave = {
      value: JSON.stringify(this.pinnedBaseline),
      version: this.pinnedVersion,
    };
    if (this.savingPins) return;
    this.savingPins = true;
    void this.enqueue(async () => {
      const saved = this.pinsToSave;
      this.pinsToSave = null;
      this.savingPins = false;
      if (!saved) return;
      if (
        new TextEncoder().encode(saved.value).length > MAX_PINNED_BYTES ||
        JSON.parse(saved.value).length > MAX_PINNED_TABS
      )
        throw new Error("Pinned tab state exceeds device storage limits");
      await this.deviceState.setPinnedTabs(this.store.scope, saved.value);
      if (this.active())
        this.confirmedPinnedVersion = Math.max(
          this.confirmedPinnedVersion,
          saved.version,
        );
    }).catch(this.onError);
  }

  private enqueue<T>(operation: () => Promise<T>) {
    const next = this.tail.then(async () => {
      if (this.active()) return operation();
    });
    this.tail = next.then(
      () => {},
      () => {},
    );
    return next;
  }

  private apply(operation: () => void) {
    this.applying = true;
    try {
      operation();
    } finally {
      this.applying = false;
    }
  }

  start() {
    if (!this.starting) {
      const pending = this.initialize();
      this.starting = pending;
      void pending.catch(() => {
        if (this.starting === pending) this.starting = null;
      });
    }
    return this.starting;
  }

  private async initialize() {
    if (!this.active()) {
      this.dispose();
      return;
    }
    if (!this.stopScope) {
      let previous = this.scope.getSnapshot().state;
      this.stopScope = this.scope.subscribe(() => {
        const next = this.scope.getSnapshot().state;
        if (!this.active()) this.dispose();
        else if (next === "connected" && previous !== "connected")
          void this.refresh().catch(this.onError);
        previous = next;
      });
    }
    if (!this.stopDevice) {
      const stop = await this.deviceState.subscribe(this.store.scope, () => {
        void this.refresh().catch(this.onError);
      });
      if (!this.active()) {
        stop();
        return;
      }
      this.stopDevice = stop;
    }
    await this.refresh();
  }

  refresh() {
    if (this.refreshing) return this.refreshing;
    const readVersion = ++this.readVersion;
    // Preserve the read's age while it waits behind native writes.
    const recentVersion = this.recentVersion;
    const pinnedVersion = this.pinnedVersion;
    this.pinAbort?.abort();
    const read = this.enqueue(async () => {
      this.refreshing = null;
      const [recent, serialized] = await Promise.all([
        this.deviceState.recentSessions(this.store.scope),
        this.managePins
          ? this.deviceState.pinnedTabs(this.store.scope)
          : Promise.resolve(null),
      ]);
      if (!this.active() || readVersion !== this.readVersion) return null;
      if (recentVersion === this.recentVersion)
        this.apply(() =>
          this.store.setState({
            recentlyOpenedSessionIds: [...new Set(recent)].slice(0, 10),
          }),
        );
      if (!this.managePins) return null;
      const pins =
        serialized &&
        new TextEncoder().encode(serialized).length <= MAX_PINNED_BYTES
          ? deserializePinnedTabs(serialized)
              .filter(
                (tab) =>
                  tab.type !== "sessions" ||
                  (typeof tab.id === "string" && tab.id.length > 0),
              )
              .slice(0, MAX_PINNED_TABS)
          : [];
      if (!this.pinnedBaseline) {
        // A whole-list write must retain pins not yet shown by this host.
        this.pinnedBaseline = this.mergePendingPins(pins);
        if (this.pinnedVersion > 0) this.savePins();
        return { pins: this.pinnedBaseline, pinnedVersion: this.pinnedVersion };
      }
      if (
        pinnedVersion !== this.pinnedVersion ||
        this.confirmedPinnedVersion !== this.pinnedVersion
      )
        return null;
      this.pinnedBaseline = pins;
      return { pins, pinnedVersion };
    });
    // Remote metadata must not hold the device persistence queue during a disconnect.
    const pending = read.then(async (snapshot) => {
      if (!snapshot || !this.managePins) return;
      const current = () =>
        this.active() &&
        readVersion === this.readVersion &&
        snapshot.pinnedVersion === this.pinnedVersion;
      if (!current()) return;
      const { pins } = snapshot;
      const abort = new AbortController();
      this.pinAbort = abort;
      const ids = [
        ...new Set(
          pins.flatMap((tab) => (tab.type === "sessions" ? [tab.id] : [])),
        ),
      ];
      const existing = new Set<string>();
      for (let index = 0; index < ids.length; index += 100) {
        try {
          const rows = await this.scope.run(() =>
            this.scope.client.sessionSummaries(
              { kind: "ids", ids: ids.slice(index, index + 100) },
              { signal: abort.signal },
            ),
          );
          if (!current()) return;
          for (const row of rows) existing.add(row.id);
        } catch (error) {
          if (!current() || abort.signal.aborted) return;
          throw error;
        }
      }
      if (!current()) return;
      this.apply(() => {
        const before = this.store.getState();
        const selection = before.currentTab;
        const allowed = pins.filter(
          (tab) => tab.type !== "sessions" || existing.has(tab.id),
        );
        const wanted = new Set(
          allowed.map((tab) => uniqueIdfromTab(getDefaultState(tab))),
        );
        for (const tab of before.tabs)
          if (tab.pinned && !wanted.has(uniqueIdfromTab(tab)))
            this.store.getState().unpin(tab);
        for (const pin of allowed) {
          const target = getDefaultState(pin);
          if (!this.store.getState().tabs.some((tab) => isSameTab(tab, target)))
            this.store.getState().openNew(pin);
          const tab = this.store
            .getState()
            .tabs.find((tab) => isSameTab(tab, target));
          if (tab && !tab.pinned) this.store.getState().pin(tab);
        }
        if (selection) {
          const tab = this.store
            .getState()
            .tabs.find((tab) => isSameTab(tab, selection));
          if (tab) this.store.getState().select(tab);
        }
        this.store.setState({
          recentlyOpenedSessionIds: before.recentlyOpenedSessionIds,
        });
      });
    });
    this.refreshing = pending;
    void pending
      .finally(() => {
        if (this.refreshing === pending) this.refreshing = null;
      })
      .catch(() => {});
    return pending;
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.stopScope?.();
    this.stopStore?.();
    this.stopDevice?.();
    this.pinAbort?.abort();
    this.stopScope = null;
    this.stopStore = null;
    this.stopDevice = null;
    this.pinsToSave = null;
    this.pinnedBaseline = null;
    this.pendingPinChanges.clear();
    this.starting = null;
    this.refreshing = null;
    this.tail = Promise.resolve();
    this.store.clear();
  }
}
