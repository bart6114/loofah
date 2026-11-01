import {
  ChevronRight,
  Cloud,
  Mic,
  Palette,
  ShieldCheck,
  Sparkles,
} from "lucide-react";

import { getDesignTheme } from "@hypr/design-theme/catalog";

import type { Snapshot } from "./api";

export const settingsTitles = {
  appearance: "Appearance",
  storage: "Notes & recordings",
  transcription: "Transcription",
  summaries: "Summaries",
  privacy: "Privacy",
};
export type SettingsSection = keyof typeof settingsTitles;

export function SettingsHome({
  snapshot,
  onSelect,
}: {
  snapshot: Snapshot;
  onSelect: (section: SettingsSection) => void;
}) {
  const provider = snapshot.settings.summary_provider;
  const sections = [
    {
      id: "appearance",
      icon: Palette,
      detail: getDesignTheme(snapshot.appearance?.design_theme).name,
    },
    {
      id: "storage",
      icon: Cloud,
      detail: snapshot.sync.connected
        ? "iCloud connected"
        : "Saved on this iPhone",
    },
    {
      id: "transcription",
      icon: Mic,
      detail: snapshot.model.ready
        ? "On-device · ready"
        : "Download a model to get started",
    },
    {
      id: "summaries",
      icon: Sparkles,
      detail:
        provider === "none"
          ? "Off · choose an AI provider"
          : snapshot.settings.ai_sharing_allowed
            ? "AI sharing enabled"
            : "Permission needed to share with AI",
    },
    { id: "privacy", icon: ShieldCheck, detail: "Your data and your choices" },
  ] as const;
  return (
    <div className="settings-home">
      <div className="page-heading">
        <h2>Settings</h2>
        <p>At home on your iPhone.</p>
      </div>
      <nav className="settings-menu" aria-label="Settings sections">
        {sections.map(({ id, icon: Icon, detail }) => (
          <button
            key={id}
            className="settings-row"
            onClick={() => onSelect(id)}
          >
            <span className="settings-row-icon">
              <Icon size={21} aria-hidden="true" />
            </span>
            <span className="settings-row-copy">
              <strong>{settingsTitles[id]}</strong>
              <small>{detail}</small>
            </span>
            <ChevronRight size={18} aria-hidden="true" />
          </button>
        ))}
      </nav>
      <p className="settings-footer">
        A quiet place for your notes.
        <br />
        Built around files you own.
      </p>
    </div>
  );
}
