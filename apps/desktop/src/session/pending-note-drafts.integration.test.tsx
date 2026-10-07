import { act, cleanup, render, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, expect, it, vi } from "vitest";

import {
  NoteEditor,
  type JSONContent,
  type EditorSaveResult,
  type NoteEditorRef,
} from "@hypr/editor/note";

import {
  captureNoteDraft,
  persistNoteDraft,
  usePendingNoteDraft,
} from "./pending-note-drafts";

afterEach(cleanup);

function doc(text: string): JSONContent {
  return {
    type: "doc",
    content: [{ type: "paragraph", content: [{ type: "text", text }] }],
  };
}

it("reopens the real editor with its blocked save draft while persisted props remain stale", async () => {
  const key = "real-editor-remount";
  const ref = createRef<NoteEditorRef>();
  let finish!: () => void;
  const write = new Promise<void>((resolve) => {
    finish = resolve;
  });
  let save: Promise<EditorSaveResult> | undefined;
  const handleChange = (content: JSONContent) => {
    save = persistNoteDraft(key, content, () => write);
    return save;
  };
  function Editor({ persisted }: { persisted: JSONContent }) {
    const { initialDraft, confirmedDraft } = usePendingNoteDraft(
      key,
      persisted,
    );
    return (
      <NoteEditor
        ref={ref}
        initialContent={persisted}
        initialDraft={initialDraft}
        confirmedDraft={confirmedDraft}
        handleChange={handleChange}
        onDraftChange={(getContent) => captureNoteDraft(key, getContent)}
        enforceTitleHeading={false}
        showFormatToolbar={false}
        showSlashCommand={false}
      />
    );
  }

  const original = doc("old");
  const mounted = render(<Editor persisted={original} />);
  await waitFor(() => expect(ref.current?.view).not.toBeNull());
  act(() => {
    const view = ref.current!.view!;
    view.dispatch(view.state.tr.insertText(" new", 4));
    ref.current!.flushPendingChanges();
  });
  expect(ref.current?.view?.state.doc.textContent).toBe("old new");
  mounted.unmount();

  const reopened = render(<Editor persisted={doc("old")} />);
  await waitFor(() => expect(ref.current?.view).not.toBeNull());
  expect(ref.current?.view?.state.doc.textContent).toBe("old new");
  reopened.rerender(<Editor persisted={doc("older query result")} />);
  expect(ref.current?.view?.state.doc.textContent).toBe("old new");

  await act(async () => {
    finish();
    await save;
  });
  expect(ref.current?.view?.state.doc.textContent).toBe("old new");
  reopened.rerender(<Editor persisted={doc("old new")} />);
  reopened.rerender(
    <Editor persisted={doc("external edit after acknowledgment")} />,
  );
  expect(ref.current?.view?.state.doc.textContent).toBe(
    "external edit after acknowledgment",
  );
});

it("captures a draft before a keyed replacement renders and before debounce or unmount flush", async () => {
  const draftKey = "keyed-real-editor-remount";
  const ref = createRef<NoteEditorRef>();
  const persist = vi.fn(() => Promise.resolve());
  function Editor() {
    const original = doc("old");
    const { initialDraft, confirmedDraft } = usePendingNoteDraft(
      draftKey,
      original,
    );
    return (
      <NoteEditor
        ref={ref}
        initialContent={original}
        initialDraft={initialDraft}
        confirmedDraft={confirmedDraft}
        onDraftChange={(getContent) => captureNoteDraft(draftKey, getContent)}
        handleChange={(content) => persistNoteDraft(draftKey, content, persist)}
        enforceTitleHeading={false}
        showFormatToolbar={false}
        showSlashCommand={false}
      />
    );
  }
  const mounted = render(<Editor key="first-mount" />);
  await waitFor(() => expect(ref.current?.view).not.toBeNull());
  act(() => {
    const view = ref.current!.view!;
    view.dispatch(view.state.tr.insertText(" latest typing", 4));
  });
  expect(persist).not.toHaveBeenCalled();
  mounted.rerender(<Editor key="second-mount" />);
  expect(ref.current?.view?.state.doc.textContent).toBe("old latest typing");
  await act(async () => {
    await Promise.resolve();
  });
  expect(persist).toHaveBeenCalledOnce();
});

it("releases a restored pending draft when its unchanged persisted props already match the completed save", async () => {
  const key = "already-delivered-restored-draft";
  const ref = createRef<NoteEditorRef>();
  const saved = doc("already delivered saved content");
  let source = saved;
  let finish!: () => void;
  const write = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const priorSave = persistNoteDraft(key, saved, async () => {
    await write;
    return () => source;
  });
  const handleChange = vi.fn(async () => () => source);
  function Editor({ persisted }: { persisted: JSONContent }) {
    const { initialDraft, confirmedDraft } = usePendingNoteDraft(
      key,
      persisted,
    );
    return (
      <NoteEditor
        ref={ref}
        initialContent={persisted}
        initialDraft={initialDraft}
        confirmedDraft={confirmedDraft}
        handleChange={handleChange}
        enforceTitleHeading={false}
        showFormatToolbar={false}
        showSlashCommand={false}
      />
    );
  }
  const mounted = render(<Editor persisted={saved} />);
  await waitFor(() => expect(ref.current?.view).not.toBeNull());
  await act(async () => {
    finish();
    await priorSave;
  });
  act(() => ref.current!.view!.focus());
  source = doc("external update after restoration");
  mounted.rerender(<Editor persisted={source} />);
  expect(mounted.getByRole("textbox").textContent).toBe(
    "already delivered saved content",
  );
  act(() => ref.current!.flushPendingChanges());
  expect(handleChange).not.toHaveBeenCalled();
  await act(async () => ref.current!.view!.dom.blur());
  expect(mounted.getByRole("textbox").textContent).toBe(
    "external update after restoration",
  );
});

it.each([false, true])(
  "releases a restored draft when its original save confirms a newer source (changed generation: %s)",
  async (changedGeneration) => {
    const key = `restored-draft-skipped-echo-${changedGeneration}`;
    const ref = createRef<NoteEditorRef>();
    const submitted = doc("submitted B");
    let source = submitted;
    let finish!: () => void;
    const write = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const priorSave = persistNoteDraft(
      key,
      submitted,
      async () => {
        await write;
        return () => source;
      },
      "old-generation",
    );
    const handleChange = vi.fn(async () => () => source);
    function Editor({ persisted }: { persisted: JSONContent }) {
      const { initialDraft, confirmedDraft } = usePendingNoteDraft(
        key,
        persisted,
        changedGeneration ? "new-generation" : "old-generation",
      );
      return (
        <NoteEditor
          ref={ref}
          initialContent={persisted}
          initialDraft={initialDraft}
          confirmedDraft={confirmedDraft}
          handleChange={handleChange}
          enforceTitleHeading={false}
          showFormatToolbar={false}
          showSlashCommand={false}
        />
      );
    }
    const mounted = render(<Editor persisted={doc("old A")} />);
    await waitFor(() => expect(ref.current?.view).not.toBeNull());
    source = doc("external C after save");
    mounted.rerender(<Editor persisted={source} />);
    await act(async () => {
      finish();
      await priorSave;
    });
    expect(mounted.getByRole("textbox").textContent).toBe(
      "external C after save",
    );
    act(() => ref.current!.flushPendingChanges());
    expect(handleChange).not.toHaveBeenCalled();
  },
);
