import {
  useStoredSettingValues,
  type StoredSettingValues,
} from "~/settings/queries";
import {
  SETTING_DEFINITIONS,
  type SettingKey,
  type SettingValue,
} from "~/settings/schema";

type JsonParsedKeys =
  | "spoken_languages"
  | "personalization_dictionary_terms"
  | "ignored_platforms"
  | "included_platforms"
  | "sidebar_expanded_tags";

type ConfigValueType<K extends SettingKey> = K extends "meeting_languages"
  ? string[] | undefined
  : K extends JsonParsedKeys
    ? string[]
    : K extends keyof typeof SETTING_DEFINITIONS
      ? "default" extends keyof (typeof SETTING_DEFINITIONS)[K]
        ? SettingValue<K>
        : SettingValue<K> | undefined
      : never;

const JSON_PARSED_KEYS = new Set<SettingKey>([
  "spoken_languages",
  "personalization_dictionary_terms",
  "ignored_platforms",
  "included_platforms",
  "sidebar_expanded_tags",
]);

export function useConfigValue<K extends SettingKey>(
  key: K,
): ConfigValueType<K> {
  return resolveConfigValue(key, useStoredSettingValues());
}

export function useConfigValues<K extends SettingKey>(
  keys: readonly K[],
): { [P in K]: ConfigValueType<P> } {
  return resolveConfigValues(keys, useStoredSettingValues());
}

export function resolveConfigValues<K extends SettingKey>(
  keys: readonly K[],
  stored: StoredSettingValues,
): { [P in K]: ConfigValueType<P> } {
  const result = {} as { [P in K]: ConfigValueType<P> };
  for (const key of keys) result[key] = resolveConfigValue(key, stored);
  return result;
}

export function resolveConfigValue<K extends SettingKey>(
  key: K,
  { values, hasValues }: StoredSettingValues,
): ConfigValueType<K> {
  const definition = SETTING_DEFINITIONS[key];
  const defaultValue = "default" in definition ? definition.default : undefined;

  const value = hasValues.has(key) ? values[key] : defaultValue;
  if (key === "meeting_languages") {
    return (
      value == null ? undefined : parseStringArray(value, [])
    ) as ConfigValueType<K>;
  }
  if (JSON_PARSED_KEYS.has(key)) {
    return parseStringArray(
      value,
      parseStringArray(defaultValue, []),
    ) as ConfigValueType<K>;
  }

  return value as ConfigValueType<K>;
}

// Cached per raw string so consumers get a stable array identity across
// renders (memo deps would otherwise invalidate every time). Treat the
// returned arrays as immutable.
const parsedStringArrayCache = new Map<string, string[]>();

function parseStringArray(value: unknown, fallback: string[]): string[] {
  if (Array.isArray(value)) {
    return value.filter((entry): entry is string => typeof entry === "string");
  }
  if (typeof value !== "string") return fallback;
  const cached = parsedStringArrayCache.get(value);
  if (cached) return cached;
  try {
    const parsed = JSON.parse(value);
    if (!Array.isArray(parsed)) return fallback;
    const result = parsed.filter(
      (entry): entry is string => typeof entry === "string",
    );
    parsedStringArrayCache.set(value, result);
    return result;
  } catch {
    return fallback;
  }
}
