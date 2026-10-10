import type { ThemePreference } from "./resolve";

const sans = '-apple-system, "SF Pro Text", system-ui, sans-serif';
const serif = '"Literata Variable", Georgia, serif';
const mono = '"IBM Plex Mono", ui-monospace, monospace';

export type ThemePalette = {
  paper: string;
  ink: string;
  muted: string;
  shell: string;
  sidebar: string;
  sidebarInk: string;
  sidebarMuted: string;
  sidebarHover: string;
  line: string;
  accent: string;
  accentInk: string;
  selection: string;
  selectionInk: string;
  wash: string;
  logo: string;
  logoInk: string;
};

export type DesignTheme = {
  id: string;
  name: string;
  description: string;
  typography: {
    heading: string;
    body: string;
    detail: string;
    wordmark: string;
  };
  geometry: {
    radius: string;
    logoRadius: string;
    titleSize: string;
    titleWeight: string;
    noteWidth: string;
    noteLeading: string;
    tabRadius: string;
  };
} & (
  | { appearance: "adaptive"; light: ThemePalette; dark: ThemePalette }
  | { appearance: "light" | "dark"; palette: ThemePalette }
);

const geometry: DesignTheme["geometry"] = {
  radius: "8px",
  logoRadius: "8px",
  titleSize: "34px",
  titleWeight: "550",
  noteWidth: "760px",
  noteLeading: "1.75",
  tabRadius: "6px",
};

