import {
  act,
  cleanup,
  fireEvent,
  render,
  waitFor,
} from "@testing-library/react";
import { redo, undo } from "prosemirror-history";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NoteEditor, type NoteEditorRef } from "./index";
import { handleNativeEditorDrag } from "./native-drag";

beforeEach(() => {
  vi.stubGlobal(
    "DataTransfer",
    class {
      files = [];
      data = new Map<string, string>();
      clearData() {
        this.data.clear();
      }
      setData(type: string, value: string) {
        this.data.set(type, value);
      }
      getData(type: string) {
        return this.data.get(type) ?? "";
      }
    },
  );
  vi.stubGlobal(
    "DragEvent",
    class extends MouseEvent {
      dataTransfer: DataTransfer | null;
      constructor(type: string, init: DragEventInit = {}) {
        super(type, init);
        this.dataTransfer = init.dataTransfer ?? null;
      }
    },
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("native editor drag", () => {
  it.each(["fileAttachment", "image"])(
    "moves a %s forward and backward, saves its attributes, and supports undo",
    async (type) => {
      const ref = createRef<NoteEditorRef>();
      const onFileUpload = vi.fn();
      const handleChange = vi.fn();
      render(
        <NoteEditor
          ref={ref}
          enforceTitleHeading={false}
          showFormatToolbar={false}
          showSlashCommand={false}
          handleChange={handleChange}
          fileHandlerConfig={{ onFileUpload }}
          initialContent={{
            type: "doc",
            content: [
              {
                type: "paragraph",
                content: [{ type: "text", text: "Before" }],
              },
              {
                type,
                attrs: {
                  attachmentId: "report.pdf",
                  name: "report.pdf",
                  src: "attachments/report.pdf",
                  path: "/tmp/report.pdf",
                  mimeType: "application/pdf",
                  editorWidth: 50,
                },
              },
              { type: "paragraph", content: [{ type: "text", text: "After" }] },
              { type: "paragraph" },
            ],
          }}
        />,
      );
      await waitFor(() => expect(ref.current?.view).toBeTruthy());
      const view = ref.current!.view!;
      const original = view.state.doc;
      const attachment = original.child(1);
      const start = original.child(0).nodeSize;
      const coords = vi.spyOn(view, "posAtCoords");
      vi.spyOn(view, "coordsAtPos").mockReturnValue({
        left: 0,
        right: 0,
        top: 0,
        bottom: 0,
      });

      act(() => {
        fireEvent(
          view.nodeDOM(start)!,
          new DragEvent("dragstart", {
            bubbles: true,
            dataTransfer: new DataTransfer(),
          }),
        );
      });
      expect(view.dragging?.slice.content.firstChild).toEqual(attachment);
      coords.mockReturnValue({ pos: original.content.size - 2, inside: -1 });

      act(() => {
        handleNativeEditorDrag(view, { type: "drop", point: { x: 20, y: 80 } });
        ref.current!.flushPendingChanges();
      });

      expect(view.state.doc.child(1).textContent).toBe("After");
      expect(view.state.doc.child(2).eq(attachment)).toBe(true);
      expect(view.dragging).toBeNull();
      expect(handleChange.mock.lastCall?.[0]).toEqual(view.state.doc.toJSON());
      expect(onFileUpload).not.toHaveBeenCalled();

      act(() => {
        undo(view.state, view.dispatch);
      });
      expect(view.state.doc.eq(original)).toBe(true);
      act(() => {
        redo(view.state, view.dispatch);
      });
      expect(view.state.doc.child(2).eq(attachment)).toBe(true);

      coords.mockReturnValue(null);
      act(() => {
        fireEvent(
          view.nodeDOM(start + original.child(2).nodeSize)!,
          new DragEvent("dragstart", {
            bubbles: true,
            dataTransfer: new DataTransfer(),
          }),
        );
      });
      coords.mockReturnValue({ pos: 0, inside: -1 });
      act(() => {
        handleNativeEditorDrag(view, { type: "drop", point: { x: 20, y: 10 } });
      });
      expect(view.state.doc.firstChild?.eq(attachment)).toBe(true);
    },
  );

  it("forwards hover and leave to the drop cursor without importing a file", () => {
    const dom = document.createElement("div");
    const dispatch = vi.spyOn(dom, "dispatchEvent");
    const view = { editable: true, dragging: { move: false }, dom } as any;

    handleNativeEditorDrag(view, { type: "over", point: { x: 12, y: 34 } });
    expect(dispatch.mock.calls[0][0]).toMatchObject({
      type: "dragover",
      clientX: 12,
      clientY: 34,
      altKey: true,
    });
    view.dragging = null;
    handleNativeEditorDrag(view, { type: "leave" });
    expect(dispatch.mock.calls[1][0].type).toBe("dragleave");
  });

  it("ignores drops without an editor drag and in read-only editors", () => {
    const dom = document.createElement("div");
    const dispatch = vi.spyOn(dom, "dispatchEvent");
    const view = { editable: true, dragging: null, dom } as any;
    handleNativeEditorDrag(view, { type: "drop", point: { x: 1, y: 2 } });
    view.editable = false;
    view.dragging = { move: true };
    handleNativeEditorDrag(view, { type: "drop", point: { x: 1, y: 2 } });
    expect(dispatch).not.toHaveBeenCalled();
  });
});
