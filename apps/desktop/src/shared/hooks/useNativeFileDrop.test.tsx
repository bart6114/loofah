import { act, cleanup, render, waitFor } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  handler: null as ((event: { payload: any }) => void) | null,
  unlisten: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn(async (handler) => {
      mocks.handler = handler;
      return mocks.unlisten;
    }),
  }),
}));

import {
  isPointInsideElement,
  nativeDragPointToCssPoint,
  useNativeFileDrop,
} from "./useNativeFileDrop";

beforeEach(() => {
  mocks.handler = null;
  mocks.unlisten.mockClear();
});

afterEach(cleanup);

describe("useNativeFileDrop", () => {
  it("forwards drags without file paths without showing the file hover state", async () => {
    const onDrop = vi.fn();
    const onHoverPaths = vi.fn();
    const onInternalDrag = vi.fn();
    const view = render(
      <Harness
        onDrop={onDrop}
        onHoverPaths={onHoverPaths}
        onInternalDrag={onInternalDrag}
      />,
    );
    await waitFor(() => expect(mocks.handler).not.toBeNull());
    vi.spyOn(
      view.getByTestId("target"),
      "getBoundingClientRect",
    ).mockReturnValue({
      left: 0,
      top: 0,
      right: 100,
      bottom: 100,
    } as DOMRect);

    act(() => {
      for (const type of ["enter", "over", "drop"]) {
        mocks.handler?.({
          payload: { type, paths: [], position: { x: 20, y: 40 } },
        });
      }
      mocks.handler?.({ payload: { type: "leave" } });
    });

    expect(onInternalDrag.mock.calls).toEqual([
      [{ type: "over", point: { x: 20, y: 40 } }],
      [{ type: "over", point: { x: 20, y: 40 } }],
      [{ type: "drop", point: { x: 20, y: 40 } }],
      [{ type: "leave" }],
    ]);
    expect(view.getByTestId("hover").textContent).toBe("no");
    expect(onDrop).not.toHaveBeenCalled();
    expect(onHoverPaths).not.toHaveBeenCalled();

    act(() => {
      mocks.handler?.({
        payload: { type: "drop", paths: [], position: { x: 200, y: 40 } },
      });
    });
    expect(onInternalDrag.mock.lastCall).toEqual([{ type: "leave" }]);
  });
  it("uses native macOS points as CSS coordinates and hit-tests the target", () => {
    expect(nativeDragPointToCssPoint({ x: 40, y: 20 })).toEqual({
      x: 40,
      y: 20,
    });
    const element = document.createElement("div");
    vi.spyOn(element, "getBoundingClientRect").mockReturnValue({
      left: 10,
      top: 10,
      right: 50,
      bottom: 30,
    } as DOMRect);
    expect(isPointInsideElement(element, { x: 40, y: 20 })).toBe(true);
    expect(isPointInsideElement(element, { x: 60, y: 20 })).toBe(false);
  });

  it("tracks hover, preserves path order, resets, and cleans up", async () => {
    const onDrop = vi.fn();
    const onHoverPaths = vi.fn();
    const view = render(
      <Harness onDrop={onDrop} onHoverPaths={onHoverPaths} />,
    );
    await waitFor(() => expect(mocks.handler).not.toBeNull());
    const target = view.getByTestId("target");
    vi.spyOn(target, "getBoundingClientRect").mockReturnValue({
      left: 0,
      top: 0,
      right: 100,
      bottom: 100,
    } as DOMRect);

    act(() => {
      mocks.handler?.({
        payload: {
          type: "enter",
          paths: ["/two", "/one"],
          position: { x: 100, y: 100 },
        },
      });
    });
    expect(view.getByTestId("hover").textContent).toBe("yes");
    expect(onHoverPaths).toHaveBeenCalledWith(["/two", "/one"]);

    act(() => {
      mocks.handler?.({
        payload: {
          type: "drop",
          paths: ["/two", "/one"],
          position: { x: 100, y: 100 },
        },
      });
    });
    expect(onDrop).toHaveBeenCalledOnce();
    expect(onDrop).toHaveBeenCalledWith(["/two", "/one"], {
      x: 100,
      y: 100,
    });
    expect(view.getByTestId("hover").textContent).toBe("no");

    view.unmount();
    expect(mocks.unlisten).toHaveBeenCalledOnce();
  });
});

function Harness({
  onDrop,
  onHoverPaths,
  onInternalDrag,
}: {
  onDrop: (paths: string[], point: { x: number; y: number }) => void;
  onHoverPaths: (paths: string[]) => void;
  onInternalDrag?: Parameters<typeof useNativeFileDrop>[1]["onInternalDrag"];
}) {
  const ref = useRef<HTMLDivElement>(null);
  const { isHovering } = useNativeFileDrop(ref, {
    onDrop,
    onHoverPaths,
    onInternalDrag,
  });
  return (
    <div ref={ref} data-testid="target">
      <span data-testid="hover">{isHovering ? "yes" : "no"}</span>
    </div>
  );
}
