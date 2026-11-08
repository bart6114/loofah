import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  model: null as unknown,
  start: vi.fn(),
  cancel: vi.fn(),
  toastError: vi.fn(),
  scope: null as null | { client: { canEdit: boolean; features: string[] } },
  prepare: vi.fn(),
  assertReady: vi.fn(),
  dispose: vi.fn(),
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.toastError },
}));

vi.mock("~/ai/hooks", () => ({
  useAITaskTask: () => ({
    isGenerating: false,
    isError: false,
    error: null,
    start: mocks.start,
    cancel: mocks.cancel,
  }),
  useLanguageModel: () => mocks.model,
}));

vi.mock("~/ai/task-window-sync", () => ({
  isMainAITaskHostWindow: () => true,
  requestMainAITaskCancel: vi.fn(),
  requestMainEnhance: vi.fn(),
}));

vi.mock("~/session/queries", () => ({
  useEnhancedDocumentTarget: () => ({
    kind: "template_output",
    document_id: "summary-1",
  }),
}));

vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({ scope: mocks.scope }),
}));

vi.mock("~/shared/application/session-generation-guard", () => ({
  captureSessionGenerationGuard: () => ({
    prepare: mocks.prepare,
    assertReady: mocks.assertReady,
    dispose: mocks.dispose,
  }),
}));
vi.mock("~/session/content-queries", () => ({
  loadSessionContentSnapshot: async () => ({
    rawMarkdown: "Useful source",
    transcripts: [],
  }),
}));

import { useEnhancedNoteActions } from "./enhanced-actions";

describe("useEnhancedNoteActions", () => {
  beforeEach(() => {
    mocks.model = null;
    mocks.scope = {
      client: {
        canEdit: true,
        features: ["document_editor_write", "legacy_desktop_enhance"],
      },
    };
    mocks.prepare.mockReset().mockResolvedValue(undefined);
    mocks.assertReady.mockReset();
    mocks.dispose.mockReset();
    mocks.start.mockReset();
    mocks.cancel.mockReset();
    mocks.toastError.mockReset();
  });

  it("shows a toast without entering an error state when Intelligence is not configured", async () => {
    const { result } = renderHook(() =>
      useEnhancedNoteActions({
        enhancedNoteId: "summary-1",
        sessionId: "session-1",
      }),
    );

    await act(() => result.current.onRegenerate());

    expect(mocks.toastError).toHaveBeenCalledWith(
      "Set up summaries in Settings before regenerating this summary.",
    );
    expect(mocks.start).not.toHaveBeenCalled();
    expect(result.current.isError).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it("refuses regeneration when captured Enhanced edits cannot flush, without dispatching a stale request", async () => {
    mocks.model = {};
    mocks.scope = {
      client: {
        canEdit: true,
        features: ["document_editor_write", "legacy_desktop_enhance"],
      },
    };
    mocks.prepare.mockRejectedValueOnce(new Error("Recover pending edits"));
    const hook = renderHook(() =>
      useEnhancedNoteActions({
        enhancedNoteId: "summary-1",
        sessionId: "session-1",
      }),
    );
    await act(() => hook.result.current.onRegenerate());
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.toastError).toHaveBeenCalledWith("Recover pending edits");
    expect(mocks.dispose).toHaveBeenCalledOnce();
  });
  it("refuses unsupported legacy generation before local source reads or dispatch", async () => {
    mocks.model = {};
    mocks.scope = {
      client: { canEdit: true, features: ["document_editor_write"] },
    };
    const hook = renderHook(() =>
      useEnhancedNoteActions({
        enhancedNoteId: "summary-1",
        sessionId: "session-1",
      }),
    );
    await act(() => hook.result.current.onRegenerate());
    hook.result.current.onCancel();
    expect(mocks.cancel).not.toHaveBeenCalled();
    expect(mocks.prepare).not.toHaveBeenCalled();
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.toastError).toHaveBeenCalledWith(
      "This connection does not support desktop summary generation.",
    );
  });
});
