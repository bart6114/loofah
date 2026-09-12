import type { LocalModel } from "@hypr/plugin-local-stt";
import {
  commands as listenerCommands,
  type TranscriptionMode,
} from "@hypr/plugin-transcription";

type LiveTranscriptionConfig = {
  languages: string[];
  transcriptionMode?: TranscriptionMode;
};

// Parakeet-EOU and English-only Whisper cannot decode other languages.
const ENGLISH_LANGUAGE_CODES = new Set(["en"]);

export function isSupportedLocalSttModel(
  model?: string | null,
): model is LocalModel {
  return (
    typeof model === "string" &&
    (model.startsWith("soniqo-") ||
      model.startsWith("am-") ||
      model.startsWith("whisper-") ||
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
    model === "soniqo-parakeet-streaming" ||
    model === "whisper-large-v3" ||
    /^Quantized(Tiny|Base|Small)(En)?$/.test(model ?? "") ||
    model === "QuantizedLargeTurbo"
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
  meetingLanguages?: readonly string[] | null,
) {
  const seen = new Set<string>();
  const languages: string[] = [];

  // Legacy vaults stored additional languages; explicit meeting languages are independent of AI output.
  for (const language of meetingLanguages ?? [
    mainLanguage,
    ...(spokenLanguages ?? []),
  ]) {
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

  // Keep every language so the backend can validate coverage and select a fallback.
  const englishOnly =
    model === "soniqo-parakeet-streaming" || model?.endsWith("En");
  const supportsAllLive =
    !englishOnly ||
    languages.every((language) =>
      ENGLISH_LANGUAGE_CODES.has(baseLanguageCode(language)),
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
  timing,
}: {
  provider?: string | null;
  model?: string | null;
  languages: readonly string[];
  timing?: string;
}): Promise<LiveTranscriptionConfig> {
  if (timing === "batch") {
    return { languages: [...languages], transcriptionMode: "batch" };
  }

  if (isFmtrLocalSttModel(provider, model)) {
    const config = getOnDeviceTranscriptionConfig(model, languages);
    if (
      config.transcriptionMode === "live" &&
      !(await isSupportedLanguagesLive("fmtr", model, languages))
    ) {
      return { ...config, transcriptionMode: "batch" };
    }
    return config;
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
