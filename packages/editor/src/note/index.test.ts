// @vitest-environment jsdom

import {
  act,
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@testing-library/react";
import { EditorState, TextSelection } from "prosemirror-state";
import type { EditorView } from "prosemirror-view";
import { createElement, createRef, StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { md2json } from "../markdown";
import {
  TaskStorageProvider,
  createInMemoryTaskStorage,
} from "../task-storage";
import { extractTasksFromContent } from "../tasks";
import type { JSONContent, NoteEditorRef } from "./index";
import {
  createReadOnlyPlugin,
  getEditorCompositionWaitMs,
  NoteEditor,
  shouldReplaceEditorContent,
} from "./index";
import { schema } from "./schema";

const baseDoc: JSONContent = {
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text: "old" }] }],
};

const nextDoc: JSONContent = {
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text: "new" }] }],
};

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("shouldReplaceEditorContent", () => {
  it("does not replace content while IME composition is active", () => {
    expect(
      shouldReplaceEditorContent({
        currentContent: baseDoc,
        nextContent: nextDoc,
        hasFocus: true,
        isComposing: true,
        syncContentWhenFocused: true,
      }),
    ).toBe(false);
  });

  it("allows focused content sync after composition ends when enabled", () => {
    expect(
      shouldReplaceEditorContent({
        currentContent: baseDoc,
        nextContent: nextDoc,
        hasFocus: true,
        isComposing: false,
        syncContentWhenFocused: true,
      }),
    ).toBe(true);
  });
});

describe("getEditorCompositionWaitMs", () => {
  it("waits through active IME composition", () => {
    expect(
      getEditorCompositionWaitMs(
        { composing: false },
        { active: true, endedAt: 0 },
      ),
    ).toBe(500);
  });

  it("returns the remaining post-composition grace window", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1000);

    expect(
      getEditorCompositionWaitMs(
        { composing: false },
        { active: false, endedAt: 600 },
      ),
    ).toBe(100);
  });

  it("returns zero after the post-composition grace window", () => {
    vi.useFakeTimers();
    vi.setSystemTime(1000);

    expect(
      getEditorCompositionWaitMs(
        { composing: false },
        { active: false, endedAt: 499 },
      ),
    ).toBe(0);
  });

  it("returns zero for a reset inactive composition state", () => {
    expect(
      getEditorCompositionWaitMs(
        { composing: false },
        { active: false, endedAt: 0 },
      ),
    ).toBe(0);
  });
});

describe("createReadOnlyPlugin", () => {
  it("rejects document changes while allowing selection changes", () => {
    const plugin = createReadOnlyPlugin();
    const state = EditorState.create({
      schema,
      doc: schema.node("doc", null, [
        schema.node("paragraph", null, [schema.text("shared")]),
      ]),
      plugins: [plugin],
    });

    expect(
      state.applyTransaction(state.tr.insertText("blocked")).transactions,
    ).toHaveLength(0);

    const selection = TextSelection.create(state.doc, 2);
    expect(
      state.applyTransaction(state.tr.setSelection(selection)).transactions,
    ).toHaveLength(1);
  });

  it("wires the editor surface to reject document changes", async () => {
    let view: EditorView | null = null;
    const handleChange = vi.fn();
    const rendered = render(
      createElement(NoteEditor, {
        initialContent: {
          type: "doc",
          content: [
            {
              type: "paragraph",
              content: [{ type: "text", text: "shared" }],
            },
          ],
        },
        handleChange,
        onViewReady: (nextView) => {
          view = nextView;
        },
        readOnly: true,
      }),
    );

    await waitFor(() => expect(view).not.toBeNull());
    const surface = rendered.getByRole("document");
    expect(surface.getAttribute("contenteditable")).toBe("false");
    expect(surface.getAttribute("aria-readonly")).toBe("true");

    act(() => {
      view?.dispatch(view.state.tr.insertText("blocked", 2));
    });

    expect(view?.state.doc.textContent).toBe("shared");
    expect(handleChange).not.toHaveBeenCalled();
  });

  it("hides attachment mutation controls in read-only documents", async () => {
    const rendered = render(
      createElement(NoteEditor, {
        initialContent: {
          type: "doc",
          content: [
            {
              type: "image",
              attrs: { src: "https://example.com/image.png" },
            },
            {
              type: "fileAttachment",
              attrs: {
                name: "notes.pdf",
                mimeType: "application/pdf",
                src: "https://example.com/notes.pdf",
                path: "https://example.com/notes.pdf",
              },
            },
          ],
        },
        readOnly: true,
      }),
    );

    await waitFor(() => expect(rendered.getByText("notes.pdf")).not.toBeNull());
    expect(
      rendered.queryByRole("button", { name: "Resize image from left" }),
    ).toBeNull();
    expect(
      rendered.queryByRole("button", { name: "Resize image from right" }),
    ).toBeNull();
    expect(
      rendered.queryByRole("button", { name: "Remove attachment" }),
    ).toBeNull();
  });
});

