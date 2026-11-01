import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { groupTranscript, Transcript } from "./Transcript";

afterEach(cleanup);

describe("readable transcript", () => {
  it("joins words and punctuation, retaining the first timestamp after pauses", () => {
    expect(
      groupTranscript([
        { text: " Hello", start: 1, end: 1.2 },
        { text: ",", start: 1.2, end: 1.2 },
        { text: " world! ", start: 1.3, end: 1.8 },
        { text: "  ", start: 2, end: 2 },
        { text: "Next thought.", start: 4, end: 5 },
      ]),
    ).toEqual([
      { start: 1, text: "Hello, world!" },
      { start: 4, text: "Next thought." },
    ]);
  });

  it("bounds continuous speech and uses sentence endings for readable breaks", () => {
    const words = Array.from({ length: 80 }, (_, index) => ({
      text: index === 39 ? "word." : "word",
      start: index * 0.2,
      end: (index + 1) * 0.2,
    }));
    words[0].text = "A".repeat(120);
    const paragraphs = groupTranscript(words);
    expect(paragraphs).toHaveLength(2);
    expect(paragraphs[0].text.endsWith("word.")).toBe(true);
    expect(paragraphs[1].start).toBe(8);
    const continuous = groupTranscript(
      Array.from({ length: 2000 }, (_, index) => ({
        text: "continuous",
        start: index * 0.2,
        end: (index + 1) * 0.2,
      })),
    );
    expect(continuous.every((paragraph) => paragraph.text.length <= 700)).toBe(
      true,
    );
    expect(
      continuous.flatMap((paragraph) => paragraph.text.split(" ")),
    ).toHaveLength(2000);
  });

  it("offers accessible timestamp seeking only when audio is available", () => {
    const words = [{ text: "A thought.", start: 65, end: 66 }];
    const view = render(<Transcript words={words} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("1:05")).toBeTruthy();
    const onSeek = vi.fn();
    view.rerender(<Transcript words={words} onSeek={onSeek} />);
    fireEvent.click(screen.getByRole("button", { name: "Play from 1:05" }));
    expect(onSeek).toHaveBeenCalledWith(65);
  });

  it("renders long recordings progressively without dropping text", () => {
    const words = Array.from({ length: 85 }, (_, index) => ({
      text: `Thought ${index}`,
      start: index * 4,
      end: index * 4 + 1,
    }));
    const view = render(<Transcript words={words} />);
    expect(view.container.querySelectorAll("p")).toHaveLength(40);
    expect(screen.queryByText("Thought 40")).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Show more transcript" }),
    );
    expect(view.container.querySelectorAll("p")).toHaveLength(80);
    fireEvent.click(
      screen.getByRole("button", { name: "Show more transcript" }),
    );
    expect(view.container.querySelectorAll("p")).toHaveLength(85);
    expect(screen.getByText("Thought 84")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
