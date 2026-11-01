import type { EditorView } from "prosemirror-view";

export type NativeEditorDragEvent =
  | { type: "over" | "drop"; point: { x: number; y: number } }
  | { type: "leave" };

export function handleNativeEditorDrag(
  view: EditorView,
  event: NativeEditorDragEvent,
) {
  if (!view.editable || (event.type !== "leave" && !view.dragging)) {
    return;
  }

  // Tauri consumes macOS drops even without file paths. Reuse ProseMirror's
  // drag handlers and drop cursor for the editor's existing dragged slice.
  view.dom.dispatchEvent(
    new DragEvent(
      event.type === "leave"
        ? "dragleave"
        : event.type === "over"
          ? "dragover"
          : "drop",
      {
        bubbles: true,
        cancelable: true,
        ...(event.type !== "leave"
          ? {
              clientX: event.point.x,
              clientY: event.point.y,
              dataTransfer: new DataTransfer(),
              altKey: !view.dragging?.move,
            }
          : {}),
      },
    ),
  );
}
