import { Trans } from "@lingui/react/macro";
import { type ReactNode, useId } from "react";

import { Switch } from "@hypr/ui/components/ui/switch";

interface SettingItem {
  value: boolean;
  onChange: (value: boolean) => void;
  disabled?: boolean;
}

interface AppSettingsViewProps {
  autostart: SettingItem;
  autoAcceptRelatedTags: SettingItem;
  showAppInDock: SettingItem;
  showTrayIcon: SettingItem;
}

export function AppSettingsView({
  autostart,
  autoAcceptRelatedTags,
  showAppInDock,
  showTrayIcon,
}: AppSettingsViewProps) {
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
            title={<Trans>Show app in Dock</Trans>}
            description={
              <Trans>Show Loofah in the Dock and app switcher.</Trans>
            }
            checked={showAppInDock.value}
            onChange={showAppInDock.onChange}
          />
          <SettingRow
            title={<Trans>Show in menu bar</Trans>}
            description={
              <Trans>Keep Loofah available from the menu bar.</Trans>
            }
            checked={showTrayIcon.value}
            onChange={showTrayIcon.onChange}
          />
        </div>
      </section>

      <section>
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
      </section>
    </div>
  );
}

export function RecordingSettingsView({
  autoStopMeetings,
  floatingBar,
}: {
  autoStopMeetings: SettingItem;
  floatingBar: SettingItem;
}) {
  return (
    <div className="flex flex-col gap-4">
      <SettingRow
        title={<Trans>Stop recording when the meeting ends</Trans>}
        description={
          <Trans>
            Stop automatically when the meeting app stops using the microphone.
          </Trans>
        }
        checked={autoStopMeetings.value}
        onChange={autoStopMeetings.onChange}
      />
      <SettingRow
        title={<Trans>Show floating recording controls</Trans>}
        description={
          <Trans>
            Keep compact recording controls visible while recording.
          </Trans>
        }
        checked={floatingBar.value}
        onChange={floatingBar.onChange}
      />
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
