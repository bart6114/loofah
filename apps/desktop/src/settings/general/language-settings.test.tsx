import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  config: {
    ai_language: "en",
    spoken_languages: ["nl"],
    meeting_languages: undefined as string[] | undefined,
  },
  save: vi.fn(),
}));
vi.mock("~/shared/config", () => ({ useConfigValues: () => mocks.config }));
vi.mock("~/settings/queries", () => ({ setSettingValues: mocks.save }));
vi.mock("./main-language", () => ({
  MainLanguageView: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (v: string) => void;
  }) => (
    <select
      aria-label="Output language"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      <option value="en">English</option>
      <option value="fr">French</option>
    </select>
  ),
}));
vi.mock("./spoken-languages", () => ({
  SpokenLanguagesView: ({
    value,
    onChange,
  }: {
    value: string[];
    onChange: (v: string[]) => void;
  }) => (
    <>
      <span>{value.join(",")}</span>
      <button onClick={() => onChange(["nl"])}>Dutch only</button>
    </>
  ),
}));

import {
  MeetingLanguageSettings,
  SummaryLanguageSettings,
} from "./language-settings";

function renderSettings(view: React.ReactNode) {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { mutations: { retry: false } } })
      }
    >
      {view}
    </QueryClientProvider>,
  );
}
afterEach(cleanup);
beforeEach(() => {
  mocks.save.mockReset().mockResolvedValue(undefined);
  mocks.config.meeting_languages = undefined;
});
describe("independent language settings", () => {
  it("preserves legacy meeting languages when changing output language", async () => {
    renderSettings(<SummaryLanguageSettings />);
    fireEvent.change(screen.getByLabelText("Output language"), {
      target: { value: "fr" },
    });
    await waitFor(() =>
      expect(mocks.save).toHaveBeenCalledWith(
        { ai_language: "fr", meeting_languages: '["en","nl"]' },
        expect.anything(),
      ),
    );
  });
  it("lets Dutch-only meetings keep English summaries", async () => {
    renderSettings(<MeetingLanguageSettings />);
    expect(screen.getByText("en,nl")).toBeTruthy();
    fireEvent.click(screen.getByText("Dutch only"));
    await waitFor(() =>
      expect(mocks.save).toHaveBeenCalledWith(
        { meeting_languages: '["nl"]' },
        expect.anything(),
      ),
    );
  });
  it("preserves an independent selection when output language changes", async () => {
    mocks.config.meeting_languages = ["nl"];
    renderSettings(<SummaryLanguageSettings />);
    fireEvent.change(screen.getByLabelText("Output language"), {
      target: { value: "fr" },
    });
    await waitFor(() =>
      expect(mocks.save).toHaveBeenCalledWith(
        { ai_language: "fr", meeting_languages: '["nl"]' },
        expect.anything(),
      ),
    );
  });
});
