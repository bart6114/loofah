import { describe, expect, it, vi } from "vitest";

import { createPlaybackHighlighter } from "./playback-highlight";

describe("playback highlighting", () => {
  it("updates overlapping lines, supports seeking, and reconciles changed content", async () => {
    const container = document.createElement("div");
    container.innerHTML =
      '<span data-line-start="100" data-line-end="300">one</span><span data-line-start="200" data-line-end="400">two</span><span data-line-start="500" data-line-end="600">three</span>';
    const highlighter = createPlaybackHighlighter(container);
    const current = () =>
      [...container.querySelectorAll('[data-line-current="true"]')].map(
        (line) => line.textContent,
      );
    highlighter.update(250);
    expect(current()).toEqual(["one", "two"]);
    highlighter.update(550);
    expect(current()).toEqual(["three"]);
    highlighter.update(150);
    expect(current()).toEqual(["one"]);
    container.firstElementChild!.setAttribute("data-line-start", "160");
    await Promise.resolve();
    expect(current()).toEqual([]);
    highlighter.update(0);
    expect(current()).toEqual([]);
    highlighter.destroy();
  });

  it("leaves all text selectable and does not mutate unchanged highlights", () => {
    const container = document.createElement("div");
    container.innerHTML = Array.from(
      { length: 10000 },
      (_, i) =>
        `<span data-line-start="${i * 100}" data-line-end="${i * 100 + 90}">${i} </span>`,
    ).join("");
    const highlighter = createPlaybackHighlighter(container);
    highlighter.update(500010);
    const line = container.querySelector('[data-line-current="true"]')!;
    const set = vi.spyOn(line, "setAttribute");
    highlighter.update(500020);
    expect(set).not.toHaveBeenCalled();
    expect(container.children).toHaveLength(10000);
    expect(container.textContent).toContain("9999");
    highlighter.destroy();
    expect(container.querySelector('[data-line-current="true"]')).toBeNull();
  });
});
