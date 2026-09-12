import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AppSettingsView, RecordingSettingsView } from "./app-settings";

function setting(value = true) {
  return {
    value,
    onChange: vi.fn(),
  };
}

function renderAppSettings() {
  return {
    ...render(
      <AppSettingsView
        autostart={setting()}
        autoAcceptRelatedTags={setting(false)}
        showAppInDock={setting()}
        showTrayIcon={setting()}
      />,
    ),
  };
}

describe("AppSettingsView", () => {
  afterEach(() => {
    cleanup();
  });

  it("does not expose a separate live transcript overlay setting", () => {
    renderAppSettings();

    expect(screen.queryByText("Show live transcript overlay")).toBeNull();
  });

  it("keeps the floating bar setting available", () => {
    render(
      <RecordingSettingsView
        autoStopMeetings={setting()}
        floatingBar={setting(false)}
      />,
    );

    expect(screen.getByText("Show floating recording controls")).toBeTruthy();
  });

  it("does not expose a usage data setting", () => {
    renderAppSettings();

    expect(screen.queryByText("Share usage data")).toBeNull();
  });
});
