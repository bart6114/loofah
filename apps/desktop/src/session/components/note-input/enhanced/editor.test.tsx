import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  target: { kind: "summary" } as
    | import("~/shared/application/protocol.gen").DocumentTarget
    | null
    | undefined,
  documentProps: [] as Record<string, unknown>[],
  previewProps: [] as Record<string, unknown>[],
  toast: vi.fn(),
  scope: { client: { scope: "scope" } },
}));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({ scope: state.scope, error: null }),
}));
vi.mock("~/shared/application/vault-document-editor", () => ({
  VaultDocumentEditor: (props: Record<string, unknown>) => {
    state.documentProps.push(props);
    return <div>Compound document editor</div>;
  },
}));
vi.mock("~/session/queries", () => ({
  useEnhancedDocumentTarget: () => state.target,
}));
vi.mock("@hypr/editor/note", () => ({
  NoteEditor: (props: Record<string, unknown>) => {
    state.previewProps.push(props);
    return <div>Preview editor</div>;
  },
  useMentionNavigation: () => vi.fn(),
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: state.toast },
}));
vi.mock("~/session/hooks/useAttachmentResolver", () => ({
  useAttachmentResolver: () => vi.fn(),
}));
vi.mock("~/editor-bridge/mention-config", () => ({
  useMentionConfig: () => ({}),
}));
vi.mock("~/editor-bridge/app-link-view", () => ({ AppLinkView: () => null }));
vi.mock("~/editor-bridge/session-view", () => ({
  SessionNodeView: () => null,
}));
vi.mock("~/editor-bridge/session-mention-drop", () => ({
  sessionMentionDropConfig: {},
}));
vi.mock("~/editor-bridge/open-editor-link", () => ({
  openEditorLink: vi.fn(),
}));

import { EnhancedEditor } from "./editor";

const props = {
  sessionId: "same",
  sessionTitle: "Session title",
  enhancedNoteId: "same",
  content: "{}",
};
beforeEach(() => {
  state.target = { kind: "summary" };
  state.documentProps = [];
  state.previewProps = [];
  state.toast.mockReset();
});
afterEach(cleanup);
it.each([
  { kind: "summary" },
  { kind: "template_output", document_id: "portable-output" },
] as const)(
  "delegates actual $kind edits to the captured compound editor and preserves host callbacks",
  (target) => {
    state.target = target;
    const onViewReady = vi.fn();
    const onNavigateToTitle = vi.fn();
    const fileHandlerConfig = { onFileUpload: vi.fn() };
    render(
      <EnhancedEditor
        {...props}
        onViewReady={onViewReady}
        onNavigateToTitle={onNavigateToTitle}
        fileHandlerConfig={fileHandlerConfig}
      />,
    );
    expect(screen.getByText("Compound document editor")).toBeDefined();
    expect(state.documentProps[state.documentProps.length - 1]).toMatchObject({
      scope: state.scope,
      sessionId: "same",
      target,
      layout: "document",
      editorOptions: { onViewReady, onNavigateToTitle, fileHandlerConfig },
    });
    expect(state.previewProps).toHaveLength(0);
  },
);
it("blocks ambiguous bare IDs without starting an editor or assuming Summary", () => {
  state.target = null;
  render(<EnhancedEditor {...props} />);
  expect(screen.getByRole("alert").textContent).toContain(
    "cannot be identified safely",
  );
  expect(state.documentProps).toHaveLength(0);
});
it("keeps loading distinct from a missing/ambiguous document", () => {
  state.target = undefined;
  render(<EnhancedEditor {...props} />);
  expect(screen.getByRole("status").textContent).toContain("Loading");
  expect(state.documentProps).toHaveLength(0);
});
it("passes an external generation/edit lock to the compound wrapper", () => {
  render(<EnhancedEditor {...props} readOnly />);
  expect(state.documentProps[state.documentProps.length - 1]?.readOnly).toBe(
    true,
  );
});
it("keeps a content override read-only and syncing without draft/task persistence", () => {
  const preview = {
    type: "doc",
    content: [
      { type: "paragraph", content: [{ type: "text", text: "Streamed" }] },
    ],
  };
  const mounted = render(
    <EnhancedEditor {...props} contentOverride={preview} />,
  );
  expect(state.previewProps[state.previewProps.length - 1]).toMatchObject({
    readOnly: true,
    syncContentWhenFocused: true,
  });
  expect(
    state.previewProps[state.previewProps.length - 1]?.handleChange,
  ).toBeUndefined();
  expect(
    state.previewProps[state.previewProps.length - 1]?.taskSource,
  ).toBeUndefined();
  expect(state.documentProps).toHaveLength(0);
  mounted.rerender(
    <EnhancedEditor {...props} contentOverride={{ ...preview, content: [] }} />,
  );
  expect(
    state.previewProps[state.previewProps.length - 1]?.initialContent,
  ).not.toEqual(state.previewProps[0].initialContent);
});
