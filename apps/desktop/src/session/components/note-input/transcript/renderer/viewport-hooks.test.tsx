import { act, cleanup, renderHook } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useTranscriptScroll } from "./viewport-hooks";

let frames: Map<number, FrameRequestCallback>;
let observers: Set<{ targets: Set<Element>; callback: () => void }>;

beforeEach(() => {
  frames = new Map();
  observers = new Set();
  let frameId = 0;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.set(++frameId, callback);
    return frameId;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
  vi.stubGlobal(
    "ResizeObserver",
    class {
      targets = new Set<Element>();
      constructor(public callback: () => void) {
        observers.add(this);
      }
      observe(target: Element) {
        this.targets.add(target);
      }
      disconnect() {
        observers.delete(this);
      }
    },
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function flushFrames() {
  act(() => {
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach((callback) => callback(0));
  });
}

function viewport({ height = 1000, top = 900, active = true } = {}) {
  const element = document.createElement("div");
  const content = document.createElement("div");
  element.append(content);
  let scrollTop = top;
  let scrollHeight = height;
  Object.defineProperties(element, {
    clientHeight: { value: 100 },
    scrollHeight: { get: () => scrollHeight },
    scrollTop: {
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = Math.max(0, Math.min(value, scrollHeight - 100));
      },
    },
  });
  element.scrollTo = vi.fn(({ top }: ScrollToOptions) => {
    element.scrollTop = top ?? 0;
    element.dispatchEvent(new Event("scroll"));
  }) as typeof element.scrollTo;
  const containerRef = createRef<HTMLDivElement>();
  const contentRef = createRef<HTMLDivElement>();
  containerRef.current = element;
  contentRef.current = content;
  const hook = renderHook(
    ({ active }) => useTranscriptScroll(containerRef, contentRef, active),
    {
      initialProps: { active },
    },
  );
  return {
    ...hook,
    element,
    content,
    resize(height: number) {
      act(() => {
        scrollHeight = height;
        element.scrollTop = scrollTop;
        element.dispatchEvent(new Event("scroll"));
        for (const observer of observers) {
          if (observer.targets.has(content)) observer.callback();
        }
      });
    },
    wheel(deltaY: number) {
      act(() => element.dispatchEvent(new WheelEvent("wheel", { deltaY })));
    },
    scroll(top: number) {
      act(() => {
        element.scrollTop = top;
        element.dispatchEvent(new Event("scroll"));
      });
    },
  };
}

describe("useTranscriptScroll", () => {
  it("keeps both edge controls meaningful for a short overflow", () => {
    const view = viewport({ height: 150, top: 0, active: false });
    expect(view.result.current).toMatchObject({
      canScroll: true,
      isAtTop: true,
      isAtBottom: false,
    });
    view.scroll(50);
    expect(view.result.current).toMatchObject({
      isAtTop: false,
      isAtBottom: true,
    });
  });

  it("does not scroll an inactive transcript on open or content growth", () => {
    const view = viewport({ top: 300, active: false });
    flushFrames();
    view.resize(1300);
    flushFrames();
    expect(view.element.scrollTop).toBe(300);
  });

  it("follows growing partial text without a React update or new segment", () => {
    const view = viewport();
    flushFrames();
    view.resize(1400);
    view.resize(1500);
    expect(frames.size).toBe(1);
    flushFrames();
    expect(view.element.scrollTop).toBe(1400);
    expect(view.result.current.isAtBottom).toBe(true);
  });

  it("keeps following after a partial contracts and clamps the scroll position", () => {
    const view = viewport();
    flushFrames();
    view.resize(975);
    expect(view.element.scrollTop).toBe(875);
    flushFrames();
    view.resize(1200);
    flushFrames();
    expect(view.element.scrollTop).toBe(1100);
  });

  it("does not mistake an anchoring adjustment for deliberate scrolling", () => {
    const view = viewport();
    flushFrames();
    view.scroll(850);
    view.resize(1300);
    flushFrames();
    expect(view.element.scrollTop).toBe(1200);
  });

  it("stops following immediately on upward wheel input, including a queued frame", () => {
    const view = viewport();
    flushFrames();
    view.resize(1200);
    view.wheel(-10);
    view.scroll(850);
    flushFrames();
    view.resize(1300);
    flushFrames();
    expect(view.element.scrollTop).toBe(850);
  });

  it("preserves deliberate scroll-away when recording becomes active again", () => {
    const view = viewport();
    flushFrames();
    view.wheel(-100);
    view.scroll(500);
    view.rerender({ active: false });
    view.rerender({ active: true });
    view.resize(1400);
    flushFrames();
    expect(view.element.scrollTop).toBe(500);
  });

  it("resumes following after manually scrolling back to the bottom", () => {
    const view = viewport();
    flushFrames();
    view.wheel(-100);
    view.scroll(500);
    view.wheel(100);
    view.scroll(900);
    view.resize(1400);
    flushFrames();
    expect(view.element.scrollTop).toBe(1300);
  });

  it("pauses for upward keyboard navigation", () => {
    const view = viewport();
    flushFrames();
    act(() =>
      view.element.dispatchEvent(
        new KeyboardEvent("keydown", { key: "PageUp" }),
      ),
    );
    view.scroll(500);
    view.resize(1400);
    flushFrames();
    expect(view.element.scrollTop).toBe(500);
  });

  it("pauses immediately when a touch gesture scrolls toward earlier text", () => {
    const view = viewport();
    flushFrames();
    view.resize(1200);
    const touch = (type: string, clientY: number) => {
      const event = new Event(type);
      Object.defineProperty(event, "touches", { value: [{ clientY }] });
      act(() => view.element.dispatchEvent(event));
    };
    touch("touchstart", 100);
    touch("touchmove", 200);
    view.scroll(500);
    flushFrames();
    view.resize(1400);
    flushFrames();
    expect(view.element.scrollTop).toBe(500);
    touch("touchmove", 50);
    view.scroll(1300);
    view.resize(1500);
    flushFrames();
    expect(view.element.scrollTop).toBe(1400);
  });

  it("ignores navigation keys inside an editable field", () => {
    const view = viewport();
    flushFrames();
    const input = document.createElement("textarea");
    view.content.append(input);
    act(() =>
      input.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Home", bubbles: true }),
      ),
    );
    view.resize(1400);
    flushFrames();
    expect(view.element.scrollTop).toBe(1300);
  });

  it("cancels queued following when recording stops", () => {
    const view = viewport();
    flushFrames();
    view.resize(1400);
    view.rerender({ active: false });
    expect(frames.size).toBe(0);
    flushFrames();
    expect(view.element.scrollTop).toBe(900);
  });

  it("lets the bottom button resume following and the top button pause it", () => {
    const view = viewport();
    flushFrames();
    act(() => view.result.current.scrollToTop());
    view.resize(1400);
    flushFrames();
    expect(view.element.scrollTop).toBe(0);
    act(() => view.result.current.scrollToBottom());
    expect(view.element.scrollTop).toBe(1300);
    view.resize(1600);
    flushFrames();
    expect(view.element.scrollTop).toBe(1500);
  });

  it("disconnects observers and cancels scheduled following on unmount", () => {
    const view = viewport();
    expect(frames.size).toBe(1);
    view.unmount();
    expect(frames.size).toBe(0);
    expect(observers.size).toBe(0);
  });
});
