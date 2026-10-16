import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { EditorView } from "~/store/zustand/tabs/schema";

const mocks = vi.hoisted(() => ({
  leftsidebar: {
    expanded: true,
    toggleExpanded: vi.fn(),
  },
  canGoBack: false,
  canGoNext: false,
  goBack: vi.fn(),
  goNext: vi.fn(),
  sessionModes: {} as Record<string, string>,
  startListening: vi.fn(),
  stopListening: vi.fn(),
  stopTranscription: vi.fn(),
  requestMainListenerControl: vi.fn(),
  isMainWebviewWindow: true,
  audioExists: false,
  hasTranscriptBySession: {} as Record<string, boolean>,
  overflowProps: [] as Array<{
    allowListening?: boolean;
    standaloneWindow?: boolean;
  }>,
}));

vi.mock("./overflow", () => ({
  OverflowButton: (props: {
    allowListening?: boolean;
    standaloneWindow?: boolean;
  }) => {
    mocks.overflowProps.push(props);
    return <button type="button">More</button>;
  },
}));

vi.mock("../shared", () => ({
  RecordingIcon: () => <div data-testid="recording-icon" />,
  useHasTranscript: (sessionId: string) =>
    mocks.hasTranscriptBySession[sessionId] ?? false,
}));

vi.mock("~/audio-player", () => ({
  useAudioPlayer: () => ({ audioExists: mocks.audioExists }),
}));

vi.mock("~/contexts/shell", () => ({
  useShell: () => ({
    leftsidebar: mocks.leftsidebar,
  }),
}));

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: vi.fn((selector: (state: unknown) => unknown) =>
    selector({
      canGoBack: mocks.canGoBack,
      canGoNext: mocks.canGoNext,
      goBack: mocks.goBack,
      goNext: mocks.goNext,
    }),
  ),
}));

vi.mock("~/stt/contexts", () => ({
  useListener: vi.fn((selector: (state: unknown) => unknown) =>
    selector({
      getSessionMode: (sessionId: string) =>
        mocks.sessionModes[sessionId] ?? "inactive",
      canStartLiveSession: (sessionId: string) =>
        (mocks.sessionModes[sessionId] ?? "inactive") === "inactive",
      stop: mocks.stopListening,
      stopTranscription: mocks.stopTranscription,
    }),
  ),
}));

vi.mock("~/stt/useStartListening", () => ({
  useStartListening: () => mocks.startListening,
}));

vi.mock("~/stt/window-control", () => ({
  isMainWebviewWindow: () => mocks.isMainWebviewWindow,
  requestMainListenerControl: mocks.requestMainListenerControl,
}));

import { OuterHeader } from "./index";

