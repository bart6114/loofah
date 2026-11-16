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

import { TooltipProvider } from "@hypr/ui/components/ui/tooltip";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  save: vi.fn(),
  permission: vi.fn(),
  open: vi.fn(),
  checkPermission: vi.fn(),
  requestPermission: vi.fn(),
  openPermission: vi.fn(),
  permissionStatuses: {} as Record<string, string>,
}));
vi.mock("@hypr/plugin-permissions", () => ({
  commands: {
    checkPermission: mocks.checkPermission,
    requestPermission: mocks.requestPermission,
    openPermission: mocks.openPermission,
  },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("~/settings/queries", () => ({ setSettingValues: mocks.save }));
vi.mock("~/shared/hooks/usePermissions", () => ({
  usePermission: (type: string) => ({
    status: mocks.permissionStatuses[type] ?? "authorized",
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

function mount(
  client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  }),
) {
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
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
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  mocks.permissionStatuses = {};
  mocks.invoke.mockImplementation((command) =>
    Promise.resolve(
      command === "dictation_get_status"
        ? { attemptId: 0, revision: 0, phase: "idle", busy: false }
        : null,
    ),
  );
  mocks.save.mockResolvedValue(undefined);
  mocks.checkPermission.mockResolvedValue({ status: "ok", data: "authorized" });
  mocks.requestPermission.mockResolvedValue({ status: "ok", data: null });
  mocks.openPermission.mockResolvedValue({ status: "ok", data: null });
  useDictationShortcut.setState({ capturing: false, unavailable: false });
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

describe("dictation permission requests", () => {
  it("prioritizes Accessibility when both macOS permissions are missing", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    mocks.permissionStatuses = {
      accessibility: "denied",
      inputMonitoring: "denied",
    };
    mount();
    expect(
      await screen.findByText("Allow Accessibility to insert your words."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    await waitFor(() => expect(mocks.permission).toHaveBeenCalled());
  });
  it("does not require separate Input Monitoring when Accessibility is authorized", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    mocks.permissionStatuses = {
      accessibility: "authorized",
      inputMonitoring: "denied",
    };
    mount();
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    expect(
      screen.queryByText("Allow Input Monitoring to use your shortcut."),
    ).toBeNull();
    expect(mocks.requestPermission).not.toHaveBeenCalled();
  });

  async function toggle() {
    const button = screen.getByRole("switch");
    await waitFor(() => expect(button.hasAttribute("disabled")).toBe(false));
    fireEvent.click(button);
    await waitFor(() => expect(button.hasAttribute("disabled")).toBe(false));
  }

  it("requests denied Accessibility access on every explicit re-enable", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    mocks.checkPermission.mockResolvedValue({ status: "ok", data: "denied" });
    mount();
    expect(mocks.requestPermission).not.toHaveBeenCalled();
    for (let count = 1; count <= 2; count++) {
      await toggle();
      expect(mocks.requestPermission).toHaveBeenCalledTimes(count - 1);
      await toggle();
      await waitFor(() =>
        expect(mocks.requestPermission).toHaveBeenCalledTimes(count),
      );
      expect(mocks.requestPermission).toHaveBeenLastCalledWith("accessibility");
      await waitFor(() =>
        expect(mocks.openPermission).toHaveBeenCalledTimes(count),
      );
    }
  });

  it.each(["authorized", "neverRequested"])(
    "handles %s Accessibility access",
    async (permission) => {
      vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
      mocks.checkPermission.mockResolvedValue({
        status: "ok",
        data: permission,
      });
      mount();
      await toggle();
      await toggle();
      await waitFor(() =>
        expect(mocks.checkPermission).toHaveBeenCalledWith("accessibility"),
      );
      expect(mocks.requestPermission).toHaveBeenCalledTimes(
        permission === "authorized" ? 0 : 1,
      );
      expect(mocks.openPermission).not.toHaveBeenCalled();
    },
  );

  it("does not request macOS permissions on Windows", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("Win32");
    mount();
    await toggle();
    await toggle();
    expect(mocks.checkPermission).not.toHaveBeenCalled();
  });

  it("does not request access if enabling could not be saved", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    mount();
    await toggle();
    mocks.save.mockRejectedValueOnce(new Error("disk"));
    await toggle();
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(mocks.checkPermission).not.toHaveBeenCalled();
  });
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
  it.each(["Escape", "Tab", "blur"])(
    "cancels deferred registration on %s without saving",
    async (cancel) => {
      let finishRegistration!: () => void;
      const registration = new Promise<void>((resolve) => {
        finishRegistration = resolve;
      });
      mocks.invoke.mockImplementation(async (command, args) => {
        if (command === "dictation_get_status")
          return { attemptId: 0, revision: 0, phase: "idle", busy: false };
        if (args?.shortcut === "Ctrl+Shift+K") await registration;
        return null;
      });
      mount();
      const input = await captureInput();
      fireEvent.keyDown(input, { key: "k", ctrlKey: true, shiftKey: true });
      await waitFor(() =>
        expect(mocks.invoke).toHaveBeenCalledWith(
          "plugin:windows|dictation_shortcut_configure",
          { shortcut: "Ctrl+Shift+K" },
        ),
      );
      if (cancel === "blur") fireEvent.blur(input);
      else fireEvent.keyDown(input, { key: cancel });
      await waitFor(() => expect(screen.queryByRole("textbox")).toBeNull());
      expect(
        screen.getByRole("button", { name: "Cancel" }).hasAttribute("disabled"),
      ).toBe(true);
      await act(async () => finishRegistration());
      await waitFor(() =>
        expect(
          screen
            .getByRole("button", { name: "Change shortcut" })
            .hasAttribute("disabled"),
        ).toBe(false),
      );
      expect(mocks.save).not.toHaveBeenCalled();
      expect(mocks.invoke).toHaveBeenLastCalledWith(
        "plugin:windows|dictation_shortcut_configure",
        { shortcut: "Ctrl+Shift+D" },
      );
    },
  );
  it("waits for a cancelled save and rollback before allowing another edit", async () => {
    let finishSave!: () => void;
    let finishRollback!: () => void;
    mocks.save
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            finishSave = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            finishRollback = resolve;
          }),
      );
    mount();
    const input = await captureInput();
    fireEvent.keyDown(input, { key: "k", ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(mocks.save).toHaveBeenCalledTimes(1));
    fireEvent.keyDown(input, { key: "Escape" });
    expect(
      screen.getByRole("button", { name: "Cancel" }).hasAttribute("disabled"),
    ).toBe(true);
    await act(async () => finishSave());
    await waitFor(() => expect(mocks.save).toHaveBeenCalledTimes(2));
    expect(mocks.save).toHaveBeenLastCalledWith({
      dictation_shortcut: "Ctrl+Shift+D",
    });
    expect(
      screen.getByRole("button", { name: "Cancel" }).hasAttribute("disabled"),
    ).toBe(true);
    await act(async () => finishRollback());
    await waitFor(() =>
      expect(
        screen
          .getByRole("button", { name: "Change shortcut" })
          .hasAttribute("disabled"),
      ).toBe(false),
    );
    expect(screen.getByText("Ctrl+Shift+D")).toBeTruthy();
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "plugin:windows|dictation_shortcut_configure",
      { shortcut: "Ctrl+Shift+D" },
    );
  });
  it("restores a pending edit after unmount and blocks capture on remount until settled", async () => {
    let finishSave!: () => void;
    mocks.save.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finishSave = resolve;
        }),
    );
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const view = mount(client);
    fireEvent.keyDown(await captureInput(), {
      key: "k",
      ctrlKey: true,
      shiftKey: true,
    });
    await waitFor(() => expect(mocks.save).toHaveBeenCalledTimes(1));
    view.unmount();
    mount(client);
    expect(
      screen.getByRole("button", { name: "Cancel" }).hasAttribute("disabled"),
    ).toBe(true);
    await act(async () => finishSave());
    await waitFor(() =>
      expect(
        screen
          .getByRole("button", { name: "Change shortcut" })
          .hasAttribute("disabled"),
      ).toBe(false),
    );
    expect(mocks.save).toHaveBeenLastCalledWith({
      dictation_shortcut: "Ctrl+Shift+D",
    });
  });
  it("reports failed cancellation rollback without claiming the shortcut is unchanged", async () => {
    let finishSave!: () => void;
    mocks.save
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            finishSave = resolve;
          }),
      )
      .mockRejectedValueOnce(new Error("disk"));
    mount();
    const input = await captureInput();
    fireEvent.keyDown(input, { key: "k", ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(mocks.save).toHaveBeenCalledTimes(1));
    fireEvent.keyDown(input, { key: "Escape" });
    await act(async () => finishSave());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn’t restore your previous shortcut. Try setting it again.",
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
