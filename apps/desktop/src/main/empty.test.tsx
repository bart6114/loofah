import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useAudioImport } from "~/store/zustand/audio-import";

const access = vi.hoisted(() => ({
  legacy: true,
  media: true,
  canEdit: true,
  state: "connected",
  device: null as { canStart: boolean } | null,
  record: vi.fn(),
}));
vi.mock("~/shared/application/device-recording-controls", () => ({
  useDeviceStartActions: () => access.device,
}));

vi.mock("~/shared/main", () => ({
  StandardContentWrapper: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

vi.mock("~/shared/useNewNote", () => ({
  useNoteCreationAvailable: () =>
    access.canEdit && access.state === "connected",
  useNewNote: () => vi.fn(),
  useNewNoteAndListen: () => access.record,
}));

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (selector: (state: { openCurrent: () => void }) => unknown) =>
    selector({
      openCurrent: vi.fn(),
    }),
}));

import { TabContentEmpty } from "./empty";

describe("TabContentEmpty", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    access.legacy = true;
    access.media = true;
    access.canEdit = true;
    access.state = "connected";
    access.device = null;
    useAudioImport.getState().setDialogOpen(false);
  });
  afterEach(() => {
    cleanup();
  });

  it("shows the home actions", () => {
    render(
      <TabContentEmpty
        tab={{
          active: true,
          pinned: false,
          slotId: "slot-home",
          type: "empty",
        }}
      />,
    );

    expect(screen.getByRole("button", { name: /New Note/ })).toBeTruthy();
  });

  it("centers actions in a draggable empty surface while keeping actions clickable", () => {
    render(
      <TabContentEmpty
        tab={{
          active: true,
          pinned: false,
          slotId: "slot-home",
          type: "empty",
        }}
      />,
    );

    const newNoteButton = screen.getByRole("button", { name: /New Note/ });
    const dragSurface = newNoteButton.parentElement?.parentElement;

    expect(dragSurface?.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(dragSurface?.className).not.toContain("mb-12");
    expect(newNoteButton.getAttribute("data-tauri-drag-region")).toBe("false");
    expect(
      screen
        .getByRole("button", { name: /Start Recording/ })
        .getAttribute("data-tauri-drag-region"),
    ).toBe("false");
    expect(
      screen
        .getByRole("button", { name: /Settings/ })
        .getAttribute("data-tauri-drag-region"),
    ).toBe("false");
  });

  it("opens scoped media and device capture without a legacy Local session path", () => {
    access.legacy = false;
    access.device = { canStart: true };
    render(
      <TabContentEmpty
        tab={{
          type: "empty",
          active: true,
          pinned: false,
          slotId: "remote-home",
        }}
      />,
    );
    const record = screen.getByRole("button", { name: /Start Recording/ });
    const audio = screen.getByRole("button", { name: "Import Audio" });
    expect((record as HTMLButtonElement).disabled).toBe(false);
    expect((audio as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(record);
    expect(access.record).toHaveBeenCalledOnce();
    fireEvent.click(audio);
    expect(useAudioImport.getState().dialogOpen).toBe(true);
  });

  it.each(["reader", "disconnected", "unavailable"])(
    "keeps %s media actions disabled",
    (mode) => {
      access.legacy = false;
      access.device = { canStart: false };
      if (mode === "reader") access.canEdit = false;
      if (mode === "disconnected") access.state = "reconnecting";
      if (mode === "unavailable") access.media = false;
      render(
        <TabContentEmpty
          tab={{
            type: "empty",
            active: true,
            pinned: false,
            slotId: "remote-home",
          }}
        />,
      );
      expect(
        (
          screen.getByRole("button", {
            name: /Start Recording/,
          }) as HTMLButtonElement
        ).disabled,
      ).toBe(true);
      const audio = screen.getByRole("button", { name: "Import Audio" });
      expect((audio as HTMLButtonElement).disabled).toBe(true);
      fireEvent.click(audio);
      expect(useAudioImport.getState().dialogOpen).toBe(false);
      expect(access.record).not.toHaveBeenCalled();
    },
  );
});

vi.mock("~/main/use-legacy-activity", async () => {
  const { legacyActivityFixture } =
    await import("~/main/legacy-activity.test-support");
  return {
    useLegacySessionActivity: () => legacyActivityFixture(),
    useLegacyDesktopAccess: () => ({
      ...legacyActivityFixture(),
      isAvailable: () => access.legacy,
      scope: {
        client: {
          canEdit: access.canEdit,
          media: access.media ? {} : undefined,
        },
        getSnapshot: () => ({ state: access.state }),
      },
    }),
  };
});
