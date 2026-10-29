import {
  type RefObject,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";

export function useTranscriptScroll(
  containerRef: RefObject<HTMLDivElement | null>,
  contentRef: RefObject<HTMLDivElement | null>,
  active: boolean,
) {
  const [isAtTop, setIsAtTop] = useState(true);
  const [isAtBottom, setIsAtBottom] = useState(true);
  const [canScroll, setCanScroll] = useState(false);
  const followingRef = useRef(true);
  const frameRef = useRef<number | null>(null);

  useEffect(() => {
    const element = containerRef.current;
    const content = contentRef.current;
    if (!element || !content) return;

    let lastScrollTop = element.scrollTop;
    let lastTouchY: number | undefined;

    const updateEdges = () => {
      const distance =
        element.scrollHeight - element.scrollTop - element.clientHeight;
      const hasOverflow = element.scrollHeight - element.clientHeight > 1;
      setCanScroll(hasOverflow);
      setIsAtTop(!hasOverflow || element.scrollTop <= 1);
      setIsAtBottom(!hasOverflow || distance <= 1);
      return distance;
    };

    const schedule = () => {
      updateEdges();
      if (!active || !followingRef.current || frameRef.current !== null) return;
      frameRef.current = requestAnimationFrame(() => {
        frameRef.current = null;
        if (!followingRef.current) return;
        element.scrollTop = element.scrollHeight;
        lastScrollTop = element.scrollTop;
        updateEdges();
      });
    };

    const pauseFollowing = () => {
      if (element.scrollHeight > element.clientHeight) {
        followingRef.current = false;
      }
    };
    const handleWheel = (event: WheelEvent) => {
      if (event.deltaY < 0) pauseFollowing();
      else if (event.deltaY > 0 && !followingRef.current && updateEdges() <= 2)
        followingRef.current = true;
    };
    const handleTouchStart = (event: TouchEvent) => {
      lastTouchY = event.touches[0]?.clientY;
    };
    const handleTouchMove = (event: TouchEvent) => {
      const nextY = event.touches[0]?.clientY;
      if (
        lastTouchY !== undefined &&
        nextY !== undefined &&
        nextY > lastTouchY
      ) {
        pauseFollowing();
      }
      lastTouchY = nextY;
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        target.closest("input, textarea, [contenteditable='true']")
      )
        return;
      if (
        ["ArrowUp", "PageUp", "Home"].includes(event.key) ||
        (event.key === " " && event.shiftKey)
      ) {
        pauseFollowing();
      }
    };
    const handleScroll = () => {
      const distance = updateEdges();
      // Layout and scroll anchoring can move scrollTop upward without user input.
      if (element.scrollTop > lastScrollTop + 1 && distance <= 2) {
        followingRef.current = true;
      }
      lastScrollTop = element.scrollTop;
    };

    element.addEventListener("scroll", handleScroll, { passive: true });
    element.addEventListener("wheel", handleWheel, { passive: true });
    element.addEventListener("touchstart", handleTouchStart, { passive: true });
    element.addEventListener("touchmove", handleTouchMove, { passive: true });
    element.addEventListener("keydown", handleKeyDown);

    const observer = new ResizeObserver(schedule);
    observer.observe(element);
    observer.observe(content);
    schedule();

    return () => {
      element.removeEventListener("scroll", handleScroll);
      element.removeEventListener("wheel", handleWheel);
      element.removeEventListener("touchstart", handleTouchStart);
      element.removeEventListener("touchmove", handleTouchMove);
      element.removeEventListener("keydown", handleKeyDown);
      observer.disconnect();
      if (frameRef.current !== null) {
        cancelAnimationFrame(frameRef.current);
        frameRef.current = null;
      }
    };
  }, [containerRef, contentRef, active]);

  const scrollToBottom = useCallback(() => {
    followingRef.current = true;
    const element = containerRef.current;
    element?.scrollTo({ top: element.scrollHeight, behavior: "auto" });
  }, [containerRef]);

  const scrollToTop = useCallback(() => {
    followingRef.current = false;
    containerRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  }, [containerRef]);

  return { isAtTop, isAtBottom, canScroll, scrollToTop, scrollToBottom };
}

export function usePlaybackAutoScroll(
  containerRef: RefObject<HTMLElement | null>,
  currentMs: number,
  isPlaying: boolean,
) {
  const lastScrolledWordIdRef = useRef<string | null>(null);
  const userScrolledRef = useRef(false);
  const lastScrollTimeRef = useRef(0);

  const resetUserScroll = useCallback(() => {
    userScrolledRef.current = false;
  }, []);

  useEffect(() => {
    if (!isPlaying) {
      lastScrolledWordIdRef.current = null;
      userScrolledRef.current = false;
      return;
    }

    const element = containerRef.current;
    if (!element) {
      return;
    }

    const handleUserScroll = () => {
      const now = Date.now();
      if (now - lastScrollTimeRef.current > 100) {
        userScrolledRef.current = true;
      }
    };

    element.addEventListener("wheel", handleUserScroll);
    element.addEventListener("touchmove", handleUserScroll);

    return () => {
      element.removeEventListener("wheel", handleUserScroll);
      element.removeEventListener("touchmove", handleUserScroll);
    };
  }, [containerRef, isPlaying]);

  useEffect(() => {
    if (!isPlaying || userScrolledRef.current) {
      return;
    }

    const now = Date.now();
    if (now - lastScrollTimeRef.current < 200) {
      return;
    }

    const element = containerRef.current;
    if (!element) {
      return;
    }

    const currentLineEl = element.querySelector<HTMLElement>(
      "[data-line-current='true']",
    );

    if (!currentLineEl) {
      return;
    }

    const lineKey = currentLineEl.textContent?.slice(0, 50) ?? "";
    if (lineKey === lastScrolledWordIdRef.current) {
      return;
    }

    lastScrolledWordIdRef.current = lineKey;
    lastScrollTimeRef.current = now;

    currentLineEl.scrollIntoView({
      behavior: "smooth",
      block: "center",
    });
  }, [containerRef, currentMs, isPlaying]);

  return { resetUserScroll };
}
