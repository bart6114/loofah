import { cleanup, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Tab } from "~/store/zustand/tabs";
const mocks = vi.hoisted(() => ({
  available: vi.fn(),
  hydrate: vi.fn(),
  preload: vi.fn(),
}));
vi.mock("~/main/use-legacy-activity", () => ({
  useLegacyDesktopAccess: () => ({
    scope: { client: { features: [] } },
    isAvailable: mocks.available,
  }),
}));
vi.mock("./general", () => ({
  SettingsApp: () => <div>Device appearance</div>,
  SettingsPermissions: () => (
    <div>Native microphone and system audio permissions</div>
  ),
  SettingsNotifications: () => (
    <div>Legacy recording and reminder controls</div>
  ),
  SettingsStorage: () => <div>Local vault storage</div>,
}));
vi.mock("~/settings/hydration-boundary", () => ({
  SettingsHydrationBoundary: ({ children }: { children: ReactNode }) => {
    mocks.hydrate();
    return children;
  },
}));
vi.mock("~/shared/application/vault-processing-settings", () => ({
  VaultProcessingSettings: () => <div>Server processing settings</div>,
}));
vi.mock("~/shared/main", () => ({
  StandardContentWrapper: ({ children }: { children: ReactNode }) => children,
}));
vi.mock("~/shared/deferred-view", () => ({
  deferredView: () => ({
    View: () => <div>Legacy processor</div>,
    preload: vi.fn(),
  }),
  DeferredView: ({ children }: { children: ReactNode }) => children,
  usePreloadViews: mocks.preload,
}));
import { TabContentSettings } from "./index";
function tab(active: string) {
  return { type: "settings", state: { tab: active } } as Extract<
    Tab,
    { type: "settings" }
  >;
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.available.mockReturnValue(false);
});
afterEach(cleanup);
describe("device settings in a Remote workspace", () => {
  it("opens native permissions without vault access, processor preloads or config hydration", () => {
    render(<TabContentSettings tab={tab("permissions")} />);
    expect(
      screen.getByText("Native microphone and system audio permissions"),
    ).toBeTruthy();
    expect(mocks.hydrate).not.toHaveBeenCalled();
    expect(mocks.preload).toHaveBeenCalledWith([]);
    expect(screen.queryByText("Server processing settings")).toBeNull();
  });
  it.each(["notifications", "storage", "intelligence", "transcription"])(
    "does not expose the unmigrated %s workflow through device permission access",
    (active) => {
      render(<TabContentSettings tab={tab(active)} />);
      expect(screen.getByRole("status")).toBeTruthy();
      expect(screen.getByText("Server processing settings")).toBeTruthy();
      expect(
        screen.queryByText("Legacy recording and reminder controls"),
      ).toBeNull();
      expect(screen.queryByText("Local vault storage")).toBeNull();
      expect(mocks.hydrate).not.toHaveBeenCalled();
    },
  );
  it("preserves the qualified Local recording settings path", () => {
    mocks.available.mockReturnValue(true);
    render(<TabContentSettings tab={tab("notifications")} />);
    expect(
      screen.getByText("Legacy recording and reminder controls"),
    ).toBeTruthy();
    expect(mocks.hydrate).toHaveBeenCalledOnce();
  });
});
