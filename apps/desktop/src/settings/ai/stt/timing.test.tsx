import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  config: {
    current_stt_provider: "fmtr",
    current_stt_model: "soniqo-parakeet-streaming",
    transcription_timing: "live",
    ai_language: "en",
    spoken_languages: [] as string[],
    meeting_languages: ["en"] as string[],
  },
  save: vi.fn(),
}));
vi.mock("~/shared/config", () => ({ useConfigValues: () => mocks.config }));
vi.mock("~/settings/queries", () => ({ setSettingValues: mocks.save }));

import { TranscriptionTiming } from "./timing";

function renderTiming() {
  const client = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  const element = () => (
    <QueryClientProvider client={client}>
      <TranscriptionTiming />
    </QueryClientProvider>
  );
  const view = render(element());
  return { rerender: () => view.rerender(element()) };
}

function radio(name: string) {
  return screen.getByRole("radio", {
    name: new RegExp(name),
  }) as HTMLInputElement;
}

beforeEach(() => {
  Object.assign(mocks.config, {
    current_stt_model: "soniqo-parakeet-streaming",
    transcription_timing: "live",
    meeting_languages: ["en"],
  });
  mocks.save.mockReset().mockImplementation(async (values) => {
    Object.assign(mocks.config, values);
  });
});
afterEach(cleanup);

test("saves either timing choice for a live-capable model", async () => {
  renderTiming();
  expect(radio("While recording").checked).toBe(true);
  fireEvent.click(radio("After recording"));
  await waitFor(() => expect(radio("After recording").checked).toBe(true));
  expect(mocks.save).toHaveBeenCalledWith(
    { transcription_timing: "batch" },
    expect.anything(),
  );
  fireEvent.click(radio("While recording"));
  await waitFor(() => expect(radio("While recording").checked).toBe(true));
  expect(mocks.save).toHaveBeenLastCalledWith(
    { transcription_timing: "live" },
    expect.anything(),
  );
});

test("preserves a live preference across model and language fallbacks", () => {
  const view = renderTiming();
  mocks.config.current_stt_model = "soniqo-parakeet-batch";
  view.rerender();
  expect(radio("While recording").disabled).toBe(true);
  expect(radio("After recording").checked).toBe(true);
  expect(
    screen.getByText("This model transcribes after recording."),
  ).toBeTruthy();
  mocks.config.current_stt_model = "soniqo-parakeet-streaming";
  mocks.config.meeting_languages = ["en", "nl"];
  view.rerender();
  expect(radio("While recording").disabled).toBe(true);
  expect(
    screen.getByText(/Live transcription supports English only/),
  ).toBeTruthy();
  mocks.config.meeting_languages = ["en"];
  view.rerender();
  expect(radio("While recording").checked).toBe(true);
  expect(radio("While recording").disabled).toBe(false);
  expect(mocks.save).not.toHaveBeenCalled();
});

test("lets users deliberately keep after recording while live is unavailable", async () => {
  mocks.config.current_stt_model = "soniqo-parakeet-batch";
  const view = renderTiming();
  fireEvent.click(radio("After recording"));
  await waitFor(() => expect(mocks.config.transcription_timing).toBe("batch"));
  mocks.config.current_stt_model = "soniqo-parakeet-streaming";
  view.rerender();
  expect(radio("After recording").checked).toBe(true);
});

test("keeps the saved choice and reports a failed write", async () => {
  mocks.save.mockRejectedValue(new Error("Could not save settings"));
  renderTiming();
  fireEvent.click(radio("After recording"));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toBe(
      "Could not save settings",
    ),
  );
  expect(radio("While recording").checked).toBe(true);
});

test("allows live Whisper with Dutch and English while preserving the saved timing", async () => {
  mocks.config.current_stt_model = "whisper-large-v3";
  mocks.config.meeting_languages = ["nl", "en"];
  mocks.config.transcription_timing = "batch";
  renderTiming();
  expect(radio("While recording").disabled).toBe(false);
  expect(radio("After recording").checked).toBe(true);
  fireEvent.click(radio("While recording"));
  await waitFor(() => expect(radio("While recording").checked).toBe(true));
});

test("English-only Whisper allows English live but falls back for Dutch", () => {
  mocks.config.current_stt_model = "QuantizedSmallEn";
  const view = renderTiming();
  expect(radio("While recording").disabled).toBe(false);
  mocks.config.meeting_languages = ["nl"];
  view.rerender();
  expect(radio("While recording").disabled).toBe(true);
});