describe("browser-safe editor controls", () => {
  it("renders an image upload started at the caret", async () => {
    const ref = createRef<NoteEditorRef>();
    let resolveUpload!: (value: {
      attachmentId: string;
      path: string;
      url: string;
    }) => void;
    const fileHandlerConfig = {
      onFileUpload: vi.fn(
        () =>
          new Promise<{
            attachmentId: string;
            path: string;
            url: string;
          }>((resolve) => {
            resolveUpload = resolve;
          }),
      ),
    };
    const rendered = render(
      createElement(
        StrictMode,
        null,
        createElement(NoteEditor, {
          ref,
          initialContent: {
            type: "doc",
            content: [
              { type: "heading", attrs: { level: 1 } },
              { type: "paragraph" },
            ],
          },
          fileHandlerConfig,
        }),
      ),
    );
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    const file = new File(["png"], "diagram.png", { type: "image/png" });

    fireEvent.paste(rendered.getByRole("textbox"), {
      clipboardData: {
        files: [file],
        getData: () => "",
        items: [],
      },
    });

    expect(rendered.getByText("diagram.png")).not.toBeNull();
    await act(async () => {
      resolveUpload({
        attachmentId: "diagram.png",
        path: "/vault/attachments/diagram.png",
        url: "asset:/diagram.png",
      });
    });
    await waitFor(() => {
      expect(ref.current?.view?.state.doc.child(1).type).toBe(
        schema.nodes.image,
      );
    });
    expect(ref.current?.view?.state.selection.$from.parent.type).toBe(
      schema.nodes.paragraph,
    );
    expect(ref.current?.view?.state.selection.$from.index(0)).toBe(2);
  });

  it("flushes the current document through the change handler immediately", async () => {
    const ref = createRef<NoteEditorRef>();
    const handleChange = vi.fn();
    render(
      createElement(NoteEditor, {
        ref,
        initialContent: baseDoc,
        handleChange,
        enforceTitleHeading: false,
      }),
    );

    await waitFor(() => expect(ref.current?.view).not.toBeNull());

    act(() => ref.current?.flushPendingChanges());

    expect(handleChange).toHaveBeenCalledOnce();
    expect(handleChange).toHaveBeenCalledWith(baseDoc);
  });

  it("cancels the original debounce after callback-changing rerenders", async () => {
    const ref = createRef<NoteEditorRef>();
    const handleChange = vi.fn();
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(
      createElement(NoteEditor, {
        ...props,
        taskSource: { type: "session_raw_note", id: "session-1" },
      }),
    );
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();

    act(() => {
      const view = ref.current?.view;
      view?.dispatch(view.state.tr.insertText(" first", 4));
    });
    rendered.rerender(
      createElement(NoteEditor, {
        ...props,
        taskSource: { type: "session_raw_note", id: "session-1" },
      }),
    );
    act(() => {
      const view = ref.current?.view;
      view?.dispatch(view.state.tr.insertText(" second", 4));
    });
    const currentBody = ref.current?.view?.state.doc.toJSON();

    act(() => ref.current?.flushPendingChanges());
    await act(() => vi.advanceTimersByTimeAsync(500));

    expect(handleChange).toHaveBeenCalledOnce();
    expect(handleChange).toHaveBeenCalledWith(currentBody);
  });

  it("flushes a pending change before disposing the editor", async () => {
    const ref = createRef<NoteEditorRef>();
    const events: string[] = [];
    const handleChange = vi.fn(() => events.push("persist"));
    const onViewDisposed = vi.fn(() => events.push("dispose"));
    const rendered = render(
      createElement(NoteEditor, {
        ref,
        initialContent: baseDoc,
        handleChange,
        onViewDisposed,
        enforceTitleHeading: false,
      }),
    );
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();

    act(() => {
      const view = ref.current?.view;
      view?.dispatch(view.state.tr.insertText(" pending", 4));
    });
    const pendingBody = ref.current?.view?.state.doc.toJSON();

    act(() => rendered.unmount());

    expect(handleChange).toHaveBeenCalledOnce();
    expect(handleChange).toHaveBeenCalledWith(pendingBody);
    expect(onViewDisposed).toHaveBeenCalledOnce();
    expect(events).toEqual(["persist", "dispose"]);

    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(handleChange).toHaveBeenCalledOnce();
  });

  it("does not persist an unchanged document when disposing the editor", async () => {
    const ref = createRef<NoteEditorRef>();
    const handleChange = vi.fn();
    const rendered = render(
      createElement(NoteEditor, {
        ref,
        initialContent: baseDoc,
        handleChange,
        enforceTitleHeading: false,
      }),
    );
    await waitFor(() => expect(ref.current?.view).not.toBeNull());

    act(() => rendered.unmount());

    expect(handleChange).not.toHaveBeenCalled();
  });

  it("does not mount the slash command surface when disabled", async () => {
    let view: EditorView | null = null;
    const rendered = render(
      createElement(NoteEditor, {
        initialContent: {
          type: "doc",
          content: [
            {
              type: "heading",
              attrs: { level: 1 },
              content: [{ type: "text", text: "Title" }],
            },
            {
              type: "paragraph",
              content: [{ type: "text", text: "/" }],
            },
          ],
        },
        onViewReady: (nextView) => {
          view = nextView;
        },
        showSlashCommand: false,
      }),
    );

    await waitFor(() => expect(view).not.toBeNull());
    act(() => {
      if (!view) return;
      view.dispatch(
        view.state.tr.setSelection(TextSelection.create(view.state.doc, 9)),
      );
    });

    expect(rendered.queryByText("Commands")).toBeNull();
  });
});

