import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  prepare: vi.fn(),
  assertReady: vi.fn(),
  dispose: vi.fn(),
  generate: vi.fn(),
  editable: true,
}));
vi.mock("~/ai/contexts", () => ({
  useAITask: (select: (value: unknown) => unknown) =>
    select({ generate: mocks.generate }),
}));
vi.mock("~/ai/hooks", () => ({ useLanguageModel: () => ({}) }));
vi.mock("~/session/queries", () => ({
  useEnhancedDocumentTarget: () => ({
    kind: "template_output",
    document_id: "output",
  }),
}));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({
    scope: {
      client: {
        canEdit: mocks.editable,
        features: ["document_editor_write", "legacy_desktop_enhance"],
      },
    },
  }),
}));
vi.mock("~/shared/application/session-generation-guard", () => ({
  captureSessionGenerationGuard: () => ({
    prepare: mocks.prepare,
    assertReady: mocks.assertReady,
    dispose: mocks.dispose,
  }),
}));

import { EnhanceError } from "./enhance-error";

function mount() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <EnhanceError
        sessionId="same"
        enhancedNoteId="output"
        error={new Error("Original processing failure")}
      />
    </QueryClientProvider>,
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.editable = true;
  mocks.prepare.mockReset().mockResolvedValue(undefined);
  mocks.generate.mockResolvedValue(undefined);
});
afterEach(cleanup);
it("keeps retry pending until the captured target's editors flush, then preserves the existing generation request", async () => {
  let finish!: () => void;
  mocks.prepare.mockImplementationOnce(
    () =>
      new Promise<void>((done) => {
        finish = done;
      }),
  );
  mount();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(mocks.prepare).toHaveBeenCalledOnce());
  expect(mocks.generate).not.toHaveBeenCalled();
  finish();
  await waitFor(() =>
    expect(mocks.generate).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({
        args: { sessionId: "same", templateDocumentId: "output" },
      }),
    ),
  );
  expect(mocks.assertReady).toHaveBeenCalledOnce();
  expect(mocks.dispose).toHaveBeenCalledOnce();
});
it("shows a draft-flush refusal without dispatching retry", async () => {
  mocks.prepare.mockRejectedValueOnce(new Error("Recover pending edits"));
  mount();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByText("Recover pending edits");
  expect(mocks.generate).not.toHaveBeenCalled();
  expect(mocks.dispose).toHaveBeenCalledOnce();
});
it("keeps retry disabled for reader access", () => {
  mocks.editable = false;
  mount();
  expect(
    screen.getByRole("button", { name: "Retry" }).hasAttribute("disabled"),
  ).toBe(true);
  expect(mocks.prepare).not.toHaveBeenCalled();
  expect(mocks.generate).not.toHaveBeenCalled();
});