describe("OuterHeader", () => {
  beforeEach(() => {
    mocks.leftsidebar.expanded = true;
    mocks.leftsidebar.toggleExpanded.mockClear();
    mocks.canGoBack = false;
    mocks.canGoNext = false;
    mocks.goBack.mockClear();
    mocks.goNext.mockClear();
    mocks.sessionModes = {};
    mocks.startListening.mockClear();
    mocks.stopListening.mockClear();
    mocks.stopTranscription.mockClear();
    mocks.requestMainListenerControl.mockClear();
    mocks.isMainWebviewWindow = true;
    mocks.audioExists = false;
    mocks.hasTranscriptBySession = {};
    mocks.overflowProps = [];
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("does not show a separate stop listening button for active sessions while the sidebar is collapsed", () => {
    mocks.leftsidebar.expanded = false;
    mocks.sessionModes = { "session-1": "active" };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const title = screen.getByText("Session title");
    const titleSlot = title.parentElement?.parentElement;

    expect(screen.queryByRole("button", { name: "Stop listening" })).toBeNull();
    expect(titleSlot?.className).toContain("right-[140px]");
    expect(titleSlot?.className).not.toContain("right-[153px]");
  });

  it("shows a disabled finalizing state while the sidebar is collapsed", () => {
    mocks.leftsidebar.expanded = false;
    mocks.sessionModes = { "session-1": "finalizing" };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const title = screen.getByText("Session title");
    const titleSlot = title.parentElement?.parentElement;

    const finalizingButton = screen.getByRole("button", { name: "Finalizing" });
    expect((finalizingButton as HTMLButtonElement).disabled).toBe(true);
    expect(titleSlot?.className).toContain("right-[140px]");
    expect(titleSlot?.className).not.toContain("right-[153px]");
  });

  it("aligns the title and actions with window controls when the sidebar is collapsed", () => {
    mocks.leftsidebar.expanded = false;

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const title = screen.getByText("Session title");
    const titleWrapper = title.parentElement;
    const titleSlot = titleWrapper?.parentElement;
    const header = titleSlot?.parentElement;

    expect(header?.className).toContain(
      "pl-[calc(var(--traffic-lights-inset)_+_80px)]",
    );
    expect(header?.className).toContain(
      "h-[calc(var(--sidebar-chrome-center-y)*2)]",
    );
    expect(header?.className).not.toContain("pb-1");
    expect(titleWrapper?.classList.contains("w-full")).toBe(true);
    expect(titleWrapper?.className).toContain("max-w-full");
    expect(titleWrapper?.className).not.toContain("max-w-[680px]");
    expect(titleSlot?.className).toContain(
      "left-[calc(var(--traffic-lights-inset)_+_28px)]",
    );
    expect(titleSlot?.className).not.toContain("-translate-y-1");
    expect(titleSlot?.className).toContain("right-[140px]");
    expect(screen.queryByRole("button", { name: "Show sidebar" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Go back" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Go forward" })).toBeNull();
  });

  it("uses a compact title offset while the sidebar is expanded", () => {
    mocks.leftsidebar.expanded = true;

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const title = screen.getByText("Session title");
    const titleSlot = title.parentElement?.parentElement;

    expect(titleSlot?.className).toContain("left-0");
    expect(titleSlot?.className).toContain("right-[140px]");
    expect(titleSlot?.className).not.toContain("justify-center");
  });

  it("can center the title slot for toolbar controls", () => {
    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        centerTitle
        title={<span>Toolbar controls</span>}
      />,
    );

    const title = screen.getByText("Toolbar controls");
    const titleSlot = title.parentElement?.parentElement;

    expect(titleSlot?.className).toContain("justify-center");
    expect(title.parentElement?.classList.contains("w-full")).toBe(false);
  });

  it("keeps sidebar header controls hidden while the sidebar is expanded", () => {
    mocks.sessionModes = { "session-1": "active" };

    const { container } = render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    expect(screen.queryByRole("button", { name: "Hide sidebar" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Go back" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Go forward" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop listening" })).toBeNull();
    expect(container.firstElementChild?.className).not.toContain(
      "pl-[calc(var(--traffic-lights-inset)_+_80px)]",
    );
  });

  it("keeps the expanded session header at its existing height", () => {
    const { container } = render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    expect(container.firstElementChild?.className).toContain("h-12");
  });

  it("marks the structural title and action strip as draggable", () => {
    const { container } = render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const header = container.firstElementChild;
    const title = screen.getByText("Session title");
    const titleWrapper = title.parentElement;
    const titleSlot = titleWrapper?.parentElement;
    const actionStrip = header?.lastElementChild;

    expect(header?.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(titleSlot?.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(titleWrapper?.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(actionStrip?.hasAttribute("data-tauri-drag-region")).toBe(true);
  });

  it("keeps the dedicated stop button hidden while the sidebar is expanded", () => {
    mocks.sessionModes = { "session-1": "active" };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    expect(screen.queryByRole("button", { name: "Stop listening" })).toBeNull();
  });

  it("does not show a separate stop button in standalone windows", () => {
    mocks.leftsidebar.expanded = true;
    mocks.sessionModes = { "session-1": "active" };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        standaloneWindow
        title={<span>Session title</span>}
      />,
    );

    const title = screen.getByText("Session title");
    const titleSlot = title.parentElement?.parentElement;

    expect(titleSlot?.className).toContain(
      "left-[var(--traffic-lights-inset)]",
    );
    expect(titleSlot?.className).toContain("right-[140px]");
    expect(titleSlot?.className).not.toContain("right-[153px]");
    expect(screen.queryByRole("button", { name: "Stop listening" })).toBeNull();

    const overflowProps = mocks.overflowProps[mocks.overflowProps.length - 1];
    expect(overflowProps?.standaloneWindow).toBe(true);
    expect(overflowProps?.allowListening).toBeUndefined();
  });

  it("does not reserve collapsed sidebar gutter in standalone windows", () => {
    mocks.leftsidebar.expanded = false;

    const { container } = render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        standaloneWindow
        title={<span>Session title</span>}
      />,
    );

    const title = screen.getByText("Session title");
    const titleSlot = title.parentElement?.parentElement;
    const header = container.firstElementChild;

    expect(header?.className).not.toContain(
      "pl-[calc(var(--traffic-lights-inset)_+_80px)]",
    );
    expect(titleSlot?.className).toContain(
      "left-[var(--traffic-lights-inset)]",
    );
    expect(titleSlot?.className).toContain("right-[140px]");
  });

  it("shows record for an inactive session with no prior transcript or audio", () => {
    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Record" }));

    expect(mocks.startListening).toHaveBeenCalledTimes(1);
  });

  it("shows resume when an inactive session already has a transcript", () => {
    mocks.hasTranscriptBySession = { "session-1": true };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "transcript" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const resumeButton = screen.getByRole("button", { name: "Resume" });

    fireEvent.click(resumeButton);

    expect(resumeButton.title).toBe("Resume listening");
    expect(screen.queryByRole("button", { name: "Record" })).toBeNull();
    expect(screen.getByTestId("recording-icon")).not.toBeNull();
    expect(mocks.startListening).toHaveBeenCalledTimes(1);
  });

  it("shows resume when an inactive session has audio without a transcript", () => {
    mocks.audioExists = true;

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "transcript" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const resumeButton = screen.getByRole("button", { name: "Resume" });

    fireEvent.click(resumeButton);

    expect(resumeButton.title).toBe("Resume listening");
    expect(screen.queryByRole("button", { name: "Record" })).toBeNull();
    expect(mocks.startListening).toHaveBeenCalledTimes(1);
  });

  it("shows transcribing instead of stop while post-capture transcription runs", () => {
    mocks.sessionModes = { "session-1": "running_batch" };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    const transcribingButton = screen.getByRole("button", {
      name: "Transcribing",
    });

    expect(transcribingButton.title).toBe("Stop transcription");
    fireEvent.click(transcribingButton);

    expect(mocks.stopTranscription).toHaveBeenCalledTimes(1);
    expect(mocks.stopListening).not.toHaveBeenCalled();
  });

  it("disables the record button while a stopped session is finalizing", () => {
    mocks.sessionModes = { "session-1": "finalizing" };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const finalizingButton = screen.getByRole("button", { name: "Finalizing" });

    expect((finalizingButton as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(finalizingButton);

    expect(mocks.stopListening).not.toHaveBeenCalled();
    expect(mocks.startListening).not.toHaveBeenCalled();
  });

  it("shows stop while a session is actively listening", () => {
    mocks.sessionModes = { "session-1": "active" };

    render(
      <OuterHeader
        sessionId="session-1"
        currentView={{ type: "raw" } as EditorView}
        title={<span>Session title</span>}
      />,
    );

    const stopButton = screen.getByRole("button", { name: "Stop" });

    fireEvent.click(stopButton);

    expect(stopButton.querySelector("svg")?.getAttribute("class")).toContain(
      "text-recording",
    );
    expect(mocks.stopListening).toHaveBeenCalledTimes(1);
  });
});
