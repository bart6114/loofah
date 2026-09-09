import { Trans, useLingui } from "@lingui/react/macro";
import { platform } from "@tauri-apps/plugin-os";
import { type ReactNode, useId } from "react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@hypr/ui/components/ui/select";
import { Switch } from "@hypr/ui/components/ui/switch";

interface SettingItem {
  value: boolean;
  onChange: (value: boolean) => void;
  disabled?: boolean;
}

interface AppSettingsViewProps {
  autostart: SettingItem;
  autoStopMeetings: SettingItem;
  floatingBar: SettingItem;
  autoAcceptRelatedTags: SettingItem;
  showAppInDock: SettingItem;
  showTrayIcon: SettingItem;
  audioRetention: {
    value: string;
    onChange: (value: string) => void;
  };
}

export function AppSettingsView({
  autostart,
  autoStopMeetings,
  floatingBar,
  autoAcceptRelatedTags,
  showAppInDock,
  showTrayIcon,
  audioRetention,
}: AppSettingsViewProps) {
  const isWindows = platform() === "windows";
  return (
    <div className="flex flex-col gap-8">
      <section>
        <div className="flex flex-col gap-4">
          <SettingRow
            title={<Trans>Start Loofah at login</Trans>}
            description={
              <Trans>Always ready without manually launching.</Trans>
            }
            checked={autostart.value}
            onChange={autostart.onChange}
          />
          <SettingRow
            title={
              isWindows ? (
                <Trans>Show app in taskbar</Trans>
              ) : (
                <Trans>Show app in Dock</Trans>
              )
            }
            description={
              isWindows ? (
                <Trans>Show Loofah in the taskbar and app switcher.</Trans>
              ) : (
                <Trans>Show Loofah in the Dock and app switcher.</Trans>
              )
            }
            checked={showAppInDock.value}
            onChange={showAppInDock.onChange}
          />
          <SettingRow
            title={<Trans>Show tray icon</Trans>}
            description={
              isWindows ? (
                <Trans>Keep Loofah available from the notification area.</Trans>
              ) : (
                <Trans>Keep Loofah available from the menu bar.</Trans>
              )
            }
            checked={showTrayIcon.value}
            onChange={showTrayIcon.onChange}
          />
        </div>
      </section>

      <section>
        <h2 className="mb-4 font-sans text-lg font-semibold">
          <Trans>Meetings</Trans>
        </h2>
        <div className="flex flex-col gap-4">
          <SettingRow
            title={<Trans>Stop when meeting ends</Trans>}
            description={
              <Trans>
                Automatically stop listening when the meeting app releases the
                microphone.
              </Trans>
            }
            checked={autoStopMeetings.value}
            onChange={autoStopMeetings.onChange}
          />
          <SettingRow
            title={<Trans>Show floating bar</Trans>}
            description={
              <Trans>Show the compact floating control while listening.</Trans>
            }
            checked={floatingBar.value}
            onChange={floatingBar.onChange}
          />
          <SettingRow
            title={<Trans>Automatically apply related tags</Trans>}
            description={
              <Trans>
                Apply only high-confidence tags from similar session content.
              </Trans>
            }
            checked={autoAcceptRelatedTags.value}
            onChange={autoAcceptRelatedTags.onChange}
          />
          <AudioRetentionRow
            value={audioRetention.value}
            onChange={audioRetention.onChange}
          />
        </div>
      </section>
    </div>
  );
}

const AUDIO_RETENTION_OPTIONS = [
  "none",
  "oneDay",
  "threeDays",
  "oneWeek",
  "oneMonth",
  "forever",
] as const;

function AudioRetentionRow({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useLingui();
  const titleId = useId();
  const descriptionId = useId();
  const copyByValue = {
    none: t`Don't save`,
    oneDay: t`1 day`,
    threeDays: t`3 days`,
    oneWeek: t`1 week`,
    oneMonth: t`1 month`,
    forever: t`Forever`,
  } as const;

  return (
    <div className="flex items-center justify-between gap-4">
      <div className="flex-1">
        <h3 id={titleId} className="mb-1 text-sm font-medium">
          <Trans>Audio file retention</Trans>
        </h3>
        <p id={descriptionId} className="text-muted-foreground text-xs">
          <Trans>How long recorded meeting audio is kept on this device.</Trans>
        </p>
      </div>
      <Select value={value} onValueChange={onChange}>
        <SelectTrigger
          aria-labelledby={titleId}
          aria-describedby={descriptionId}
          className="bg-card h-9 w-36 shadow-none focus:ring-0"
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {AUDIO_RETENTION_OPTIONS.map((option) => (
            <SelectItem key={option} value={option}>
              {copyByValue[option]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

function SettingRow({
  title,
  description,
  checked,
  onChange,
  disabled = false,
}: {
  title: ReactNode;
  description: ReactNode;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}) {
  const titleId = useId();
  const descriptionId = useId();

  return (
    <div className="flex items-center justify-between gap-4">
      <div className="flex-1">
        <h3 id={titleId} className="mb-1 text-sm font-medium">
          {title}
        </h3>
        <p id={descriptionId} className="text-muted-foreground text-xs">
          {description}
        </p>
      </div>
      <Switch
        checked={checked}
        onCheckedChange={onChange}
        disabled={disabled}
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      />
    </div>
  );
}
