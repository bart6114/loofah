import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DESIGN_THEMES } from "@hypr/design-theme/catalog";

import { snapshotOptions, type Snapshot } from "./api";
import {
  applyAppearance,
  defaultAppearance,
  normalizeAppearance,
  restoreAppearance,
  useAppearance,
} from "./appearance";
import { AppearanceSettings } from "./AppearanceSettings";
import { groupNotes } from "./NoteLibrary";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
let change: (() => void) | undefined;
let prefersDark = false;
const snapshot: Snapshot = {
  appearance: defaultAppearance,
  vault: { local_path: "/test", icloud_path: null },
  sessions: [],
  jobs: [],
  recording: {
    session_id: null,
    elapsed_seconds: 0,
    interrupted: false,
    error: null,
  },
  model: {
    ready: false,
    downloading: false,
    phase: "idle",
    downloaded_bytes: 0,
    total_bytes: 0,
    model_id: "parakeet-v3",
    revision: "",
  },
  summary: { available: false, reason: null },
  sync: {
    connected: false,
    state: "idle",
    pending: 0,
    error: null,
    conflicts: [],
  },
  settings: {
    transcription_model: "parakeet-v3",
    summary_provider: "none",
    summary_model: "",
    summary_base_url: "",
    providers: {},
    summary_language: "en",
    has_api_key: false,
    ai_sharing_allowed: false,
  },
};
beforeEach(() => {
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    clear: () => values.clear(),
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  });
  localStorage.clear();
  prefersDark = false;
  change = undefined;
  vi.stubGlobal("matchMedia", () => ({
    get matches() {
      return prefersDark;
    },
    addEventListener: (_: string, fn: () => void) => {
      change = fn;
    },
    removeEventListener: vi.fn(),
  }));
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  applyAppearance(defaultAppearance);
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  client.setQueryData(snapshotOptions.queryKey, snapshot);
  function Harness() {
    const { data } = useQuery({ ...snapshotOptions, enabled: false });
    useAppearance(data?.appearance);
    return <AppearanceSettings snapshot={data!} />;
  }
  render(
    <QueryClientProvider client={client}>
      <Harness />
    </QueryClientProvider>,
  );
  return client;
}
describe("iPhone appearance", () => {
  it("falls back safely for obsolete or missing vault preferences", () => {
    expect(
      normalizeAppearance({ theme: "obsolete", design_theme: "removed" }),
    ).toEqual(defaultAppearance);
    expect(
      normalizeAppearance({ theme: "dark", design_theme: "removed" }),
    ).toEqual({ theme: "dark", design_theme: "default" });
    expect(normalizeAppearance()).toEqual(defaultAppearance);
  });
  it("restores persisted theme before rendering and recovers from damaged cache", () => {
    applyAppearance({ theme: "dark", design_theme: "fieldnotes" });
    document.documentElement.removeAttribute("style");
    restoreAppearance();
    expect(document.documentElement.dataset.designTheme).toBe("fieldnotes");
    expect(document.documentElement.style.colorScheme).toBe("light");
    localStorage.setItem("loofah-mobile-appearance", "broken");
    restoreAppearance();
    expect(document.documentElement.dataset.designTheme).toBe("default");
  });
  it("follows system changes while fixed themes keep their chosen appearance", () => {
    mount();
    prefersDark = true;
    change?.();
    expect(document.documentElement.style.colorScheme).toBe("dark");
    applyAppearance({ theme: "system", design_theme: "fieldnotes" });
    change?.();
    expect(document.documentElement.style.colorScheme).toBe("light");
    applyAppearance({ theme: "system", design_theme: "default" });
    expect(document.documentElement.style.colorScheme).toBe("dark");
  });
  it("saves the shared theme and restores the selected card on reopening", async () => {
    const client = mount();
    for (const theme of DESIGN_THEMES)
      expect(screen.getByRole("button", { name: theme.name })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Midnight" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_save_appearance", {
        theme: "system",
        designTheme: "midnight",
      }),
    );
    await waitFor(() =>
      expect(
        screen
          .getByRole("button", { name: "Midnight" })
          .getAttribute("aria-pressed"),
      ).toBe("true"),
    );
    expect(document.documentElement.style.colorScheme).toBe("dark");
    expect(
      client.getQueryData<Snapshot>(snapshotOptions.queryKey)?.appearance
        .design_theme,
    ).toBe("midnight");
    expect(invoke).toHaveBeenCalledWith("mobile_apply_appearance", {
      appearance: "dark",
    });
  });
  it("rolls back the preview and native appearance when saving fails", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "mobile_save_appearance") throw new Error("Disk is full");
    });
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Ember" }));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "Couldn’t save your appearance. Disk is full",
    );
    expect(document.documentElement.dataset.designTheme).toBe("default");
    expect(
      screen
        .getByRole("button", { name: "Default" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
    expect(invoke).toHaveBeenLastCalledWith("mobile_apply_appearance", {
      appearance: "system",
    });
  });
  it("persists an explicit dark preference independently of theme selection", async () => {
    mount();
    fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_save_appearance", {
        theme: "dark",
        designTheme: "default",
      }),
    );
    expect(document.documentElement.style.colorScheme).toBe("dark");
  });
});
it("groups notes in local calendar days across month boundaries", () => {
  const notes = [
    {
      id: "older",
      title: "Older",
      created_at: "2026-09-20T12:00:00",
      has_transcript_words: false,
    },
    {
      id: "yesterday",
      title: "Yesterday",
      created_at: "2026-10-31T23:59:00",
      has_transcript_words: false,
    },
    {
      id: "today",
      title: "Today",
      created_at: "2026-11-01T00:01:00",
      has_transcript_words: false,
    },
  ];
  const groups = groupNotes(notes, new Date("2026-11-01T12:00:00"));
  expect(groups[0]).toEqual(["Today", [notes[2]]]);
  expect(groups[1]).toEqual(["Yesterday", [notes[1]]]);
  expect(notes[0].id).toBe("older");
});
