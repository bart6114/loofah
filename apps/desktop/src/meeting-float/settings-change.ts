import type { FloatingBarSettingsChange } from "@hypr/plugin-windows";

import type { SettingValues } from "~/settings/schema";
type FloatingOverlaySettingsStorage = Pick<
  SettingValues,
  | "floating_bar_opacity"
  | "live_caption_opacity"
  | "live_caption_width"
  | "live_caption_line_count"
  | "live_caption_position"
  | "live_caption_minimized"
>;
const DEFAULT_FLOATING_OVERLAY_SETTINGS = {
  floatingBarOpacity: 0.78,
  liveCaptionOpacity: 0.3,
  liveCaptionWidth: 440,
  liveCaptionLineCount: 1,
  liveCaptionPosition: "topCenter",
};
const FLOATING_BAR_MIN_OPACITY = 0.35,
  FLOATING_BAR_MAX_OPACITY = 0.95,
  LIVE_CAPTION_MIN_OPACITY = 0.05,
  LIVE_CAPTION_MAX_OPACITY = 1,
  LIVE_CAPTION_MIN_WIDTH = 260,
  LIVE_CAPTION_MAX_WIDTH = 640,
  LIVE_CAPTION_MIN_LINE_COUNT = 1,
  LIVE_CAPTION_MAX_LINE_COUNT = 4;
function normalizeNumber(
  value: unknown,
  fallback: number,
  min: number,
  max: number,
) {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.min(Math.max(value, min), max)
    : fallback;
}
const normalizeOpacity = normalizeNumber;
function normalizeInteger(
  value: unknown,
  fallback: number,
  min: number,
  max: number,
) {
  return Math.round(normalizeNumber(value, fallback, min, max));
}
function normalizeLiveCaptionPosition(value: unknown) {
  return typeof value === "string" &&
    [
      "topCenter",
      "topLeft",
      "topRight",
      "bottomLeft",
      "bottomRight",
      "bottomCenter",
    ].includes(value)
    ? value
    : "topCenter";
}
export function getSettingsValuesFromNativeChange(
  change: FloatingBarSettingsChange,
) {
  const values: Partial<FloatingOverlaySettingsStorage> = {};

  if (
    change.floatingBarOpacity !== null &&
    change.floatingBarOpacity !== undefined
  ) {
    values.floating_bar_opacity = normalizeOpacity(
      change.floatingBarOpacity,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.floatingBarOpacity,
      FLOATING_BAR_MIN_OPACITY,
      FLOATING_BAR_MAX_OPACITY,
    );
  }

  if (
    change.liveCaptionOpacity !== null &&
    change.liveCaptionOpacity !== undefined
  ) {
    values.live_caption_opacity = normalizeOpacity(
      change.liveCaptionOpacity,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionOpacity,
      LIVE_CAPTION_MIN_OPACITY,
      LIVE_CAPTION_MAX_OPACITY,
    );
  }

  if (
    change.liveCaptionWidth !== null &&
    change.liveCaptionWidth !== undefined
  ) {
    values.live_caption_width = normalizeNumber(
      change.liveCaptionWidth,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionWidth,
      LIVE_CAPTION_MIN_WIDTH,
      LIVE_CAPTION_MAX_WIDTH,
    );
  }

  if (
    change.liveCaptionLineCount !== null &&
    change.liveCaptionLineCount !== undefined
  ) {
    values.live_caption_line_count = normalizeInteger(
      change.liveCaptionLineCount,
      DEFAULT_FLOATING_OVERLAY_SETTINGS.liveCaptionLineCount,
      LIVE_CAPTION_MIN_LINE_COUNT,
      LIVE_CAPTION_MAX_LINE_COUNT,
    );
  }

  if (
    change.liveCaptionPosition !== null &&
    change.liveCaptionPosition !== undefined
  ) {
    values.live_caption_position = normalizeLiveCaptionPosition(
      change.liveCaptionPosition,
    );
  }

  if (
    change.liveCaptionMinimized !== null &&
    change.liveCaptionMinimized !== undefined
  ) {
    values.live_caption_minimized = change.liveCaptionMinimized === true;
  }

  return values;
}
