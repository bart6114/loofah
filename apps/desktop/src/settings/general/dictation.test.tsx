import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  save: vi.fn(),
  permission: vi.fn(),
  open: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("~/settings/queries", () => ({ setSettingValues: mocks.save }));
vi.mock("~/shared/hooks/usePermissions", () => ({
  usePermission: () => ({
    status: "authorized",
    open: mocks.permission,
    request: mocks.permission,
  }),
}));
vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (selector: (state: unknown) => unknown) =>
    selector({ openNew: mocks.open }),
}));
vi.mock("~/shared/config", async (original) => ({
  ...(await original<typeof import("~/shared/config")>()),
  useConfigValues: () => ({
    current_stt_provider: "fmtr",
    current_stt_model: "whisper-large-v3",
    meeting_languages: ["en"],
  }),
}));
vi.mock("@hypr/plugin-local-stt", () => ({
  commands: {
    isModelDownloaded: () => Promise.resolve({ status: "ok", data: true }),
  },
}));

import { DictationSettings, parseDictationShortcut } from "./dictation";

import { useDictationShortcut } from "~/dictation/state";

function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <DictationSettings
        storedSettings={{
          values: {
            dictation_enabled: true,
            dictation_shortcut: "Ctrl+Shift+D",
            dictation_trigger_mode: "hold",
          },
          hasValues: new Set([
            "dictation_enabled",
            "dictation_shortcut",
            "dictation_trigger_mode",
          ]),
        }}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  mocks.invoke.mockImplementation((command) =>
    Promise.resolve(
      command === "dictation_get_status"
        ? { attemptId: 0, revision: 0, phase: "idle", busy: false }
        : null,
    ),
  );
  mocks.save.mockResolvedValue(undefined);
  useDictationShortcut.setState({ capturing: false, unavailable: false });
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function captureInput() {
  fireEvent.click(screen.getByRole("button", { name: "Change shortcut" }));
  return screen.findByRole("textbox", { name: "Press a new shortcut" });
}

describe("dictation shortcut editing", () => {
  it("suspends the global shortcut before accepting keys", async () => {
    mount();
    await captureInput();
    expect(mocks.invoke).toHaveBeenCalledWith(
      "plugin:windows|dictation_shortcut_configure",
      { shortcut: "" },
    );
    expect(useDictationShortcut.getState().capturing).toBe(true);
  });
  it("lets Tab leave capture and keeps the previous shortcut", async () => {
    mount();
    const input = await captureInput();
    expect(fireEvent.keyDown(input, { key: "Tab" })).toBe(true);
    await waitFor(() =>
      expect(useDictationShortcut.getState().capturing).toBe(false),
    );
    expect(mocks.save).not.toHaveBeenCalled();
  });
  it("cancels on Escape without changing settings", async () => {
    mount();
    fireEvent.keyDown(await captureInput(), { key: "Escape" });
    expect(mocks.save).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(useDictationShortcut.getState().capturing).toBe(false),
    );
  });
  it("explains rejected shortcuts", async () => {
    mount();
    fireEvent.keyDown(await captureInput(), { key: "F8" });
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(mocks.save).not.toHaveBeenCalled();
  });
  it("registers a candidate before saving", async () => {
    mount();
    fireEvent.keyDown(await captureInput(), {
      key: "k",
      ctrlKey: true,
      shiftKey: true,
    });
    await waitFor(() =>
      expect(mocks.save).toHaveBeenCalledWith(
        expect.objectContaining({ dictation_shortcut: "Ctrl+Shift+K" }),
      ),
    );
    const registration =
      mocks.invoke.mock.invocationCallOrder[
        mocks.invoke.mock.calls.findIndex(
          ([command, args]) =>
            command === "plugin:windows|dictation_shortcut_configure" &&
            args.shortcut === "Ctrl+Shift+K",
        )
      ];
    expect(registration).toBeLessThan(mocks.save.mock.invocationCallOrder[0]);
  });
  it("does not persist a shortcut that cannot register", async () => {
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "dictation_get_status")
        return { attemptId: 0, revision: 0, phase: "idle", busy: false };
      if (args?.shortcut === "Ctrl+Shift+K") throw new Error("unavailable");
      return null;
    });
    mount();
    fireEvent.keyDown(await captureInput(), {
      key: "k",
      ctrlKey: true,
      shiftKey: true,
    });
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(mocks.save).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(useDictationShortcut.getState().capturing).toBe(false),
    );
  });
  it("restores the old registration if saving fails", async () => {
    mocks.save.mockRejectedValue(new Error("disk"));
    mount();
    fireEvent.keyDown(await captureInput(), {
      key: "k",
      ctrlKey: true,
      shiftKey: true,
    });
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith(
        "plugin:windows|dictation_shortcut_configure",
        { shortcut: "Ctrl+Shift+D" },
      ),
    );
    expect(await screen.findByRole("alert")).toBeTruthy();
  });
  it("also registers Reset before saving and rolls back failure", async () => {
    mocks.save.mockRejectedValue(new Error("disk"));
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith(
        "plugin:windows|dictation_shortcut_configure",
        { shortcut: "Ctrl+Shift+D" },
      ),
    );
  });
  it("clears capture suspension on unmount", async () => {
    const view = mount();
    await captureInput();
    act(() => view.unmount());
    await waitFor(() =>
      expect(useDictationShortcut.getState().capturing).toBe(false),
    );
  });
});

it("uses logical non-US letters and platform modifier names", () => {
  expect(
    parseDictationShortcut(
      {
        key: "z",
        ctrlKey: true,
        shiftKey: true,
        altKey: false,
        metaKey: false,
      },
      true,
    ),
  ).toBe("Ctrl+Shift+Z");
  expect(
    parseDictationShortcut(
      {
        key: "k",
        ctrlKey: true,
        shiftKey: false,
        altKey: false,
        metaKey: true,
      },
      false,
    ),
  ).toBe("Ctrl+Win+K");
  expect(
    parseDictationShortcut(
      {
        key: "∆",
        ctrlKey: true,
        shiftKey: false,
        altKey: true,
        metaKey: false,
      },
      true,
    ),
  ).toBe(null);
});
