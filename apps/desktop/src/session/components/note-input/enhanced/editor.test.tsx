import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { NoteEditorRef } from "@hypr/editor/note";

import { EnhancedEditor as SessionEnhancedEditor } from "./editor";

import {
  enhancedNoteDraftKey,
  hasUnsavedSessionNoteDrafts,
  discardNoteDraft,
} from "~/session/pending-note-drafts";
import { sessionEditorFlushKey } from "~/shared/application/session-cleanup-guard";
import {
  enqueueDatabaseWrite,
  flushDatabaseWrites,
} from "~/shared/write-queue";

const hoisted = vi.hoisted(() => ({
  content: JSON.stringify({ type: "doc", content: [] }),
  sessionTitle: "Weekly sync",
  persistContent: vi.fn(() => Promise.resolve()),
  noteEditorProps: [] as Record<string, unknown>[],
  sonnerToastError: vi.fn(),
  scope: "enhanced:default",
}));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => ({
    scope: {
      client: { scope: hoisted.scope, canEdit: true },
      getSnapshot: () => ({ state: "connected" }),
    },
  }),
}));

vi.mock("@hypr/editor/markdown", () => ({
  parseJsonContent: (value: string) => JSON.parse(value),
}));

vi.mock("@hypr/editor/note", () => ({
  areEquivalentEditorContents: (left: unknown, right: unknown) =>
    JSON.stringify(left) === JSON.stringify(right),
  normalizePortableAttachmentUrls: (value: unknown) => value,
  NoteEditor: (props: Record<string, unknown>) => {
    hoisted.noteEditorProps.push(props);

    return <div>Note editor</div>;
  },
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: hoisted.sonnerToastError },
}));

vi.mock("~/session/hooks/useAttachmentResolver", () => ({
  useAttachmentResolver: () => () => null,
}));

vi.mock("~/editor-bridge/app-link-view", () => ({
  AppLinkView: () => null,
}));

vi.mock("~/editor-bridge/mention-config", () => ({
  useMentionConfig: () => ({ users: [] }),
}));

vi.mock("~/editor-bridge/open-editor-link", () => ({
  openEditorLink: vi.fn(),
}));

vi.mock("~/editor-bridge/session-mention-drop", () => ({
  sessionMentionDropConfig: { read: () => null },
}));

vi.mock("~/editor-bridge/session-view", () => ({
  SessionNodeView: () => null,
}));

vi.mock("~/session/queries", () => ({
  useRefreshEnhancedNote: () => () => Promise.resolve(),
  useEnhancedNote: () => ({ content: hoisted.content }),
  useUpdateEnhancedNoteContent: () => hoisted.persistContent,
}));

function EnhancedEditor(
  props: Omit<
    React.ComponentProps<typeof SessionEnhancedEditor>,
    "sessionTitle"
  >,
) {
  return (
    <SessionEnhancedEditor {...props} sessionTitle={hoisted.sessionTitle} />
  );
}

function readInitialDraft() {
  return (
    hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1]
      ?.initialDraft as (() => unknown) | undefined
  )?.();
}

