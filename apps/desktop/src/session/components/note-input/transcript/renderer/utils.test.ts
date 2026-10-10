import chroma from "chroma-js";
import { describe, expect, it } from "vitest";

import {
  getActiveLineIndex,
  getSegmentColor,
  getSegmentColorVars,
} from "./utils";

import { DESIGN_THEMES } from "~/shared/theme/catalog";
import type { SegmentKey, SegmentWord } from "~/stt/live-segment";

describe("transcript renderer utils", () => {
  for (const theme of DESIGN_THEMES) {
    it(`keeps every speaker label readable in ${theme.name}`, () => {
      const palettes =
        theme.appearance === "adaptive"
          ? ([
              ["light", theme.light],
              ["dark", theme.dark],
            ] as const)
          : ([[theme.appearance, theme.palette]] as const);
      for (const [mode, palette] of palettes) {
        for (const channel of ["DirectMic", "RemoteParty"] as const) {
          for (let speaker_index = 0; speaker_index < 6; speaker_index++) {
            const color = getSegmentColor(
              { channel, speaker_index, speaker_human_id: null },
              mode,
            );
            expect(
              chroma.contrast(color, palette.paper),
              `${mode} ${channel} ${speaker_index}`,
            ).toBeGreaterThanOrEqual(4.5);
          }
        }
      }
    });
  }
  it("uses a brighter speaker color for dark mode", () => {
    const key: SegmentKey = {
      channel: "RemoteParty",
      speaker_index: 1,
      speaker_human_id: null,
    };

    expect(chroma(getSegmentColor(key, "dark")).luminance()).toBeGreaterThan(
      chroma(getSegmentColor(key)).luminance(),
    );
  });

  it("exposes light and dark speaker color variables", () => {
    const key: SegmentKey = {
      channel: "DirectMic",
      speaker_index: 0,
      speaker_human_id: null,
    };

    expect(getSegmentColorVars(key)).toEqual({
      "--segment-color-light": getSegmentColor(key),
      "--segment-color-dark": getSegmentColor(key, "dark"),
    });
  });

  it("finds the active transcript line without building line groups", () => {
    const words: SegmentWord[] = [
      createWord("word-1", "Hello", 100, 400),
      createWord("word-2", "world.", 400, 900),
      createWord("word-3", "Next", 1400, 1600),
      createWord("word-4", "line!", 1600, 2100),
    ];

    expect(getActiveLineIndex(words, 50, 0)).toBeNull();
    expect(getActiveLineIndex(words, 50, 149)).toBeNull();
    expect(getActiveLineIndex(words, 50, 150)).toBe(0);
    expect(getActiveLineIndex(words, 50, 950)).toBe(0);
    expect(getActiveLineIndex(words, 50, 1200)).toBeNull();
    expect(getActiveLineIndex(words, 50, 1450)).toBe(1);
    expect(getActiveLineIndex(words, 50, 2150)).toBe(1);
    expect(getActiveLineIndex(words, 50, 2200)).toBeNull();
  });
});

function createWord(
  id: string,
  text: string,
  startMs: number,
  endMs: number,
): SegmentWord {
  return {
    id,
    text,
    start_ms: startMs,
    end_ms: endMs,
    channel: "MixedCapture",
    is_final: true,
  };
}
