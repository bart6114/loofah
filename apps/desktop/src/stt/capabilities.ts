import type { LocalModel } from "@hypr/plugin-local-stt";
import {
  commands as listenerCommands,
  type TranscriptionMode,
} from "@hypr/plugin-transcription";

type LiveTranscriptionConfig = {
  languages: string[];
  transcriptionMode?: TranscriptionMode;
};

// Parakeet-EOU (the streaming model) has an English-only vocabulary; it decodes
// other languages into gibberish. Must stay in sync with the authoritative Rust
// check, `is_parakeet_eou_language` in crates/language/src/lib.rs.
const SONIQO_STREAMING_LANGUAGE_CODES = new Set(["en"]);

export function isSupportedLocalSttModel(
  model?: string | null,
): model is LocalModel {
  return (
    typeof model === "string" &&
    (model.startsWith("soniqo-") ||
      model.startsWith("onnx-") ||
      model.startsWith("am-") ||
      model.startsWith("Quantized"))
  );
}

export function isFmtrLocalSttModel(
  provider?: string | null,
  model?: string | null,
): model is LocalModel {
  return provider === "fmtr" && isSupportedLocalSttModel(model);
}

export function isConfiguredSttModel(
  provider?: string | null,
  model?: string | null,
) {
  if (!provider || !model) {
    return false;
  }

  if (provider === "fmtr") {
    return isSupportedLocalSttModel(model);
  }

  return true;
}

export function isRealtimeLocalModel(model?: string | null) {
  return (
    model === "soniqo-parakeet-streaming" || model === "onnx-parakeet-streaming"
  );
}

function baseLanguageCode(language: string) {
  return language.split(/[-_]/)[0]?.toLowerCase() ?? "";
}

export async function isSupportedLanguagesLive(
  provider: string,
  model: string | null | undefined,
  languages: readonly string[],
) {
  const result = await listenerCommands.isSupportedLanguagesLive(
    provider,
    model ?? null,
    [...languages],
  );

  return result.status === "ok" ? result.data : true;
}

export async function isSupportedLanguagesBatch(
  provider: string,
  model: string | null | undefined,
  languages: readonly string[],
) {
  const result = await listenerCommands.isSupportedLanguagesBatch(
    provider,
    model ?? null,
    [...languages],
  );

  return result.status === "ok" ? result.data : true;
}

export function getTranscriptionLanguages(
  mainLanguage: string | null | undefined,
  spokenLanguages: readonly string[] | null | undefined,
) {
  const seen = new Set<string>();
  const languages: string[] = [];

  for (const language of [mainLanguage, ...(spokenLanguages ?? [])]) {
    if (!language) {
      continue;
    }

    const baseCode = baseLanguageCode(language);
    if (!baseCode || seen.has(baseCode)) {
      continue;
    }

    seen.add(baseCode);
    languages.push(language);
  }

  return languages;
}

export function getOnDeviceTranscriptionConfig(
  model: string | null | undefined,
  languages: readonly string[],
): LiveTranscriptionConfig {
  if (!isRealtimeLocalModel(model)) {
    return {
      languages: [...languages],
      transcriptionMode: "batch",
    };
  }

  // Demote to batch when ANY configured language is outside the streaming
  // model's support: the batch model covers more languages, and sending a
  // truncated language list would bypass the Rust-side demotion check.
  const supportsAllLive = languages.every((language) =>
    SONIQO_STREAMING_LANGUAGE_CODES.has(baseLanguageCode(language)),
  );

  return {
    languages: [...languages],
    transcriptionMode: supportsAllLive ? "live" : "batch",
  };
}

export function getOnDeviceTranscriptionMode(
  model: string | null | undefined,
  languages: readonly string[] = [],
) {
  return getOnDeviceTranscriptionConfig(model, languages).transcriptionMode;
}

export async function getLiveTranscriptionConfig({
  provider,
  model,
  languages,
}: {
  provider?: string | null;
  model?: string | null;
  languages: readonly string[];
}): Promise<LiveTranscriptionConfig> {
  if (isFmtrLocalSttModel(provider, model)) {
    return getOnDeviceTranscriptionConfig(model, languages);
  }

  const config = {
    languages: [...languages],
    transcriptionMode: undefined as TranscriptionMode | undefined,
  } satisfies LiveTranscriptionConfig;

  if (!provider || languages.length <= 1) {
    return config;
  }

  if (await isSupportedLanguagesLive(provider, model, languages)) {
    return config;
  }

  const primaryLanguage = languages[0];
  if (
    primaryLanguage &&
    (await isSupportedLanguagesLive(provider, model, [primaryLanguage]))
  ) {
    return {
      ...config,
      languages: [primaryLanguage],
    };
  }

  return config;
}

export async function isLiveTranscriptionSupported(
  provider?: string | null,
  model?: string | null,
) {
  if (!provider || !model) {
    return false;
  }

  return isSupportedLanguagesLive(provider, model, []);
}
