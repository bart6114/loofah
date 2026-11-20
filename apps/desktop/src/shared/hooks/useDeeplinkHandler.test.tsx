import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import type { DeepLink } from "@hypr/plugin-deeplink2";

import { useDeeplinkHandler } from "./useDeeplinkHandler";

const mocks = vi.hoisted(() => ({
  webview: { label: "main" },
  listen: vi.fn(),
  takePendingDeepLinks: vi.fn(),
  windowShow: vi.fn(),
  sessionGet: vi.fn(),
  openNew: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => mocks.webview,
}));
vi.mock("@tauri-apps/plugin-os", () => ({ platform: () => "macos" }));
vi.mock("@hypr/plugin-deeplink2", () => ({
  commands: { takePendingDeepLinks: mocks.takePendingDeepLinks },
  events: { deepLinkEvent: () => ({ listen: mocks.listen }) },
}));
vi.mock("@hypr/plugin-windows", () => ({
  dismissInstruction: vi.fn(),
  commands: { windowShow: mocks.windowShow },
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.toastError },
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionGet: mocks.sessionGet },
}));
vi.mock("~/store/zustand/tabs", () => ({
  useTabs: { getState: () => ({ openNew: mocks.openNew }) },
}));

const meetingLink = {
  to: "/meetings/open",
  search: { id: "meeting-1" },
} satisfies DeepLink;

beforeEach(() => {
  vi.resetAllMocks();
  mocks.webview.label = "main";
  mocks.listen.mockResolvedValue(vi.fn());
  mocks.takePendingDeepLinks.mockResolvedValue({ status: "ok", data: [] });
  mocks.windowShow.mockResolvedValue({ status: "ok", data: null });
  mocks.sessionGet.mockResolvedValue({
    status: "ok",
    data: { id: "meeting-1" },
  });
});

it("opens the queued meeting in the main window", async () => {
  mocks.takePendingDeepLinks.mockResolvedValueOnce({
    status: "ok",
    data: [meetingLink],
  });
  renderHook(useDeeplinkHandler);

  await waitFor(() => {
    expect(mocks.windowShow).toHaveBeenCalledWith({ type: "main" });
    expect(mocks.sessionGet).toHaveBeenCalledWith("meeting-1");
    expect(mocks.openNew).toHaveBeenCalledExactlyOnceWith({
      type: "sessions",
      id: "meeting-1",
    });
  });
});

it("drains a live meeting link once and leaves the tab alone when the meeting is missing", async () => {
  mocks.sessionGet.mockResolvedValueOnce({ status: "ok", data: null });
  renderHook(useDeeplinkHandler);
  await waitFor(() =>
    expect(mocks.takePendingDeepLinks).toHaveBeenCalledTimes(1),
  );

  mocks.takePendingDeepLinks.mockResolvedValueOnce({
    status: "ok",
    data: [meetingLink],
  });
  const [onEvent] = mocks.listen.mock.calls[0]!;
  act(() => {
    onEvent({ payload: meetingLink });
  });

  await waitFor(() => {
    expect(mocks.toastError).toHaveBeenCalledWith(
      "Meeting not found in this vault",
    );
  });
  expect(mocks.sessionGet).toHaveBeenCalledTimes(1);
  expect(mocks.openNew).not.toHaveBeenCalled();
});

it("leaves the pending queue for the main window", () => {
  mocks.webview.label = "note-meeting-1";
  renderHook(useDeeplinkHandler);

  expect(mocks.listen).not.toHaveBeenCalled();
  expect(mocks.takePendingDeepLinks).not.toHaveBeenCalled();
});