describe("EnhancedEditor", () => {
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    hoisted.noteEditorProps = [];
    hoisted.scope = "enhanced:default";
    hoisted.sonnerToastError.mockClear();
    hoisted.content = JSON.stringify({ type: "doc", content: [] });
    hoisted.sessionTitle = "Weekly sync";
    hoisted.persistContent = vi.fn(() => Promise.resolve());
  });

  it("flushes pre-debounce content and waits for its captured write when cleanup starts", async () => {
    let finish!: () => void;
    hoisted.persistContent = vi.fn(() =>
      enqueueDatabaseWrite(
        "session:cleanup-summary",
        () =>
          new Promise<void>((resolve) => {
            finish = resolve;
          }),
      ),
    );
    const mounted = render(
      <EnhancedEditor
        sessionId="cleanup-summary"
        enhancedNoteId="summary"
        content={hoisted.content}
      />,
    );
    const props = hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1];
    const input = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "First 500ms summary" }],
        },
      ],
    };
    (props.onDraftChange as (getContent: () => unknown) => void)(() => input);
    let saving!: Promise<unknown>;
    const flush = vi.fn(() => {
      saving = (props.handleChange as (content: unknown) => Promise<unknown>)(
        input,
      );
    });
    (props.ref as (value: NoteEditorRef) => void)({
      flushPendingChanges: flush,
    } as unknown as NoteEditorRef);
    expect(hasUnsavedSessionNoteDrafts(hoisted.scope, "cleanup-summary")).toBe(
      true,
    );
    let completed = false;
    const preparation = flushDatabaseWrites([
      sessionEditorFlushKey(hoisted.scope, "cleanup-summary"),
    ]).then(() => {
      completed = true;
    });
    await vi.waitFor(() =>
      expect(hoisted.persistContent).toHaveBeenCalledOnce(),
    );
    expect(flush).toHaveBeenCalledOnce();
    expect(completed).toBe(false);
    mounted.unmount();
    await act(async () => {
      finish();
      await saving;
      await preparation;
    });
    expect(completed).toBe(true);
    expect(hasUnsavedSessionNoteDrafts(hoisted.scope, "cleanup-summary")).toBe(
      false,
    );
    discardNoteDraft(
      enhancedNoteDraftKey(hoisted.scope, "cleanup-summary", "summary"),
    );
  });

  it("does not reuse a pending enhanced draft from an identical ID in a different scope", () => {
    hoisted.scope = "enhanced:first";
    const props = {
      sessionId: "same-note",
      enhancedNoteId: "same-output",
      content: hoisted.content,
    };
    const first = render(<EnhancedEditor {...props} />);
    const input = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "First scope private" }],
        },
      ],
    };
    (
      hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1]
        .onDraftChange as (capture: () => unknown) => void
    )(() => input);
    first.unmount();
    hoisted.scope = "enhanced:second";
    render(<EnhancedEditor {...props} />);
    expect(readInitialDraft()).toBeUndefined();
    expect(hasUnsavedSessionNoteDrafts("enhanced:first", "same-note")).toBe(
      true,
    );
    expect(hasUnsavedSessionNoteDrafts("enhanced:second", "same-note")).toBe(
      false,
    );
    discardNoteDraft(
      enhancedNoteDraftKey("enhanced:first", "same-note", "same-output"),
    );
  });

  it("shows the session title as the first line for persisted notes", () => {
    hoisted.content = JSON.stringify({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Summary Section" }],
        },
      ],
    });

    render(
      <EnhancedEditor
        sessionId="session-1"
        enhancedNoteId="note-1"
        content={hoisted.content}
      />,
    );

    const props = hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1];

    expect(props?.className).toContain("session-note-editor");
    expect(props?.className).toContain("enhanced-summary-editor");
    expect(props?.placeholderComponent).toEqual(expect.any(Function));
    expect(props?.syncContentWhenFocused).toBe(false);
    expect(props?.handleChange).not.toBe(hoisted.persistContent);
    expect(props?.taskSource).toEqual({ type: "enhanced_note", id: "note-1" });
    expect(props?.initialContent).toMatchObject({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Weekly sync" }],
        },
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Summary Section" }],
        },
      ],
    });
  });

  it("does not rerender the editor when its props are unchanged", () => {
    const view = render(
      <EnhancedEditor
        sessionId="session-1"
        enhancedNoteId="note-1"
        content={hoisted.content}
      />,
    );

    view.rerender(
      <EnhancedEditor
        sessionId="session-1"
        enhancedNoteId="note-1"
        content={hoisted.content}
      />,
    );

    expect(hoisted.noteEditorProps).toHaveLength(1);
  });

  it("persists content and updates the session title from the first line", async () => {
    render(
      <EnhancedEditor
        sessionId="session-1"
        enhancedNoteId="note-1"
        content={hoisted.content}
      />,
    );

    const props = hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1];
    const input = {
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Edited title" }],
        },
      ],
    };

    await act(async () => {
      await (props?.handleChange as (input: unknown) => Promise<void>)(input);
    });

    expect(hoisted.persistContent).toHaveBeenCalledWith(
      JSON.stringify(input),
      "Edited title",
    );
  });

  it("keeps streamed previews syncing while focused", () => {
    const contentOverride = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "Generating" }] },
      ],
    };

    render(
      <EnhancedEditor
        sessionId="session-1"
        enhancedNoteId="note-1"
        content={hoisted.content}
        contentOverride={contentOverride}
      />,
    );

    const props = hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1];

    expect(props?.syncContentWhenFocused).toBe(true);
    expect(props?.handleChange).toBeUndefined();
    expect(props?.taskSource).toBeUndefined();
    expect(props?.initialContent).toMatchObject({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Weekly sync" }],
        },
        {
          type: "paragraph",
          content: [{ type: "text", text: "Generating" }],
        },
      ],
    });
  });

  it("retains a failed summary draft when reopened without affecting preview content", async () => {
    hoisted.persistContent.mockRejectedValue(new Error("disk full"));
    const props = {
      sessionId: "failed-session",
      enhancedNoteId: "failed-summary",
      content: hoisted.content,
    };
    const input = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Unsaved summary" }],
        },
      ],
    };
    const mounted = render(<EnhancedEditor {...props} />);
    const handleChange = hoisted.noteEditorProps[
      hoisted.noteEditorProps.length - 1
    ]?.handleChange as (input: unknown) => Promise<void>;
    await act(async () => {
      await expect(handleChange(input)).rejects.toThrow("disk full");
    });
    expect(hoisted.sonnerToastError).toHaveBeenCalledWith(
      expect.stringContaining("Summary is NOT being saved"),
      { id: "summary-save-failed:failed-summary" },
    );
    mounted.unmount();
    const reopened = render(<EnhancedEditor {...props} />);
    expect(readInitialDraft()).toEqual(input);
    reopened.rerender(<EnhancedEditor {...props} contentOverride={input} />);
    expect(readInitialDraft()).toBeUndefined();
    expect(
      hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1]?.handleChange,
    ).toBeUndefined();
  });
});

it("passes read-only to the editor and removes attachment drop configuration", () => {
  render(
    <EnhancedEditor
      sessionId="locked"
      enhancedNoteId="locked"
      content='{"type":"doc","content":[]}'
      readOnly
      fileHandlerConfig={{ onFileUpload: vi.fn() }}
    />,
  );
  const props = hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1];
  expect(props?.readOnly).toBe(true);
  expect(props?.fileHandlerConfig).toBeUndefined();
  expect(props?.sessionMentionDropConfig).toBeUndefined();
  cleanup();
});