it("keeps completed Markdown tasks completed when creating a real editor view", async () => {
  const ref = createRef<NoteEditorRef>();
  render(
    createElement(NoteEditor, {
      ref,
      initialContent: md2json("- [x] Completed task"),
      enforceTitleHeading: false,
    }),
  );
  await waitFor(() => expect(ref.current?.view).not.toBeNull());
  expect(
    extractTasksFromContent(ref.current!.view!.state.doc.toJSON(), {
      type: "session_raw_note",
      id: "session",
    }),
  ).toMatchObject([{ status: "done" }]);
});

it("matches saved task identities before assigning identities to Markdown tasks", async () => {
  const ref = createRef<NoteEditorRef>();
  const source = { type: "session_raw_note", id: "session" };
  const storage = createInMemoryTaskStorage();
  storage.upsertTasksForSource(source, [
    {
      taskId: "saved-task",
      sourceType: source.type,
      sourceId: source.id,
      sourceOrder: 0,
      status: "done",
      textPreview: "Completed task",
      dueDate: "2026-10-01",
      body: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "Completed task" }],
        },
      ],
    },
  ]);
  render(
    createElement(TaskStorageProvider, {
      storage,
      children: createElement(NoteEditor, {
        ref,
        taskSource: source,
        initialContent: md2json("- [x] Completed task"),
        enforceTitleHeading: false,
      }),
    }),
  );
  await waitFor(() => expect(ref.current?.view).not.toBeNull());
  const tasks = extractTasksFromContent(
    ref.current!.view!.state.doc.toJSON(),
    source,
  );
  expect(tasks).toHaveLength(1);
  expect(tasks[0]).toMatchObject({ taskId: "saved-task", status: "done" });
});
