import { listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";

import {
  type AppConfig,
  commands as settingsCommands,
  type JsonValue,
} from "@hypr/plugin-settings";

import {
  SETTING_DEFINITIONS,
  type SettingKey,
  type SettingValues,
} from "~/settings/schema";

export type StoredSettingValues = {
  values: SettingValues;
  hasValues: Set<SettingKey>;
};

export type ConfigSnapshot = {
  config: AppConfig;
  settings: StoredSettingValues;
};

export type ConfigStoreState = {
  snapshot: ConfigSnapshot | undefined;
  isLoading: boolean;
  error: Error | null;
};

const CONFIG_CHANGED_EVENT = "config-changed";

// These keys are real JSON arrays in config.json, but the TypeScript setting
// schema (and every consumer of it) still speaks JSON-encoded strings.
export const ARRAY_SETTING_KEYS = new Set<SettingKey>([
  "spoken_languages",
  "meeting_languages",
  "ignored_platforms",
  "included_platforms",
]);

let state: ConfigStoreState = {
  snapshot: undefined,
  isLoading: true,
  error: null,
};
const subscribers = new Set<() => void>();
let initPromise: Promise<void> | null = null;
let eventVersion = 0;
let readSequence = 0;
let appliedReadSequence = 0;

function setState(next: ConfigStoreState): void {
  state = next;
  for (const subscriber of subscribers) subscriber();
}

export function applyConfigSnapshot(config: AppConfig): void {
  if (
    state.snapshot &&
    !state.error &&
    configsEqual(state.snapshot.config, config)
  ) {
    return;
  }
  setState({
    snapshot: { config, settings: toStoredSettingValues(config) },
    isLoading: false,
    error: null,
  });
}

function configsEqual(previous: AppConfig, next: AppConfig): boolean {
  const keys = Object.keys(previous) as (keyof AppConfig)[];
  return (
    keys.length === Object.keys(next).length &&
    keys.every(
      (key) =>
        Object.is(previous[key], next[key]) ||
        JSON.stringify(previous[key]) === JSON.stringify(next[key]),
    )
  );
}

export const EMPTY_STORED_SETTINGS: StoredSettingValues = {
  values: {},
  hasValues: new Set(),
};

export function useConfigSelector<T>(
  selector: (settings: StoredSettingValues) => T,
): T {
  return useSyncExternalStore(subscribe, () =>
    selector(state.snapshot?.settings ?? EMPTY_STORED_SETTINGS),
  );
}

export function initConfigStore(): Promise<void> {
  initPromise ??= startConfigStore();
  return initPromise;
}

async function startConfigStore(): Promise<void> {
  const version = eventVersion;
  try {
    // Subscribe first, and never let a delayed read replace a newer event.
    await listen<AppConfig>(CONFIG_CHANGED_EVENT, (event) => {
      eventVersion++;
      applyConfigSnapshot(event.payload);
    });
    await fetchConfig();
  } catch (error) {
    if (state.snapshot && version !== eventVersion) return;
    setState({
      snapshot: undefined,
      isLoading: false,
      error: error instanceof Error ? error : new Error(String(error)),
    });
  }
}

function subscribe(subscriber: () => void): () => void {
  subscribers.add(subscriber);
  void initConfigStore();
  return () => {
    subscribers.delete(subscriber);
  };
}

export function useConfigStateSelector<T>(
  selector: (state: ConfigStoreState) => T,
): T {
  return useSyncExternalStore(subscribe, () => selector(state));
}

export function useConfigStoreState(): ConfigStoreState {
  return useConfigStateSelector((state) => state);
}

export async function fetchConfig(): Promise<AppConfig> {
  const version = eventVersion;
  const sequence = ++readSequence;
  let result: Awaited<ReturnType<typeof settingsCommands.getConfig>>;
  try {
    result = await settingsCommands.getConfig();
  } catch (error) {
    const newer = newerConfigSnapshot(version, sequence);
    if (newer) return newer;
    throw error;
  }
  const newer = newerConfigSnapshot(version, sequence);
  if (newer) return newer;
  if (result.status === "error") throw new Error(result.error);
  appliedReadSequence = sequence;
  applyConfigSnapshot(result.data);
  return result.data;
}

function newerConfigSnapshot(version: number, sequence: number) {
  return version !== eventVersion || sequence < appliedReadSequence
    ? state.snapshot?.config
    : undefined;
}

export async function fetchStoredSettingValues(): Promise<StoredSettingValues> {
  return toStoredSettingValues(await fetchConfig());
}

export async function writeConfigValues(
  values: Partial<{ [key in string]: JsonValue }>,
): Promise<void> {
  const result = await settingsCommands.setConfigValues(values);
  if (result.status === "error") throw new Error(result.error);
  // The single native publisher confirms writes. A delayed command reply must
  // not merge older values over a snapshot already received from that publisher.
}

export function resetConfigStoreForTests(): void {
  eventVersion++;
  initPromise = null;
  state = { snapshot: undefined, isLoading: true, error: null };
}

// `hasValues` marks keys the user explicitly configured. config.json always
// materializes every key after the first write, so "explicitly configured"
// is approximated as "differs from the schema default" (optional keys count
// as configured whenever present).
function toStoredSettingValues(config: AppConfig): StoredSettingValues {
  const values: SettingValues = {};
  const hasValues = new Set<SettingKey>();
  const record = config as Record<string, unknown>;

  for (const key of Object.keys(SETTING_DEFINITIONS) as SettingKey[]) {
    const definition = SETTING_DEFINITIONS[key];
    let value = record[key];
    if (ARRAY_SETTING_KEYS.has(key)) {
      value = Array.isArray(value)
        ? JSON.stringify(
            value.filter((entry): entry is string => typeof entry === "string"),
          )
        : undefined;
    }
    if (value === undefined || value === null) continue;
    if (typeof value !== definition.type) continue;

    const defaultValue =
      "default" in definition ? definition.default : undefined;
    if (value === defaultValue) continue;

    (values as Record<string, unknown>)[key] = value;
    hasValues.add(key);
  }

  return { values, hasValues };
}