// Themes only supply tokens. The shell, editor, controls, and logo share one implementation.
export const DESIGN_THEMES: readonly DesignTheme[] = [
  {
    id: "default",
    name: "Default",
    description: "Loofah, with light and dark appearances.",
    appearance: "adaptive",
    typography: { heading: sans, body: sans, detail: sans, wordmark: sans },
    geometry: { ...geometry },
    light: {
      paper: "#fdfdfb",
      ink: "#242423",
      muted: "#686863",
      shell: "#f1f1ec",
      sidebar: "#f0f0e9",
      sidebarInk: "#30302b",
      sidebarMuted: "#68685e",
      sidebarHover: "#e2e2d9",
      line: "#ddddd4",
      accent: "#88621e",
      accentInk: "#ffffff",
      selection: "#dfdfd3",
      selectionInk: "#343429",
      wash: "#eeeee6",
      logo: "#88621e",
      logoInk: "#fff8e7",
    },
    dark: {
      paper: "#242423",
      ink: "#ececea",
      muted: "#b0b0a7",
      shell: "#191918",
      sidebar: "#1c1c1a",
      sidebarInk: "#e6e6df",
      sidebarMuted: "#adada1",
      sidebarHover: "#2a2a25",
      line: "#3e3e38",
      accent: "#d5b471",
      accentInk: "#2f2513",
      selection: "#3d3d32",
      selectionInk: "#e9e9d9",
      wash: "#30302a",
      logo: "#d5b471",
      logoInk: "#2f2513",
    },
  },
  {
    id: "fieldnotes",
    name: "Fieldnotes",
    description: "Warm paper, literary type, terracotta ink.",
    appearance: "light",
    typography: { heading: serif, body: serif, detail: sans, wordmark: serif },
    geometry: {
      ...geometry,
      radius: "5px",
      logoRadius: "3px",
      titleSize: "36px",
      titleWeight: "450",
      noteWidth: "720px",
      noteLeading: "1.85",
      tabRadius: "3px",
    },
    palette: {
      paper: "#fcf9f2",
      ink: "#3f3429",
      muted: "#70614f",
      shell: "#f1ebdf",
      sidebar: "#eee6d8",
      sidebarInk: "#514334",
      sidebarMuted: "#74644f",
      sidebarHover: "#e5dccf",
      line: "#e0d5c4",
      accent: "#a84a28",
      accentInk: "#fff8f0",
      selection: "#ded2bd",
      selectionInk: "#493a28",
      wash: "#f2e5d5",
      logo: "#a84a28",
      logoInk: "#fff8f0",
    },
  },
  {
    id: "signal",
    name: "Signal",
    description: "Graphite, electric green, precise details.",
    appearance: "light",
    typography: { heading: sans, body: sans, detail: mono, wordmark: mono },
    geometry: {
      ...geometry,
      radius: "5px",
      logoRadius: "4px",
      titleSize: "33px",
      titleWeight: "650",
      noteWidth: "760px",
      tabRadius: "3px",
    },
    palette: {
      paper: "#ffffff",
      ink: "#272b28",
      muted: "#646e63",
      shell: "#f2f4ef",
      sidebar: "#f5f6f2",
      sidebarInk: "#2d332c",
      sidebarMuted: "#626d5e",
      sidebarHover: "#e8eee1",
      line: "#dce3d6",
      accent: "#334224",
      accentInk: "#d4f874",
      selection: "#e5eddb",
      selectionInk: "#2d3a23",
      wash: "#eff5e7",
      logo: "#2b3323",
      logoInk: "#c9f36a",
    },
  },
  {
    id: "forma",
    name: "Forma",
    description: "Confident cobalt, soft shapes, open space.",
    appearance: "light",
    typography: { heading: sans, body: sans, detail: sans, wordmark: sans },
    geometry: {
      ...geometry,
      radius: "14px",
      logoRadius: "50%",
      titleSize: "38px",
      titleWeight: "650",
      noteWidth: "800px",
      tabRadius: "12px",
    },
    palette: {
      paper: "#fffefa",
      ink: "#263250",
      muted: "#626b82",
      shell: "#f0f2fd",
      sidebar: "#3458db",
      sidebarInk: "#ffffff",
      sidebarMuted: "#e1e8ff",
      sidebarHover: "#2645bb",
      line: "#e0e5f2",
      accent: "#3458db",
      accentInk: "#ffffff",
      selection: "#fffefa",
      selectionInk: "#2c4bc4",
      wash: "#edf1ff",
      logo: "#ffffff",
      logoInk: "#3458db",
    },
  },
  {
    id: "margin",
    name: "Margin",
    description: "Lilac edges, a quiet canvas, room to think.",
    appearance: "light",
    typography: { heading: sans, body: sans, detail: sans, wordmark: sans },
    geometry: {
      ...geometry,
      radius: "10px",
      logoRadius: "50%",
      titleSize: "34px",
      titleWeight: "500",
      noteWidth: "820px",
      noteLeading: "1.85",
      tabRadius: "9px",
    },
    palette: {
      paper: "#fdfbff",
      ink: "#443c51",
      muted: "#6f5f7b",
      shell: "#f7f1fb",
      sidebar: "#eee5f5",
      sidebarInk: "#635071",
      sidebarMuted: "#725980",
      sidebarHover: "#e5d8ef",
      line: "#e6daee",
      accent: "#785694",
      accentInk: "#ffffff",
      selection: "#ded0ea",
      selectionInk: "#513466",
      wash: "#f1eaf7",
      logo: "#785694",
      logoInk: "#fdfbff",
    },
  },
  {
    id: "workshop",
    name: "Workshop",
    description: "Forest green, a working journal, ruled details.",
    appearance: "light",
    typography: { heading: serif, body: sans, detail: mono, wordmark: sans },
    geometry: {
      ...geometry,
      radius: "4px",
      logoRadius: "4px",
      titleSize: "34px",
      titleWeight: "500",
      noteWidth: "740px",
      tabRadius: "3px",
    },
    palette: {
      paper: "#fcfcf5",
      ink: "#354337",
      muted: "#58674f",
      shell: "#dfe7d8",
      sidebar: "#edf0e4",
      sidebarInk: "#40543e",
      sidebarMuted: "#5c6e50",
      sidebarHover: "#e1e7d6",
      line: "#d7e0cb",
      accent: "#38654d",
      accentInk: "#faffef",
      selection: "#38654d",
      selectionInk: "#faffef",
      wash: "#e6eedd",
      logo: "#38654d",
      logoInk: "#faffef",
    },
  },
  {
    id: "midnight",
    name: "Midnight",
    description: "Ink blue, cool silver, a focused night desk.",
    appearance: "dark",
    typography: { heading: sans, body: sans, detail: mono, wordmark: sans },
    geometry: {
      ...geometry,
      radius: "9px",
      logoRadius: "8px",
      titleSize: "35px",
      titleWeight: "550",
      noteWidth: "780px",
    },
    palette: {
      paper: "#161e2e",
      ink: "#e4ebf7",
      muted: "#a1afc7",
      shell: "#101827",
      sidebar: "#111b2c",
      sidebarInk: "#dbe6fa",
      sidebarMuted: "#9fb2d3",
      sidebarHover: "#1c2b43",
      line: "#2b3a53",
      accent: "#9cbfff",
      accentInk: "#152e56",
      selection: "#2b4266",
      selectionInk: "#e3eeff",
      wash: "#223149",
      logo: "#24395c",
      logoInk: "#bed6ff",
    },
  },
  {
    id: "ember",
    name: "Ember",
    description: "Warm charcoal, copper light, understated type.",
    appearance: "dark",
    typography: { heading: serif, body: sans, detail: sans, wordmark: serif },
    geometry: {
      ...geometry,
      radius: "7px",
      logoRadius: "6px",
      titleSize: "36px",
      titleWeight: "450",
      noteWidth: "740px",
    },
    palette: {
      paper: "#25201e",
      ink: "#f1e5dc",
      muted: "#bca69a",
      shell: "#1b1715",
      sidebar: "#201a17",
      sidebarInk: "#efddd0",
      sidebarMuted: "#c3aa98",
      sidebarHover: "#32251e",
      line: "#48372d",
      accent: "#efaa78",
      accentInk: "#3a2111",
      selection: "#59402e",
      selectionInk: "#ffebda",
      wash: "#382a22",
      logo: "#4e3221",
      logoInk: "#ffc68d",
    },
  },
];

