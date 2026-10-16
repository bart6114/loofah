import { beforeEach, describe, expect, it } from "vitest";

import {
  DESIGN_THEMES,
  designThemeTokens,
  getDesignTheme,
  hexToHsl,
} from "./catalog";
import { applyDocumentDesignTheme } from "./design";

function contrast(a: string, b: string) {
  const luminance = (hex: string) => {
    const channels = [1, 3, 5].map((offset) => {
      const channel = parseInt(hex.slice(offset, offset + 2), 16) / 255;
      return channel <= 0.04045
        ? channel / 12.92
        : ((channel + 0.055) / 1.055) ** 2.4;
    });
    return (
      channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722
    );
  };
  const values = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (values[0]! + 0.05) / (values[1]! + 0.05);
}

describe("design themes", () => {
  beforeEach(() => {
    document.documentElement.removeAttribute("style");
    document.documentElement.removeAttribute("data-design-theme");
    document.documentElement.className = "";
  });

  it("falls back for a removed or unknown theme without losing appearance", () => {
    document.documentElement.classList.add("dark");
    applyDocumentDesignTheme("removed-theme");
    expect(document.documentElement.dataset.designTheme).toBe("default");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(document.documentElement.style.colorScheme).toBe("dark");
    expect(getDesignTheme(null).id).toBe("default");
  });

  it("has one adaptive default, five light themes, and two dark themes", () => {
    expect(DESIGN_THEMES.map(({ id }) => id)).toEqual([
      "default",
      "fieldnotes",
      "signal",
      "forma",
      "margin",
      "workshop",
      "midnight",
      "ember",
    ]);
    expect(
      DESIGN_THEMES.filter(({ appearance }) => appearance === "dark").map(
        ({ id }) => id,
      ),
    ).toEqual(["midnight", "ember"]);
  });

  it("replaces the whole palette and typography when switching themes", () => {
    applyDocumentDesignTheme("fieldnotes", false);
    applyDocumentDesignTheme("midnight", true);
    const root = document.documentElement;
    for (const [key, value] of Object.entries(
      designThemeTokens(getDesignTheme("midnight"), true),
    )) {
      expect(root.style.getPropertyValue(key), key).toBe(value);
    }
    expect(root.dataset.designTheme).toBe("midnight");
  });

  for (const theme of DESIGN_THEMES) {
    for (const appearance of theme.appearance === "adaptive"
      ? ["light", "dark"]
      : [theme.appearance]) {
      it(`${theme.name} ${appearance} preserves readable text, controls, and navigation`, () => {
        const p =
          theme.appearance === "adaptive"
            ? appearance === "dark"
              ? theme.dark
              : theme.light
            : theme.palette;
        for (const [label, text, background] of [
          ["note text", p.ink, p.paper],
          ["muted text", p.muted, p.paper],
          ["muted text on dialogs", p.muted, p.shell],
          ["muted text on inset controls", p.muted, p.wash],
          ["control text", p.ink, p.wash],
          ["sidebar", p.sidebarInk, p.sidebar],
          ["sidebar details", p.sidebarMuted, p.sidebar],
          ["sidebar hover", p.sidebarInk, p.sidebarHover],
          ["selection", p.selectionInk, p.selection],
          ["primary control", p.accentInk, p.accent],
        ]) {
          expect(contrast(text!, background!), label).toBeGreaterThanOrEqual(
            4.5,
          );
        }
        expect(contrast(p.logoInk, p.logo), "logo").toBeGreaterThanOrEqual(3);
        expect(
          contrast(p.accent, p.paper),
          "links and active tabs",
        ).toBeGreaterThanOrEqual(4.5);
        expect(
          Object.keys(designThemeTokens(theme, appearance === "dark")),
        ).toEqual(Object.keys(designThemeTokens(DESIGN_THEMES[0]!, false)));
      });
    }
  }

  it("converts achromatic and saturated colors to the shared HSL contract", () => {
    expect(hexToHsl("#ffffff")).toBe("0.00 0.00% 100.00%");
    expect(hexToHsl("#ff0000")).toBe("0.00 100.00% 50.00%");
  });
});
