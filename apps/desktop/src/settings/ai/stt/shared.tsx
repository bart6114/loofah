import type { ReactNode } from "react";

import type { LocalModel } from "@hypr/plugin-local-stt";

import { AppProviderIcon } from "~/settings/ai/shared";
import { type ProviderRequirement } from "~/settings/ai/shared/eligibility";
import { sortProviders } from "~/settings/ai/shared/sort-providers";
import { localSttQueries } from "~/stt/useLocalSttModel";

export { localSttQueries as sttModelQueries };

type Provider = {
  disabled: boolean;
  id: string;
  displayName: string;
  icon: ReactNode;
  baseUrl?: string;
  models: LocalModel[] | string[];
  badge?: string | null;
  requirements: ProviderRequirement[];
  links?: {
    models?: { label: string; url: string };
    setup?: { label: string; url: string };
  };
};

// STT is on-device only (no hosted provider models left to alias), so this is
// just an identity fallback for whatever id the local model reports.
export const displayModelId = (model: string) => model;

function isOnDeviceModelId(model: string) {
  return (
    model.startsWith("soniqo-") ||
    model.startsWith("am-") ||
    model.startsWith("whisper-") ||
    model.startsWith("Quantized")
  );
}

export function displayModelLabel(model: string, displayName?: string) {
  if (isOnDeviceModelId(model)) {
    return "On device";
  }

  return displayName ?? displayModelId(model);
}

export function displayModelTitle(model: string, displayName?: string) {
  const title = displayName ?? displayModelId(model);

  return displayModelLabel(model, displayName) === title ? undefined : title;
}

export function formatModelSize(sizeBytes?: number | null) {
  if (!sizeBytes) {
    return null;
  }

  const unit = sizeBytes >= 1024 * 1024 * 1024 ? "GB" : "MB";
  const value =
    unit === "GB" ? sizeBytes / 1024 / 1024 / 1024 : sizeBytes / 1024 / 1024;

  return `~${value.toLocaleString(undefined, {
    maximumFractionDigits: value >= 10 ? 0 : 1,
  })} ${unit}`;
}

// STT is on-device only: the sole provider hosts the local Soniqo/Argmax/
// Whisper models via `localSttCommands`. There is no cloud/hosted STT.
export const _PROVIDERS = [
  {
    disabled: false,
    id: "fmtr",
    displayName: "On-device",
    badge: "Recommended",
    icon: <AppProviderIcon />,
    models: [],
    requirements: [],
  },
] as const satisfies readonly Provider[];

export const PROVIDERS = sortProviders(_PROVIDERS);
export type ProviderId = (typeof _PROVIDERS)[number]["id"];
