import { disable, enable } from "@tauri-apps/plugin-autostart";
import { useCallback } from "react";
import { useShallow } from "zustand/react/shallow";

import { commands as detectCommands } from "@hypr/plugin-detect";
import { commands as localSttCommands } from "@hypr/plugin-local-stt";
import type { JsonValue } from "@hypr/plugin-settings";
import { commands as trayCommands } from "@hypr/plugin-tray";
import { commands as windowsCommands } from "@hypr/plugin-windows";

import {
  SETTING_DEFINITIONS,
  type SettingKey,
  type SettingValue,
  type SettingValues,
} from "~/settings/schema";
import {
  ARRAY_SETTING_KEYS,
  fetchStoredSettingValues,
  type StoredSettingValues,
  useConfigStoreState,
  useConfigStateSelector,
  useConfigSelector,
  writeConfigValues,
} from "~/shared/config/store";
import { isConfiguredSttModel, isFmtrLocalSttModel } from "~/stt/capabilities";
import { getDefaultSttModel } from "~/stt/model-selection";

export type { StoredSettingValues } from "~/shared/config/store";

const EMPTY_STORED_SETTINGS: StoredSettingValues = {
  values: {},
  hasValues: new Set(),
};

export function useStoredSettingValuesQuery(): {
  data: StoredSettingValues | undefined;
  isLoading: boolean;
  error: Error | null;
} {
  const { snapshot, isLoading, error } = useConfigStoreState();
  return { data: snapshot?.settings, isLoading, error };
}

export function useStoredSettingValues(): StoredSettingValues {
  const { data = EMPTY_STORED_SETTINGS } = useStoredSettingValuesQuery();
  return data;
}

export function useSettingsReady(): boolean {
  return useConfigStateSelector((state) => !state.isLoading && !state.error);
}

export function useStoredSettingValue<K extends SettingKey>(
  key: K,
): {
  value: SettingValue<K> | undefined;
  hasValue: boolean;
} {
  return useConfigSelector(
    useShallow(({ values, hasValues }: StoredSettingValues) => ({
      value: values[key] as SettingValue<K> | undefined,
      hasValue: hasValues.has(key),
    })),
  );
}

export function getStoredSettingValues(): Promise<StoredSettingValues> {
  return fetchStoredSettingValues();
}

export async function initializeApplicationSettings(): Promise<void> {
  const stored = await getStoredSettingValues();
  const languageResult = await detectCommands
    .getPreferredLanguages()
    .catch(() => null);
  const updates: SettingValues = {};

  if (languageResult?.status === "ok" && languageResult.data.length > 0) {
    if (!stored.hasValues.has("ai_language")) {
      updates.ai_language = languageResult.data[0];
    }
    if (!stored.hasValues.has("spoken_languages")) {
      updates.spoken_languages = JSON.stringify(languageResult.data);
    }
  }

  if (!stored.values.current_stt_model) {
    const defaultModel = getDefaultSttModel(stored.values.current_stt_provider);
    if (defaultModel) {
      updates.current_stt_model = defaultModel;
    }
  }

  if (Object.keys(updates).length > 0) {
    await setSettingValues(updates);
  }
  const current =
    Object.keys(updates).length > 0 ? await getStoredSettingValues() : stored;
  applySettingSideEffects(current.values);
}

export function setSettingValue<K extends SettingKey>(
  key: K,
  value: SettingValue<K>,
): Promise<void> {
  return setSettingValues({ [key]: value } as SettingValues);
}

export async function setSettingValues(values: SettingValues): Promise<void> {
  const payload: Partial<{ [key in string]: JsonValue }> = {};
  for (const [key, value] of Object.entries(values)) {
    payload[key] = toConfigJsonValue(key as SettingKey, value);
  }

  if (Object.keys(payload).length > 0) {
    await writeConfigValues(payload);
  }
  applySettingSideEffects(values);
}

export async function updateSettingValue<K extends SettingKey>(
  key: K,
  update: (current: SettingValue<K> | undefined) => SettingValue<K>,
): Promise<SettingValue<K>> {
  const stored = await getStoredSettingValues();
  const definition = SETTING_DEFINITIONS[key];
  const fallback =
    "default" in definition
      ? (definition.default as SettingValue<K>)
      : undefined;
  const current = stored.hasValues.has(key)
    ? (stored.values[key] as unknown as SettingValue<K>)
    : fallback;
  const next = update(current);
  await setSettingValues({ [key]: next } as SettingValues);
  return next;
}

export function useSetSettingValue<K extends SettingKey>(key: K) {
  return useCallback(
    (value: SettingValue<K>) => {
      void setSettingValue(key, value).catch((error) => {
        console.error(`[settings] failed to update ${key}`, error);
      });
    },
    [key],
  );
}

export function useSetSettingValues() {
  return useCallback((values: SettingValues) => {
    void setSettingValues(values).catch((error) => {
      console.error("[settings] failed to update values", error);
    });
  }, []);
}

// config.json stores these keys as real JSON arrays while the setting schema
// still passes them around as JSON-encoded strings.
function toConfigJsonValue(
  key: SettingKey,
  value: boolean | number | string,
): JsonValue {
  if (!ARRAY_SETTING_KEYS.has(key)) return value;
  if (Array.isArray(value)) return value;
  try {
    const parsed: unknown = JSON.parse(String(value));
    return Array.isArray(parsed)
      ? parsed.filter((entry): entry is string => typeof entry === "string")
      : [];
  } catch {
    return [];
  }
}

function applySettingSideEffects(values: SettingValues): void {
  if (values.autostart !== undefined) {
    void (values.autostart ? enable() : disable()).catch(console.error);
  }
  if (values.respect_dnd !== undefined) {
    void detectCommands
      .setRespectDoNotDisturb(values.respect_dnd)
      .catch(console.error);
  }
  if (values.ignored_platforms !== undefined) {
    void detectCommands
      .setIgnoredBundleIds(parseStringArray(values.ignored_platforms))
      .catch(console.error);
  }
  if (values.included_platforms !== undefined) {
    void detectCommands
      .setIncludedBundleIds(parseStringArray(values.included_platforms))
      .catch(console.error);
  }
  if (values.mic_active_threshold !== undefined) {
    void detectCommands
      .setMicActiveThreshold(values.mic_active_threshold)
      .catch(console.error);
  }
  if (values.show_app_in_dock !== undefined) {
    void windowsCommands
      .setShowAppInDock(values.show_app_in_dock)
      .catch(console.error);
  }
  if (values.show_tray_icon !== undefined) {
    void trayCommands
      .setTrayIconVisible(values.show_tray_icon)
      .catch(console.error);
  }
  if (
    values.current_stt_provider !== undefined ||
    values.current_stt_model !== undefined
  ) {
    void syncLocalSttServer().catch(console.error);
  }
}

async function syncLocalSttServer(): Promise<void> {
  const { values } = await getStoredSettingValues();
  const provider = values.current_stt_provider;
  let model = values.current_stt_model;

  if (provider === "fmtr" && model && !isConfiguredSttModel(provider, model)) {
    model = "";
    await writeConfigValues({ current_stt_model: model });
  }

  if (isFmtrLocalSttModel(provider, model)) {
    await localSttCommands.startServer(model);
  } else {
    await localSttCommands.stopServer(null);
  }
}

function parseStringArray(value: string): string[] {
  try {
    const parsed: unknown = JSON.parse(value);
    return Array.isArray(parsed)
      ? parsed.filter((entry): entry is string => typeof entry === "string")
      : [];
  } catch {
    return [];
  }
}
