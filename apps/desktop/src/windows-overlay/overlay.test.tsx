import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import type { FloatingBarState, OverlaySnapshot } from "@hypr/plugin-windows";

const mocks = vi.hoisted(() => ({
  snapshot: vi.fn(),
  stop: vi.fn(),
  open: vi.fn(),
  settings: vi.fn(),
  preferences: vi.fn(),
  listener: null as null | ((event: { payload: OverlaySnapshot }) => void),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_: string, callback: typeof mocks.listener) => {
    mocks.listener = callback;
    return vi.fn();
  }),
}));
vi.mock("@hypr/plugin-windows", () => ({
  commands: {
    overlaySnapshot: mocks.snapshot,
    overlaySetSettingsOpen: mocks.preferences,
  },
  events: {
    floatingBarStop: { emit: mocks.stop },
    floatingBarOpenMain: { emit: mocks.open },
    floatingBarSettingsChange: { emit: mocks.settings },
  },
}));

import { Overlay } from "./overlay";

const recording: FloatingBarState = {
  amplitude: 0.4,
  title: "Project meeting",
  status: "recording",
  colorScheme: "dark",
  opacity: 0.9,
  liveCaptionOpacity: 0.9,
  liveCaptionWidth: 500,
  liveCaptionLineCount: 3,
  liveCaptionPosition: "topCenter",
  liveCaptionMinimized: true,
  liveCaptionToggleVisible: true,
  transcriptBubbles: [],
};
function mount() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <Overlay />
    </QueryClientProvider>,
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.stop.mockResolvedValue(undefined);
  mocks.settings.mockResolvedValue(undefined);
  mocks.snapshot.mockResolvedValue({
    revision: 1,
    state: { type: "floatingBar", state: recording },
  });
});
afterEach(cleanup);

it("routes stop and caption actions through the existing session host", async () => {
  mount();
  fireEvent.click(
    await screen.findByRole("button", { name: "Toggle live captions" }),
  );
  await waitFor(() =>
    expect(mocks.settings).toHaveBeenCalledWith(
      expect.objectContaining({ liveCaptionMinimized: false }),
    ),
  );
  expect(mocks.stop).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Stop recording" }));
  await waitFor(() => expect(mocks.stop).toHaveBeenCalledOnce());
});

it("persists caption layout choices from the Windows display panel", async () => {
  mocks.snapshot.mockResolvedValue({
    revision: 1,
    state: { type: "settings", state: recording },
  });
  mount();
  fireEvent.change(
    await screen.findByRole("slider", { name: "Caption width" }),
    {
      target: { value: "700" },
    },
  );
  await waitFor(() =>
    expect(mocks.settings).toHaveBeenCalledWith(
      expect.objectContaining({ liveCaptionWidth: 700 }),
    ),
  );
  fireEvent.change(screen.getByLabelText("Caption position"), {
    target: { value: "bottomRight" },
  });
  await waitFor(() =>
    expect(mocks.settings).toHaveBeenCalledWith(
      expect.objectContaining({ liveCaptionPosition: "bottomRight" }),
    ),
  );
});

it("keeps newer streamed state when an initial snapshot arrives late", async () => {
  let resolve!: (snapshot: OverlaySnapshot) => void;
  mocks.snapshot.mockReturnValue(
    new Promise<OverlaySnapshot>((done) => {
      resolve = done;
    }),
  );
  mount();
  await waitFor(() => expect(mocks.snapshot).toHaveBeenCalledOnce());
  act(() =>
    mocks.listener?.({
      payload: {
        revision: 3,
        state: {
          type: "floatingBar",
          state: { ...recording, title: "Current meeting" },
        },
      },
    }),
  );
  await act(async () =>
    resolve({ revision: 1, state: { type: "floatingBar", state: recording } }),
  );
  expect(screen.getByRole("button", { name: "Current meeting" })).toBeTruthy();
  expect(screen.queryByText("Project meeting")).toBeNull();
});
