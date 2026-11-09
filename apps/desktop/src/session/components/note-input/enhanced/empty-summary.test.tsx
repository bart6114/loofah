import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  source: true,
  target: { kind: "summary" } as object | null | undefined,
  scope: { client: { features: ["summary_body_jobs", "generated_titles"] } },
  panel: vi.fn(),
  local: vi.fn(() => {
    throw new Error("No Local provider calls from main generation controls");
  }),
}));
vi.mock("~/session/hooks/useSummarySource", () => ({
  useSummarySource: () => mocks.source,
}));
vi.mock("~/session/queries", () => ({
  useEnhancedDocumentTarget: () => mocks.target,
}));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({ scope: mocks.scope }),
}));
vi.mock("~/ai/hooks", () => ({ useLLMConnectionStatus: mocks.local }));
vi.mock("~/services/enhancer", () => ({ getEnhancerService: mocks.local }));
vi.mock("~/shared/application/vault-processing", () => ({
  VaultProcessing: (props: unknown) => {
    mocks.panel(props);
    return <div>Scoped generation controls</div>;
  },
}));
import { EmptySummary } from "./empty-summary";

afterEach(cleanup);
beforeEach(() => {
  vi.clearAllMocks();
  mocks.source = true;
  mocks.target = { kind: "summary" };
  mocks.scope.client.features = ["summary_body_jobs", "generated_titles"];
});
it.each([{ extra: [] }, { extra: ["legacy_desktop_enhance"] }])(
  "uses the same scoped jobs with extra capabilities %j",
  ({ extra }) => {
    mocks.scope.client.features.push(...extra);
    render(<EmptySummary sessionId="note" sessionTitle="Title" />);
    expect(mocks.panel).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: mocks.scope,
        sessionId: "note",
        target: { kind: "summary" },
        defaultTitlePolicy: "generate_if_empty",
        sourceAvailable: true,
      }),
    );
    expect(mocks.local).not.toHaveBeenCalled();
  },
);
it("explains empty source and disables generation without creating a summary", () => {
  mocks.source = false;
  render(<EmptySummary sessionId="note" sessionTitle="Title" />);
  expect(
    screen.getByText("Add a note or transcript to generate a summary."),
  ).toBeTruthy();
  expect(mocks.panel).toHaveBeenCalledWith(
    expect.objectContaining({ sourceAvailable: false }),
  );
});
it("reports unavailable shared capabilities without falling back to the legacy provider", () => {
  mocks.scope.client.features = ["legacy_desktop_enhance"];
  render(<EmptySummary sessionId="note" sessionTitle="Title" />);
  expect(screen.getByRole("status").textContent).toContain("unavailable");
  expect(mocks.panel).not.toHaveBeenCalled();
  expect(mocks.local).not.toHaveBeenCalled();
});
it("refuses a missing template target", () => {
  mocks.target = null;
  render(
    <EmptySummary
      sessionId="note"
      sessionTitle="Title"
      enhancedNoteId="output"
    />,
  );
  expect(screen.getByRole("alert").textContent).toContain("not available");
  expect(mocks.panel).not.toHaveBeenCalled();
});
