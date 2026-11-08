import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
const fake = vi.hoisted(() => ({
  open: vi.fn(),
  note: vi.fn(),
  record: vi.fn(),
  navigate: vi.fn(),
  tab: null as ((event: any) => void) | null,
  link: null as ((event: any) => void) | null,
}));
vi.mock("@tanstack/react-router", () => ({ useNavigate: () => fake.navigate }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({}),
}));
vi.mock("../useNewNote", () => ({
  useNewNote: () => fake.note,
  useNewNoteAndListen: () => fake.record,
}));
vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (select: any) => select({ openNew: fake.open }),
  isTabInputSupported: () => true,
}));
vi.mock("@hypr/plugin-windows", () => ({
  events: {
    openTab: () => ({
      listen: async (callback: any) => {
        fake.tab = callback;
        return vi.fn();
      },
    }),
    navigate: () => ({
      listen: async (callback: any) => {
        fake.link = callback;
        return vi.fn();
      },
    }),
  },
}));
import { useNavigationEvents } from "./useNavigationEvents";
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
it("routes the tray recording intent into the shared recording action without ordinary session creation", async () => {
  const view = renderHook(useNavigationEvents);
  await act(async () => {});
  act(() =>
    fake.tab!({
      payload: {
        tab: { type: "sessions", id: "new", state: { autoStart: true } },
      },
    }),
  );
  expect(fake.record).toHaveBeenCalledOnce();
  expect(fake.note).not.toHaveBeenCalled();
  expect(fake.open).not.toHaveBeenCalled();
  act(() => fake.tab!({ payload: { tab: { type: "sessions", id: "new" } } }));
  expect(fake.note).toHaveBeenCalledOnce();
  view.unmount();
  act(() =>
    fake.tab!({
      payload: {
        tab: { type: "sessions", id: "new", state: { autoStart: true } },
      },
    }),
  );
  expect(fake.record).toHaveBeenCalledOnce();
});
it("keeps explicit recording links distinct from ordinary new-note links", async () => {
  renderHook(useNavigationEvents);
  await act(async () => {});
  act(() =>
    fake.link!({ payload: { path: "/app/new", search: { record: "true" } } }),
  );
  expect(fake.record).toHaveBeenCalledOnce();
  expect(fake.note).not.toHaveBeenCalled();
  act(() => fake.link!({ payload: { path: "/app/new" } }));
  expect(fake.note).toHaveBeenCalledOnce();
});
