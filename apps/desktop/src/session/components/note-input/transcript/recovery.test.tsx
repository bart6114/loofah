import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { IncompleteTranscript } from "./recovery";

const mocks = vi.hoisted(() => ({
  retry: vi.fn(),
  stop: vi.fn(),
  listen: vi.fn(),
  regenerate: vi.fn(),
  unlisten: vi.fn(),
}));
vi.mock("@hypr/plugin-transcription", () => ({
  commands: { retryMissingAudio: mocks.retry, stopTranscription: mocks.stop },
  events: { transcriptionEvent: { listen: mocks.listen } },
}));
vi.mock("./actions", () => ({
  useRegenerateTranscript: () => mocks.regenerate,
}));
vi.mock("~/stt/queries", () => ({
  useSessionTranscriptMetadata: () => ({
    data: {
      batch_recoveries: [
        {
          version: 1,
          run_id: "saved-run",
          status: "incomplete",
          unresolved: [
            {
              channel: 1,
              start_sample: 16000,
              end_sample: 48000,
              reason: "Decoder failed",
            },
          ],
        },
      ],
    },
  }),
}));

afterEach(cleanup);

let deliver: (event: { payload: Record<string, unknown> }) => void;
beforeEach(() => {
  vi.clearAllMocks();
  mocks.retry.mockResolvedValue({ status: "ok", data: null });
  mocks.stop.mockResolvedValue({ status: "ok", data: null });
  mocks.listen.mockImplementation(async (handler) => {
    deliver = handler;
    return mocks.unlisten;
  });
});
function mount() {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { mutations: { retry: false } } })
      }
    >
      <p>Recovered words stay readable</p>
      <IncompleteTranscript sessionId="session-1" />
    </QueryClientProvider>,
  );
}

test("restores missing ranges without starting recovery and keeps text visible during retry", async () => {
  mount();
  expect(screen.getByText("Incomplete transcript")).toBeTruthy();
  expect(screen.getByText("Channel 2: 0:01–0:03")).toBeTruthy();
  expect(mocks.retry).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("Retry missing audio"));
  await waitFor(() =>
    expect(mocks.retry).toHaveBeenCalledWith("session-1", "saved-run"),
  );
  expect(screen.getByText("Recovered words stay readable")).toBeTruthy();
  expect(screen.getByText("Retrying missing audio…")).toBeTruthy();
  await act(async () =>
    deliver({ payload: { type: "completed", session_id: "session-1" } }),
  );
  await waitFor(() => expect(mocks.unlisten).toHaveBeenCalledOnce());
});

test("explains conflicts and offers full retranscription", async () => {
  mocks.retry.mockResolvedValue({
    status: "error",
    error: "The recording changed. Transcribe the full recording again.",
  });
  mount();
  fireEvent.click(screen.getByText("Retry missing audio"));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain(
      "recording changed",
    ),
  );
  fireEvent.click(screen.getByText("Transcribe full recording again"));
  expect(mocks.regenerate).toHaveBeenCalledOnce();
  expect(screen.getByText("Recovered words stay readable")).toBeTruthy();
});
