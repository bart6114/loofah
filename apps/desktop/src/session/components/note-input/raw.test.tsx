import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RawEditor as SessionRawEditor } from "./raw";

const hoisted = vi.hoisted(() => ({
  rawMd: JSON.stringify({ type: "doc", content: [] }),
  sessionTitle: "Weekly sync",
  saveSessionNote: vi.fn<(...args: unknown[]) => Promise<void>>(() =>
    Promise.resolve(),
  ),
  noteEditorProps: [] as Record<string, unknown>[],
  json2mdStrict: vi.fn(() => "markdown"),
  sonnerToastError: vi.fn(),
}));

vi.mock("@hypr/editor/markdown", () => ({
  parseJsonContent: (value: string) => JSON.parse(value),
  json2mdStrict: hoisted.json2mdStrict,
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

vi.mock("@hypr/plugin-opener2", () => ({
  commands: { openUrl: vi.fn() },
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

vi.mock("~/session/components/shared", () => ({
  hasStoredNoteContent: (value: unknown) => Boolean(value),
}));

vi.mock("~/session/queries", () => ({
  useRefreshSessionNote: () => () => Promise.resolve(),
  saveSessionNote: (...args: unknown[]) => hoisted.saveSessionNote(...args),
}));

vi.mock("~/session/hooks/useAttachmentResolver", () => ({
  useAttachmentResolver: () => () => null,
}));

function RawEditor({
  sessionId,
  className,
}: {
  sessionId: string;
  className?: string;
}) {
  return (
    <SessionRawEditor
      sessionId={sessionId}
      rawMd={hoisted.rawMd}
      sessionTitle={hoisted.sessionTitle}
      className={className}
    />
  );
}

function readInitialDraft() {
  return (
    hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1]
      ?.initialDraft as (() => unknown) | undefined
  )?.();
}

describe("RawEditor", () => {
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    hoisted.noteEditorProps = [];
    hoisted.rawMd = JSON.stringify({ type: "doc", content: [] });
    hoisted.sessionTitle = "Weekly sync";
    hoisted.saveSessionNote.mockReset().mockResolvedValue(undefined);
    hoisted.json2mdStrict.mockReset().mockReturnValue("markdown");
    hoisted.sonnerToastError.mockReset();
  });

  it("uses the shared session note editor styling", () => {
    render(<RawEditor sessionId="session-1" className="custom-editor-class" />);

    const props = hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1];

    expect(props?.className).toContain("session-note-editor");
    expect(props?.className).toContain("custom-editor-class");
    expect(props?.placeholderComponent).toEqual(expect.any(Function));
    expect(props?.initialContent).toMatchObject({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Weekly sync" }],
        },
      ],
    });
  });

  it("shows a persistent toast when saving the note fails", async () => {
    hoisted.saveSessionNote.mockRejectedValue(new Error("disk full"));

    render(<RawEditor sessionId="session-1" />);

    const props = hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1];
    const handleChange = props?.handleChange as (
      input: unknown,
    ) => Promise<void>;
    await act(async () => {
      await expect(handleChange({ type: "doc", content: [] })).rejects.toThrow(
        "disk full",
      );
    });

    await waitFor(() =>
      expect(hoisted.saveSessionNote).toHaveBeenCalledWith(
        "session-1",
        "markdown",
        "",
      ),
    );
    await waitFor(() =>
      expect(hoisted.sonnerToastError).toHaveBeenCalledWith(
        expect.stringContaining("Note is NOT being saved"),
        expect.objectContaining({ id: "note-save-failed:session-1" }),
      ),
    );
  });

  it("reopens the submitted draft while its save and persisted query are still pending", async () => {
    let finish!: () => void;
    hoisted.saveSessionNote.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
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
    const mounted = render(<RawEditor sessionId="pending-session" />);
    const handleChange = hoisted.noteEditorProps[
      hoisted.noteEditorProps.length - 1
    ]?.handleChange as (input: unknown) => Promise<void>;
    let save!: Promise<void>;
    act(() => {
      save = handleChange(input);
    });
    mounted.unmount();
    const reopened = render(<RawEditor sessionId="pending-session" />);
    expect(readInitialDraft()).toEqual(input);
    expect(
      hoisted.noteEditorProps[hoisted.noteEditorProps.length - 1]
        ?.initialContent,
    ).not.toEqual(input);
    await act(async () => {
      finish();
      await save;
    });
    expect(readInitialDraft()).toEqual(input);
    hoisted.rawMd = JSON.stringify(input);
    hoisted.sessionTitle = "Edited title";
    reopened.rerender(<RawEditor sessionId="pending-session" />);
    expect(readInitialDraft()).toBeUndefined();
  });

  it("keeps an unserializable draft without writing an empty note", async () => {
    hoisted.json2mdStrict.mockImplementation(() => {
      throw new Error("Unsupported node");
    });
    const input = { type: "doc", content: [{ type: "unsupported" }] };
    const mounted = render(<RawEditor sessionId="serialization-failure" />);
    const handleChange = hoisted.noteEditorProps[
      hoisted.noteEditorProps.length - 1
    ]?.handleChange as (content: unknown) => Promise<void>;
    await act(async () => {
      await expect(handleChange(input)).rejects.toThrow("Unsupported node");
    });
    expect(hoisted.saveSessionNote).not.toHaveBeenCalled();
    expect(hoisted.sonnerToastError).toHaveBeenCalledWith(
      expect.stringContaining("Note is NOT being saved"),
      { id: "note-save-failed:serialization-failure" },
    );
    mounted.unmount();
    render(<RawEditor sessionId="serialization-failure" />);
    expect(readInitialDraft()).toEqual(input);
  });
});
