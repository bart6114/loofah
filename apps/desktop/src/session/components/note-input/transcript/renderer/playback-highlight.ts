import { useLayoutEffect, useRef, type RefObject } from "react";

export function createPlaybackHighlighter(container: HTMLElement) {
  let lines: {
    element: HTMLElement;
    start: number;
    end: number;
    maxEnd: number;
  }[] = [];
  let active = new Set<HTMLElement>();
  let currentMs = 0;
  const update = (time: number) => {
    currentMs = time;
    const next = new Set<HTMLElement>();
    if (time > 0) {
      let low = 0;
      let high = lines.length;
      while (low < high) {
        const mid = (low + high) >>> 1;
        if (lines[mid]!.start <= time) low = mid + 1;
        else high = mid;
      }
      for (
        let index = low - 1;
        index >= 0 && lines[index]!.maxEnd >= time;
        index--
      ) {
        const line = lines[index]!;
        if (line.end >= time) next.add(line.element);
      }
    }
    for (const element of active)
      if (!next.has(element)) element.removeAttribute("data-line-current");
    for (const element of next)
      if (!active.has(element))
        element.setAttribute("data-line-current", "true");
    active = next;
  };
  const rebuild = () => {
    lines = Array.from(
      container.querySelectorAll<HTMLElement>(
        "[data-line-start][data-line-end]",
      ),
      (element) => ({
        element,
        start: Number(element.dataset.lineStart),
        end: Number(element.dataset.lineEnd),
        maxEnd: 0,
      }),
    ).sort((a, b) => a.start - b.start);
    let maxEnd = -Infinity;
    for (const line of lines) line.maxEnd = maxEnd = Math.max(maxEnd, line.end);
    update(currentMs);
  };
  const observer = new MutationObserver(rebuild);
  observer.observe(container, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["data-line-start", "data-line-end"],
  });
  rebuild();
  return {
    update,
    destroy: () => {
      observer.disconnect();
      update(0);
      lines = [];
    },
  };
}

export function usePlaybackHighlight(
  containerRef: RefObject<HTMLDivElement | null>,
  currentMs: number,
) {
  const highlighter = useRef<ReturnType<
    typeof createPlaybackHighlighter
  > | null>(null);
  useLayoutEffect(() => {
    if (!containerRef.current) return;
    const value = createPlaybackHighlighter(containerRef.current);
    highlighter.current = value;
    return () => {
      value.destroy();
      highlighter.current = null;
    };
  }, [containerRef]);
  useLayoutEffect(() => {
    highlighter.current?.update(currentMs);
  }, [currentMs]);
}
