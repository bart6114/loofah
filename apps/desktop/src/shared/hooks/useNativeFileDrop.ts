import { isTauri } from "@tauri-apps/api/core";
import type { PhysicalPosition } from "@tauri-apps/api/dpi";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { platform } from "@tauri-apps/plugin-os";
import { useEffect, useRef, useState, type RefObject } from "react";

import type { NativeEditorDragEvent } from "@hypr/editor/note";

export type NativeDropPoint = { x: number; y: number };

export function useNativeFileDrop(
  targetRef: RefObject<HTMLElement | null>,
  callbacks: {
    onDrop: (paths: string[], point: NativeDropPoint) => void;
    onHoverPaths?: (paths: string[]) => void;
    onHoverEnd?: () => void;
    onInternalDrag?: (event: NativeEditorDragEvent) => void;
  },
) {
  const callbacksRef = useRef(callbacks);
  callbacksRef.current = callbacks;
  const hoveringRef = useRef(false);
  const pathsRef = useRef<string[]>([]);
  const [isHovering, setIsHovering] = useState(false);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }

    let disposed = false;
    let unlisten: (() => void) | null = null;

    void getCurrentWebview()
      .onDragDropEvent(({ payload }) => {
        if (disposed) return;

        if (payload.type === "leave") {
          hoveringRef.current = false;
          pathsRef.current = [];
          setIsHovering(false);
          callbacksRef.current.onHoverEnd?.();
          callbacksRef.current.onInternalDrag?.({ type: "leave" });
          return;
        }

        const point = nativeDragPointToCssPoint(payload.position);
        const inside = isPointInsideElement(targetRef.current, point);
        if (payload.type === "enter") {
          pathsRef.current = [...payload.paths];
        }
        const paths =
          payload.type === "drop" ? payload.paths : pathsRef.current;
        if (paths.length === 0) {
          if (hoveringRef.current) callbacksRef.current.onHoverEnd?.();
          hoveringRef.current = false;
          setIsHovering(false);
          callbacksRef.current.onInternalDrag?.(
            inside
              ? {
                  type: payload.type === "drop" ? "drop" : "over",
                  point,
                }
              : { type: "leave" },
          );
          return;
        }
        const enteredTarget = inside && !hoveringRef.current;
        const leftTarget = !inside && hoveringRef.current;
        hoveringRef.current = inside;
        setIsHovering(inside);
        if (leftTarget) callbacksRef.current.onHoverEnd?.();

        if (payload.type === "enter") {
          if (inside) callbacksRef.current.onHoverPaths?.(pathsRef.current);
          return;
        }

        if (payload.type === "over" && enteredTarget) {
          callbacksRef.current.onHoverPaths?.(pathsRef.current);
          return;
        }

        if (payload.type === "drop") {
          hoveringRef.current = false;
          pathsRef.current = [];
          setIsHovering(false);
          callbacksRef.current.onHoverEnd?.();
          if (inside && payload.paths.length > 0) {
            callbacksRef.current.onDrop([...payload.paths], point);
          }
        }
      })
      .then((cleanup) => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      })
      .catch((error) => {
        console.error("[native-file-drop] failed to register listener", error);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [targetRef]);

  return { isHovering };
}

export function nativeDragPointToCssPoint(
  position: Pick<PhysicalPosition, "x" | "y">,
): NativeDropPoint {
  // Wry reports macOS drag locations in AppKit points, which already match CSS pixels.
  if (platform() === "windows") {
    return {
      x: position.x / window.devicePixelRatio,
      y: position.y / window.devicePixelRatio,
    };
  }
  return { x: position.x, y: position.y };
}

export function isPointInsideElement(
  element: HTMLElement | null,
  point: NativeDropPoint,
) {
  if (!element) return false;
  const rect = element.getBoundingClientRect();
  const inside =
    point.x >= rect.left &&
    point.x <= rect.right &&
    point.y >= rect.top &&
    point.y <= rect.bottom;
  if (!inside) return false;

  const hit = document.elementFromPoint?.(point.x, point.y);
  return !hit || element === hit || element.contains(hit);
}
