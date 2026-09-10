import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  hasSource: false,
  model: null as unknown,
  main: true,
  enhance: vi.fn(),
  remote: vi.fn(),
}));
vi.mock("~/ai/hooks", () => ({
  useLLMConnectionStatus: () =>
    mocks.model
      ? { status: "success", providerId: "chatgpt" }
      : { status: "pending", reason: "missing_provider" },
}));
vi.mock("~/ai/task-window-sync", () => ({
  isMainAITaskHostWindow: () => mocks.main,
  requestMainEnhance: mocks.remote,
}));
vi.mock("~/services/enhancer", () => ({
  getEnhancerService: () => ({ enhance: mocks.enhance }),
}));
vi.mock("~/session/hooks/useSummarySource", () => ({
  useSummarySource: () => mocks.hasSource,
}));
vi.mock("~/session/queries", () => ({ useEnhancedNote: () => null }));
vi.mock("~/shared/config", () => ({ useConfigValue: () => "template-1" }));
vi.mock("./config-error", () => ({
  ConfigError: () => <div>Set up Intelligence</div>,
}));
vi.mock("../header", () => ({
  TemplatePickerPopover: ({
    onSelectTemplate,
  }: {
    onSelectTemplate: (selection: {
      templateId: string;
      title: string;
    }) => void;
  }) => (
    <button
      onClick={() =>
        onSelectTemplate({ templateId: "decisions", title: "Decisions" })
      }
    >
      Choose template
    </button>
  ),
}));
import { EmptySummary } from "./empty-summary";

function mount() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <EmptySummary sessionId="session-1" sessionTitle="Planning" />
    </QueryClientProvider>,
  );
}

describe("EmptySummary", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.hasSource = false;
    mocks.model = {};
    mocks.main = true;
    mocks.enhance.mockResolvedValue({ type: "started", noteId: "summary-1" });
    mocks.remote.mockResolvedValue({ type: "started", noteId: "summary-1" });
  });
  afterEach(cleanup);
  it("is accessible without creating a document and disables empty generation", () => {
    mount();
    expect(
      screen.getByText("Add a note or transcript to generate a summary."),
    ).toBeTruthy();
    expect(
      screen
        .getByRole("button", { name: "Generate summary" })
        .hasAttribute("disabled"),
    ).toBe(true);
    expect(mocks.enhance).not.toHaveBeenCalled();
  });
  it("shows setup when source exists but Intelligence is missing", () => {
    mocks.hasSource = true;
    mocks.model = null;
    mount();
    expect(screen.getByText("Set up Intelligence")).toBeTruthy();
  });
  it.each([true, false])(
    "starts from source in main window: %s",
    async (main) => {
      mocks.hasSource = true;
      mocks.main = main;
      mount();
      fireEvent.click(screen.getByRole("button", { name: "Generate summary" }));
      await waitFor(() =>
        expect(main ? mocks.enhance : mocks.remote).toHaveBeenCalledWith(
          "session-1",
          { templateId: "template-1", targetNoteId: undefined },
        ),
      );
      expect(main ? mocks.remote : mocks.enhance).not.toHaveBeenCalled();
    },
  );
  it("generates with the chosen template before a document exists", async () => {
    mocks.hasSource = true;
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Choose template" }));
    await waitFor(() =>
      expect(mocks.enhance).toHaveBeenCalledWith("session-1", {
        templateId: "decisions",
        templateTitle: "Decisions",
        targetNoteId: undefined,
      }),
    );
  });
  it("shows start failures and allows retry", async () => {
    mocks.hasSource = true;
    mocks.enhance.mockRejectedValueOnce(new Error("Try again"));
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Generate summary" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe("Try again"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate summary" }));
    await waitFor(() => expect(mocks.enhance).toHaveBeenCalledTimes(2));
  });
});