export const DEFAULT_DESIGN_THEME = DESIGN_THEMES[0]!;

export function getDesignTheme(id: unknown): DesignTheme {
  return DESIGN_THEMES.find((theme) => theme.id === id) ?? DEFAULT_DESIGN_THEME;
}

export function hexToHsl(hex: string): string {
  const [r, g, b] = [1, 3, 5].map(
    (offset) => parseInt(hex.slice(offset, offset + 2), 16) / 255,
  ) as [number, number, number];
  const max = Math.max(r, g, b),
    min = Math.min(r, g, b);
  const lightness = (max + min) / 2,
    delta = max - min;
  const saturation =
    delta === 0 ? 0 : delta / (1 - Math.abs(2 * lightness - 1));
  const hue =
    delta === 0
      ? 0
      : max === r
        ? ((g - b) / delta + (g < b ? 6 : 0)) * 60
        : max === g
          ? ((b - r) / delta + 2) * 60
          : ((r - g) / delta + 4) * 60;
  return `${hue.toFixed(2)} ${(saturation * 100).toFixed(2)}% ${(lightness * 100).toFixed(2)}%`;
}

export function designThemeTokens(
  theme: DesignTheme,
  isDark: boolean,
): Record<string, string> {
  const p =
    theme.appearance === "adaptive"
      ? isDark
        ? theme.dark
        : theme.light
      : theme.palette;
  isDark =
    theme.appearance === "adaptive" ? isDark : theme.appearance === "dark";
  const hsl = hexToHsl;
  return {
    "--background": hsl(p.shell),
    "--foreground": hsl(p.ink),
    "--card": hsl(p.paper),
    "--card-foreground": hsl(p.ink),
    "--popover": hsl(p.paper),
    "--popover-foreground": hsl(p.ink),
    "--primary": hsl(p.accent),
    "--primary-foreground": hsl(p.accentInk),
    "--secondary": hsl(p.wash),
    "--secondary-foreground": hsl(p.ink),
    "--muted": hsl(p.wash),
    "--muted-foreground": hsl(p.muted),
    "--accent": hsl(p.wash),
    "--accent-foreground": hsl(p.ink),
    "--brand": hsl(p.accent),
    "--brand-foreground": hsl(p.accentInk),
    "--border": hsl(p.line),
    "--input": hsl(p.muted),
    "--ring": hsl(p.accent),
    "--sidebar-background": hsl(p.sidebar),
    "--sidebar-foreground": hsl(p.sidebarInk),
    "--sidebar-primary": hsl(p.accent),
    "--sidebar-primary-foreground": hsl(p.accentInk),
    "--sidebar-muted": hsl(p.sidebarMuted),
    "--sidebar-hover": hsl(p.sidebarHover),
    "--sidebar-accent": hsl(p.selection),
    "--sidebar-accent-foreground": hsl(p.selectionInk),
    "--sidebar-border": hsl(p.line),
    "--sidebar-ring": hsl(p.sidebarInk),
    "--logo-background": p.logo,
    "--logo-foreground": p.logoInk,
    "--app-floating-chrome": hsl(p.shell),
    "--app-floating-panel": hsl(p.paper),
    "--app-floating-border": hsl(p.line),
    "--recording": isDark ? "4 80% 72%" : "4 64% 46%",
    "--recording-foreground": isDark ? "4 50% 12%" : "0 0% 100%",
    "--destructive": isDark ? "4 80% 72%" : "4 64% 46%",
    "--destructive-foreground": isDark ? "4 50% 12%" : "0 0% 100%",
    "--theme-heading-font": theme.typography.heading,
    "--theme-body-font": theme.typography.body,
    "--theme-detail-font": theme.typography.detail,
    "--theme-wordmark-font": theme.typography.wordmark,
    "--radius": theme.geometry.radius,
    "--theme-logo-radius": theme.geometry.logoRadius,
    "--theme-title-size": theme.geometry.titleSize,
    "--theme-title-weight": theme.geometry.titleWeight,
    "--theme-note-width": theme.geometry.noteWidth,
    "--theme-note-leading": theme.geometry.noteLeading,
    "--theme-tab-radius": theme.geometry.tabRadius,
    "--selection-overlay": `${p.accent}33`,
  };
}

export function resolveThemeAppearance(
  theme: DesignTheme,
  preference: ThemePreference,
): ThemePreference {
  return theme.appearance === "adaptive" ? preference : theme.appearance;
}
