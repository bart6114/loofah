import { Trans } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { Loader2Icon } from "lucide-react";

import { AppSettingsView, RecordingSettingsView } from "./app-settings";
import { NotificationSettingsView } from "./notification";
import { Permissions } from "./permissions";
import { StorageSettingsView } from "./storage";
import { ThemeSelector } from "./theme";
import { TimezoneSelector } from "./timezone";

import { SettingsPageTitle } from "~/settings/page-title";
import {
  type StoredSettingValues,
  useSetSettingValues,
  useStoredSettingValuesQuery,
} from "~/settings/queries";
import { resolveConfigValues } from "~/shared/config";

export function SettingsApp() {
  return <GeneralSettings page="app" />;
}

export function SettingsNotifications() {
  return <GeneralSettings page="recording" />;
}

function GeneralSettings({ page }: { page: "app" | "recording" }) {
  const { data, isLoading, error } = useStoredSettingValuesQuery();
  if (error) throw error;
  if (isLoading || !data) {
    return (
      <div className="flex min-h-48 items-center justify-center">
        <Loader2Icon
          aria-label="Loading settings"
          className="text-muted-foreground size-5 animate-spin"
        />
      </div>
    );
  }
  return (
    <GeneralSettingsContent key={page} storedSettings={data} page={page} />
  );
}

function GeneralSettingsContent({
  storedSettings,
  page,
}: {
  storedSettings: StoredSettingValues;
  page: "app" | "recording";
}) {
  const config = resolveConfigValues(
    [
      "autostart",
      "show_app_in_dock",
      "show_tray_icon",
      "auto_stop_meetings",
      "floating_bar_enabled",
      "auto_accept_related_tags",
    ] as const,
    storedSettings,
  );
  const setSettingValues = useSetSettingValues();
  const form = useForm({
    defaultValues: config,
    listeners: {
      onChange: ({ formApi }) => {
        void formApi.handleSubmit();
      },
    },
    onSubmit: ({ value }) => {
      setSettingValues(value);
    },
  });

  return (
    <div className="flex flex-col gap-8">
      <SettingsPageTitle
        title={page === "app" ? <Trans>App</Trans> : <Trans>Recording</Trans>}
      />
      {page === "app" ? (
        <>
          <ThemeSelector />
          <form.Field name="autostart">
            {(autostart) => (
              <form.Field name="show_app_in_dock">
                {(dock) => (
                  <form.Field name="show_tray_icon">
                    {(tray) => (
                      <form.Field name="auto_accept_related_tags">
                        {(tags) => (
                          <AppSettingsView
                            autostart={{
                              value: autostart.state.value,
                              onChange: autostart.handleChange,
                            }}
                            showAppInDock={{
                              value: dock.state.value,
                              onChange: dock.handleChange,
                            }}
                            showTrayIcon={{
                              value: tray.state.value,
                              onChange: tray.handleChange,
                            }}
                            autoAcceptRelatedTags={{
                              value: tags.state.value,
                              onChange: tags.handleChange,
                            }}
                          />
                        )}
                      </form.Field>
                    )}
                  </form.Field>
                )}
              </form.Field>
            )}
          </form.Field>
          <TimezoneSelector />
        </>
      ) : (
        <>
          <form.Field name="auto_stop_meetings">
            {(stop) => (
              <form.Field name="floating_bar_enabled">
                {(floating) => (
                  <RecordingSettingsView
                    autoStopMeetings={{
                      value: stop.state.value,
                      onChange: stop.handleChange,
                    }}
                    floatingBar={{
                      value: floating.state.value,
                      onChange: floating.handleChange,
                    }}
                  />
                )}
              </form.Field>
            )}
          </form.Field>
          <NotificationSettingsView />
        </>
      )}
    </div>
  );
}

export function SettingsStorage() {
  return (
    <div className="flex flex-col gap-8">
      <SettingsPageTitle title={<Trans>Storage</Trans>} />
      <StorageSettingsView />
    </div>
  );
}

export function SettingsPermissions() {
  return (
    <div className="flex flex-col gap-8">
      <SettingsPageTitle title={<Trans>Permissions</Trans>} />
      <Permissions />
    </div>
  );
}
