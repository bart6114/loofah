// @vitest-environment jsdom

import {
  act,
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@testing-library/react";
import { undoDepth } from "prosemirror-history";
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
  it("renders an external replacement and keeps subsequent typing and saves on that document", async () => {
    const ref = createRef<NoteEditorRef>();
    const handleChange = vi.fn();
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
      showFormatToolbar: false,
      showSlashCommand: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(rendered.getByRole("textbox").textContent).toBe("new");
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
    expect(handleChange).not.toHaveBeenCalled();
    act(() => {
      const view = ref.current!.view!;
      view.dispatch(view.state.tr.insertText(" edited", 4));
      ref.current!.flushPendingChanges();
    });
    expect(rendered.getByRole("textbox").textContent).toBe("new edited");
    expect(handleChange).toHaveBeenCalledOnce();
    expect(handleChange.mock.calls[0][0]).toEqual(md2json("new edited"));
  });

  it("accepts a fresh post-save snapshot when query delivery skipped the saved echo", async () => {
    const ref = createRef<NoteEditorRef>();
    let finish!: (readContent: () => JSONContent) => void;
    const handleChange = vi.fn(
      () =>
        new Promise<() => JSONContent>((resolve) => {
          finish = resolve;
        }),
    );
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();
    act(() =>
      ref.current!.view!.dispatch(
        ref.current!.view!.state.tr.insertText(" saved", 4),
      ),
    );
    await act(() => vi.advanceTimersByTimeAsync(500));
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("old saved");
    await act(async () => finish(() => nextDoc));
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
  });

  it("defers a confirmed external edit while focused and applies it on blur", async () => {
    const ref = createRef<NoteEditorRef>();
    const handleChange = vi.fn(async () => () => nextDoc);
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();
    act(() => {
      const view = ref.current!.view!;
      view.focus();
      view.dispatch(view.state.tr.insertText(" saved", 4));
    });
    await act(() => vi.advanceTimersByTimeAsync(500));
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("old saved");
    await act(async () => ref.current!.view!.dom.blur());
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
  });

  it("reads the latest confirmed source when props skip past the refreshed snapshot and never replays stale props", async () => {
    const ref = createRef<NoteEditorRef>();
    let finish!: (readContent: () => JSONContent) => void;
    let source = nextDoc;
    const handleChange = vi.fn(
      () =>
        new Promise<() => JSONContent>((resolve) => {
          finish = resolve;
        }),
    );
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();
    act(() => {
      const view = ref.current!.view!;
      view.dispatch(view.state.tr.insertText(" saved", 4));
    });
    await act(() => vi.advanceTimersByTimeAsync(500));
    source = md2json("Latest external C");
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: source }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("old saved");
    await act(async () => finish(() => source));
    expect(ref.current!.view!.state.doc.textContent).toBe("Latest external C");
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("Latest external C");

    source = md2json("Latest external D");
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: source }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("Latest external D");
  });

  it("does not release newer local typing when an older save returns a source reader", async () => {
    const ref = createRef<NoteEditorRef>();
    let finish!: (readContent: () => JSONContent) => void;
    const handleChange = vi.fn(
      () =>
        new Promise<() => JSONContent>((resolve) => {
          finish = resolve;
        }),
    );
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();
    act(() => {
      const view = ref.current!.view!;
      view.dispatch(view.state.tr.insertText(" A", 4));
    });
    await act(() => vi.advanceTimersByTimeAsync(500));
    act(() => {
      const view = ref.current!.view!;
      view.dispatch(view.state.tr.insertText(" B", 6));
    });
    await act(async () => finish(() => nextDoc));
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("old A B");
    expect(undoDepth(ref.current!.view!.state)).toBeGreaterThan(0);
  });

  it("keeps newer typing and undo through older save echoes, then accepts clean external edits", async () => {
    const ref = createRef<NoteEditorRef>();
    const completions: (() => void)[] = [];
    const handleChange = vi.fn(
      () => new Promise<void>((resolve) => completions.push(resolve)),
    );
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();

    act(() => {
      const view = ref.current!.view!;
      view.focus();
      view.dispatch(view.state.tr.insertText(" first", 4));
    });
    const first = ref.current!.view!.state.doc.toJSON();
    await act(() => vi.advanceTimersByTimeAsync(500));
    act(() => {
      const view = ref.current!.view!;
      view.dispatch(
        view.state.tr.insertText(" SECOND", view.state.doc.content.size - 1),
      );
      view.dom.blur();
    });
    const second = ref.current!.view!.state.doc.toJSON();
    const historyBefore = undoDepth(ref.current!.view!.state);
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: first }),
    );
    await act(async () => completions[0]());
    expect(ref.current!.view!.state.doc.toJSON()).toEqual(second);
    expect(undoDepth(ref.current!.view!.state)).toBe(historyBefore);

    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(handleChange).toHaveBeenCalledTimes(2);
    // The persisted query can arrive before its save promise settles.
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: second }),
    );
    expect(undoDepth(ref.current!.view!.state)).toBe(historyBefore);
    await act(async () => completions[1]());
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
  });

  it("protects submitted edits until the newest save is acknowledged", async () => {
    const ref = createRef<NoteEditorRef>();
    const completions: (() => void)[] = [];
    const handleChange = vi.fn(
      () => new Promise<void>((resolve) => completions.push(resolve)),
    );
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();
    act(() =>
      ref.current!.view!.dispatch(
        ref.current!.view!.state.tr.insertText(" A", 4),
      ),
    );
    const first = ref.current!.view!.state.doc.toJSON();
    await act(() => vi.advanceTimersByTimeAsync(500));
    act(() =>
      ref.current!.view!.dispatch(
        ref.current!.view!.state.tr.insertText(" B", 6),
      ),
    );
    const second = ref.current!.view!.state.doc.toJSON();
    await act(() => vi.advanceTimersByTimeAsync(500));
    await act(async () => completions[0]());
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: first }),
    );
    expect(ref.current!.view!.state.doc.toJSON()).toEqual(second);
    await act(async () => completions[1]());
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.toJSON()).toEqual(second);
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: second }),
    );
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
  });

  it("keeps failed edits when a refresh arrives", async () => {
    const ref = createRef<NoteEditorRef>();
    const handleChange = vi.fn().mockRejectedValue(new Error("disk full"));
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();
    act(() =>
      ref.current!.view!.dispatch(
        ref.current!.view!.state.tr.insertText(" unsaved", 4),
      ),
    );
    await act(() => vi.advanceTimersByTimeAsync(500));
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("old unsaved");
    expect(undoDepth(ref.current!.view!.state)).toBeGreaterThan(0);
  });

  it("starts a remounted editor from its draft and waits for persisted content", async () => {
    const ref = createRef<NoteEditorRef>();
    const handleChange = vi.fn();
    const props = {
      ref,
      initialContent: baseDoc,
      initialDraft: nextDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: { ...baseDoc } }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
    rendered.rerender(
      createElement(NoteEditor, {
        ...props,
        initialContent: nextDoc,
        initialDraft: undefined,
      }),
    );
    rendered.rerender(
      createElement(NoteEditor, {
        ...props,
        initialContent: baseDoc,
        initialDraft: undefined,
      }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("old");
    expect(handleChange).not.toHaveBeenCalled();
  });

  it("continues accepting focused preview updates without a persistence handler", async () => {
    const ref = createRef<NoteEditorRef>();
    const props = {
      ref,
      initialContent: baseDoc,
      syncContentWhenFocused: true,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    act(() => {
      const view = ref.current!.view!;
      view.focus();
      view.dispatch(view.state.tr.insertText(" preview edit", 4));
    });
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: nextDoc }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("new");
  });

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

  it("does not save an unchanged document when asked to flush", async () => {
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

    expect(handleChange).not.toHaveBeenCalled();
  });

  it("does not overwrite a deferred external edit when a clean focused editor is flushed for a tab switch", async () => {
    const ref = createRef<NoteEditorRef>();
    let source = baseDoc;
    const handleChange = vi.fn(async () => () => source);
    const props = {
      ref,
      initialContent: baseDoc,
      handleChange,
      enforceTitleHeading: false,
    };
    const rendered = render(createElement(NoteEditor, props));
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    vi.useFakeTimers();
    act(() => {
      const view = ref.current!.view!;
      view.focus();
      view.dispatch(view.state.tr.insertText(" saved", 4));
      source = view.state.doc.toJSON();
    });
    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(handleChange).toHaveBeenCalledOnce();
    source = nextDoc;
    rendered.rerender(
      createElement(NoteEditor, { ...props, initialContent: source }),
    );
    expect(ref.current!.view!.state.doc.textContent).toBe("old saved");
    act(() => ref.current!.flushPendingChanges());
    rendered.unmount();
    expect(handleChange).toHaveBeenCalledOnce();
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
