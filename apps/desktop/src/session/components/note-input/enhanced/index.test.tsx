import { cleanup, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  scope: { client: { scope: "owner", features: ["summary_body_jobs"] } },
  target: { kind: "summary" } as object | null | undefined,
  content: "Canonical saved summary",
  exists: true,
  panel: vi.fn(),
  model: vi.fn(() => {
    throw new Error("Main summary must not read a Local model");
  }),
}));
vi.mock("~/ai/hooks", () => ({
  useAITaskTask: mocks.model,
  useLanguageModel: mocks.model,
}));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({ scope: mocks.scope }),
}));
vi.mock("~/session/hooks/useSummarySource", () => ({
  useSummarySource: () => true,
}));
vi.mock("~/session/queries", () => ({
  useEnhancedNote: () => (mocks.exists ? { content: mocks.content } : null),
  useEnhancedDocumentTarget: () => mocks.target,
}));
vi.mock("~/shared/application/vault-processing", () => ({
  VaultProcessing: (props: unknown) => {
    mocks.panel(props);
    return <div>Shared jobs</div>;
  },
}));
vi.mock("./editor", () => ({
  EnhancedEditor: ({ content }: { content: string }) => {
    const [mountId] = useState(() => crypto.randomUUID());
    return (
      <div data-testid="editor" data-mount-id={mountId}>
        {content}
      </div>
    );
  },
}));
import { Enhanced } from "./index";

beforeEach(() => {
  vi.clearAllMocks();
  mocks.exists = true;
  mocks.content = "Canonical saved summary";
  mocks.target = { kind: "summary" };
});
afterEach(cleanup);
const view = () => (
  <Enhanced
    sessionId="session"
    sessionTitle="Saved title"
    enhancedNoteId="session"
  />
);

it("mounts shared scoped processing with legacy-compatible automatic empty-title generation", () => {
  render(view());
  expect(mocks.panel).toHaveBeenCalledWith(
    expect.objectContaining({
      scope: mocks.scope,
      sessionId: "session",
      target: { kind: "summary" },
      defaultTitlePolicy: "generate_if_empty",
    }),
  );
  expect(screen.getByTestId("editor").textContent).toBe(
    "Canonical saved summary",
  );
  expect(mocks.model).not.toHaveBeenCalled();
});
it("retains the compound editor across empty content and canonical job updates", () => {
  mocks.exists = false;
  const mounted = render(view());
  const mountId = screen.getByTestId("editor").dataset.mountId;
  mocks.exists = true;
  mocks.content = "";
  mounted.rerender(view());
  expect(screen.getByTestId("editor").dataset.mountId).toBe(mountId);
  mocks.content = "New canonical result";
  mounted.rerender(view());
  expect(screen.getByTestId("editor").dataset.mountId).toBe(mountId);
  expect(screen.getByText("New canonical result")).toBeTruthy();
});
it("keeps device recovery mounted while the document target is loading", () => {
  mocks.target = undefined;
  render(view());
  expect(screen.getByTestId("editor")).toBeTruthy();
  expect(mocks.panel).not.toHaveBeenCalled();
});
it("refuses ambiguous targets without mounting generation or an editable document", () => {
  mocks.target = null;
  render(view());
  expect(screen.getByRole("alert").textContent).toContain(
    "cannot be identified safely",
  );
  expect(screen.queryByTestId("editor")).toBeNull();
  expect(mocks.panel).not.toHaveBeenCalled();
});
it("passes the exact selected template output identity to shared processing", () => {
  mocks.target = { kind: "template_output", document_id: "output" };
  render(view());
  expect(mocks.panel).toHaveBeenCalledWith(
    expect.objectContaining({ target: mocks.target }),
  );
});
