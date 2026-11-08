import type { SettingValues } from "~/settings/schema";
export type LiveCaptionPosition =
  | "topCenter"
  | "topLeft"
  | "topRight"
  | "bottomLeft"
  | "bottomRight"
  | "bottomCenter";
export type FloatingOverlaySettings = {
  floatingBarOpacity: number;
  liveCaptionOpacity: number;
  liveCaptionWidth: number;
  liveCaptionLineCount: number;
  liveCaptionPosition: LiveCaptionPosition;
  liveCaptionMinimized: boolean;
};
type FloatingOverlaySettingsStorage = Pick<
  SettingValues,
  | "floating_bar_opacity"
  | "live_caption_opacity"
  | "live_caption_width"
  | "live_caption_line_count"
  | "live_caption_position"
  | "live_caption_minimized"
>;
export const DEFAULT_FLOATING_OVERLAY_SETTINGS: FloatingOverlaySettings = {
  floatingBarOpacity: 0.78,
  liveCaptionOpacity: 0.3,
  liveCaptionWidth: 440,
  liveCaptionLineCount: 1,
  liveCaptionPosition: "topCenter",
  liveCaptionMinimized: true,
};

const FLOATING_BAR_MIN_OPACITY = 0.35;
const FLOATING_BAR_MAX_OPACITY = 0.95;
const LIVE_CAPTION_MIN_OPACITY = 0.05;
const LIVE_CAPTION_MAX_OPACITY = 1;
const LIVE_CAPTION_MIN_WIDTH = 260;
const LIVE_CAPTION_MAX_WIDTH = 640;
const LIVE_CAPTION_MIN_LINE_COUNT = 1;
const LIVE_CAPTION_MAX_LINE_COUNT = 4;
const LIVE_CAPTION_HORIZONTAL_PADDING_PX = 32;
const LIVE_CAPTION_AVERAGE_CHARACTER_WIDTH_PX = 7.8;

const LIVE_CAPTION_POSITIONS: ReadonlySet<string> = new Set([
  "topCenter",
  "topLeft",
  "topRight",
  "bottomLeft",
  "bottomRight",
  "bottomCenter",
]);

export function getFloatingOverlaySettings(
  values: Partial<FloatingOverlaySettingsStorage>,
): FloatingOverlaySettings {
  return {
    floatingBarOpacity: normalizeOpacity(
      values.floating_bar_opacity,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.floatingBarOpacity,
      FLOATING_BAR_MIN_OPACITY,
      FLOATING_BAR_MAX_OPACITY,
    ),
    liveCaptionOpacity: normalizeOpacity(
      values.live_caption_opacity,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionOpacity,
      LIVE_CAPTION_MIN_OPACITY,
      LIVE_CAPTION_MAX_OPACITY,
    ),
    liveCaptionWidth: normalizeNumber(
      values.live_caption_width,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionWidth,
      LIVE_CAPTION_MIN_WIDTH,
      LIVE_CAPTION_MAX_WIDTH,
    ),
    liveCaptionLineCount: normalizeInteger(
      values.live_caption_line_count,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionLineCount,
      LIVE_CAPTION_MIN_LINE_COUNT,
      LIVE_CAPTION_MAX_LINE_COUNT,
    ),
    liveCaptionPosition: normalizeLiveCaptionPosition(
      values.live_caption_position,
    ),
    liveCaptionMinimized:
      (values.live_caption_minimized ??
        DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionMinimized) === true,
  };
}

export function getLiveCaptionDisplayText(
  text: string,
  settings: Pick<
    FloatingOverlaySettings,
    "liveCaptionWidth" | "liveCaptionLineCount"
  > = DEFAULT_FLOATING_OVERLAY_SETTINGS,
) {
  const normalizedText = text.trim().replace(/\s+/g, " ");
  if (!normalizedText) {
    return "";
  }

  const contentWidth = Math.max(
    settings.liveCaptionWidth - LIVE_CAPTION_HORIZONTAL_PADDING_PX,
    LIVE_CAPTION_MIN_WIDTH - LIVE_CAPTION_HORIZONTAL_PADDING_PX,
  );
  const charactersPerLine = Math.max(
    12,
    Math.floor(contentWidth / LIVE_CAPTION_AVERAGE_CHARACTER_WIDTH_PX),
  );
  const maxCharacters = Math.max(
    24,
    charactersPerLine * settings.liveCaptionLineCount,
  );

  if (normalizedText.length <= maxCharacters) {
    return normalizedText;
  }

  return `... ${getTextSuffixAtWordBoundary(normalizedText, maxCharacters - 4)}`;
}

function getTextSuffixAtWordBoundary(text: string, maxCharacters: number) {
  const suffix = text.slice(-maxCharacters).trimStart();
  const firstWhitespaceIndex = suffix.search(/\s/);
  if (firstWhitespaceIndex > 0 && firstWhitespaceIndex < suffix.length - 1) {
    return suffix.slice(firstWhitespaceIndex).trimStart();
  }

  return suffix;
}
function normalizeOpacity(
  value: unknown,
  fallback: number,
  min: number,
  max: number,
): number {
  return normalizeNumber(value, fallback, min, max);
}

function normalizeNumber(
  value: unknown,
  fallback: number,
  min: number,
  max: number,
): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return fallback;
  }

  return Math.min(Math.max(value, min), max);
}

function normalizeInteger(
  value: unknown,
  fallback: number,
  min: number,
  max: number,
): number {
  return Math.round(normalizeNumber(value, fallback, min, max));
}

function normalizeLiveCaptionPosition(value: unknown): LiveCaptionPosition {
  if (typeof value === "string" && LIVE_CAPTION_POSITIONS.has(value)) {
    return value as LiveCaptionPosition;
  }

  return DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionPosition;
}
