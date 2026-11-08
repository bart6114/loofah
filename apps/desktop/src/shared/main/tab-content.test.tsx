import { cleanup, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";

import type { Tab } from "~/store/zustand/tabs";

vi.mock("~/main/use-legacy-activity", () => ({
  useLegacyDesktopAccess: () => ({ isAvailable: () => false }),
}));
vi.mock("~/session", () => ({ TabContentNote: () => null }));
vi.mock("~/shared/deferred-view", () => ({
  deferredView: () => ({
    View: ({ tab }: { tab: Tab }) => <div>{tab.type} panel</div>,
    preload: vi.fn(),
  }),
  DeferredView: ({ children }: { children: ReactNode }) => children,
  usePreloadViews: vi.fn(),
}));
import { MainTabContent } from "./tab-content";

afterEach(cleanup);
it("opens public issue resources without authorizing a Local vault workflow", () => {
  render(
    <MainTabContent
      tab={{
        type: "task",
        id: "public-issue",
        resources: [
          {
            type: "github_issue",
            owner: "bart6114",
            repo: "loofah",
            number: 123,
          },
        ],
        active: true,
        pinned: false,
        slotId: "resource",
      }}
    />,
  );
  expect(screen.getByText("task panel")).toBeTruthy();
  expect(screen.queryByRole("status")).toBeNull();
});
it("keeps the Local onboarding tab gated without Local desktop access", () => {
  render(
    <MainTabContent
      tab={{ type: "onboarding", active: true, pinned: false, slotId: "setup" }}
    />,
  );
  expect(screen.getByRole("status")).toBeTruthy();
  expect(screen.queryByText("onboarding panel")).toBeNull();
});
