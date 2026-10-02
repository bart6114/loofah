export { DEFAULT_RESULT } from "@hypr/ai-providers/models/list-common";
export { REQUEST_TIMEOUT } from "@hypr/ai-providers/models/list-common";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

import { fetchJson as sharedFetchJson } from "@hypr/ai-providers/models/list-common";
export const fetchJson = (url: string, headers: Record<string, string>) =>
  sharedFetchJson(url, headers, tauriFetch);
export { shouldIgnoreCommonKeywords } from "@hypr/ai-providers/models/list-common";
export { isDateSnapshot } from "@hypr/ai-providers/models/list-common";
export { isNonChatModel } from "@hypr/ai-providers/models/list-common";
export { isOldModel } from "@hypr/ai-providers/models/list-common";
export { sortModelsByRecency } from "@hypr/ai-providers/models/list-common";
export { partition } from "@hypr/ai-providers/models/list-common";
export { extractMetadataMap } from "@hypr/ai-providers/models/list-common";
export type {
  ModelIgnoreReason,
  IgnoredModel,
  InputModality,
  ModelMetadata,
  ListModelsResult,
} from "@hypr/ai-providers/models/list-common";
