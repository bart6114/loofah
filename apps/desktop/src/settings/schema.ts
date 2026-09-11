export const SETTING_DEFINITIONS = {
  autostart: {
    type: "boolean",
    path: ["general", "autostart"],
    default: false as boolean,
  },
  auto_stop_meetings: {
    type: "boolean",
    path: ["general", "auto_stop_meetings"],
    default: true as boolean,
  },
  floating_bar_enabled: {
    type: "boolean",
    path: ["general", "floating_bar_enabled"],
    default: true as boolean,
  },
  floating_bar_opacity: {
    type: "number",
    path: ["general", "floating_bar_opacity"],
    default: 0.78 as number,
  },
  live_caption_opacity: {
    type: "number",
    path: ["general", "live_caption_opacity"],
    default: 0.3 as number,
  },
  live_caption_width: {
    type: "number",
    path: ["general", "live_caption_width"],
    default: 440 as number,
  },
  live_caption_line_count: {
    type: "number",
    path: ["general", "live_caption_line_count"],
    default: 1 as number,
  },
  live_caption_position: {
    type: "string",
    path: ["general", "live_caption_position"],
    default: "topCenter" as string,
  },
  live_caption_minimized: {
    type: "boolean",
    path: ["general", "live_caption_minimized"],
    default: true as boolean,
  },
  show_app_in_dock: {
    type: "boolean",
    path: ["general", "show_app_in_dock"],
    default: true as boolean,
  },
  show_tray_icon: {
    type: "boolean",
    path: ["general", "show_tray_icon"],
    default: true as boolean,
  },
  theme: {
    type: "string",
    path: ["general", "theme"],
    default: "system" as string,
  },
  auto_accept_related_tags: {
    type: "boolean",
    path: ["general", "auto_accept_related_tags"],
    default: false as boolean,
  },
  notification_detect: {
    type: "boolean",
    path: ["notification", "detect"],
    default: true as boolean,
  },
  respect_dnd: {
    type: "boolean",
    path: ["notification", "respect_dnd"],
    default: false as boolean,
  },
  cloud_sync_enabled: {
    type: "boolean",
    path: ["general", "cloud_sync_enabled"],
    default: true as boolean,
  },
  ai_language: {
    type: "string",
    path: ["language", "ai_language"],
    default: "en" as string,
  },
  sidebar_group_by: {
    type: "string",
    path: ["general", "sidebar_group_by"],
    default: "date" as string,
  },
  // JSON array of expanded tag names (not collapsed ones), so tags — including
  // newly created ones — start collapsed. Supersedes sidebar_collapsed_tags;
  // stale entries for that key may linger in old vault configs and are ignored.
  sidebar_expanded_tags: {
    type: "string",
    path: ["general", "sidebar_expanded_tags"],
    default: "[]" as string,
  },
  spoken_languages: {
    type: "string",
    path: ["language", "spoken_languages"],
    default: "[]" as string,
  },
  personalization_dictionary_terms: {
    type: "string",
    path: ["personalization", "dictionary_terms"],
    default: "[]" as string,
  },
  custom_summary_instructions: {
    type: "string",
    path: ["personalization", "custom_summary_instructions"],
    default: "" as string,
  },
  custom_summary_instructions_token_aware: {
    type: "boolean",
    path: ["personalization", "custom_summary_instructions_token_aware"],
    default: false as boolean,
  },
  auto_summary_prompt: {
    type: "string",
    path: ["ai", "auto_summary_prompt"],
    default: "" as string,
  },
  ignored_platforms: {
    type: "string",
    path: ["notification", "ignored_platforms"],
    default: "[]" as string,
  },
  included_platforms: {
    type: "string",
    path: ["notification", "included_platforms"],
    default: "[]" as string,
  },
  mic_active_threshold: {
    type: "number",
    path: ["notification", "mic_active_threshold"],
    default: 15 as number,
  },
  current_llm_provider: {
    type: "string",
    path: ["ai", "current_llm_provider"],
  },
  current_llm_model: {
    type: "string",
    path: ["ai", "current_llm_model"],
  },
  current_stt_provider: {
    type: "string",
    path: ["ai", "current_stt_provider"],
  },
  current_stt_model: {
    type: "string",
    path: ["ai", "current_stt_model"],
  },
  timezone: {
    type: "string",
    path: ["general", "timezone"],
  },
} as const;

export type SettingKey = keyof typeof SETTING_DEFINITIONS;

type SettingTypeMap = {
  boolean: boolean;
  number: number;
  string: string;
};

export type SettingValue<K extends SettingKey> =
  SettingTypeMap[(typeof SETTING_DEFINITIONS)[K]["type"]];

export type SettingValues = {
  [K in SettingKey]?: SettingValue<K>;
};
