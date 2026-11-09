import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

vi.mock("~/types/tauri.gen", () => ({ commands: {} }));

import { ApplicationRequestError } from "./errors";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultRecordingProcessing } from "./vault-recording-processing";
import { VaultScope } from "./vault-scope";

import type {
  LocalCaptureControlSnapshot,
  LocalCaptureStatus,
  LocalCaptureResolution,
} from "~/types/tauri.gen";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});

async function fixture() {
  let event!: (value: VaultClientEvent) => void;
  const status: LocalCaptureStatus = {
    declaration: {
      request_id: crypto.randomUUID(),
      session_id: "note",
      append: false,
      batch_requested: true,
      transcription: { model: "local", languages: [], keywords: [] },
      summary: null,
    },
    state: "review",
    transcription_owner: "00000000-0000-0000-0000-000000000000",
    summary_owner: "desktop",
    transcript_id: null,
    summary_job_id: null,
    message: "Original result is retained",
  };
  const control: LocalCaptureControlSnapshot = {
    status,
    revision: "a".repeat(64),
    actions: ["recover"],
    resolution: null,
  };
  const api = {
    controls: vi.fn(async () => [control]),
    cancel: vi.fn(async () => control),
    recover: vi.fn(async () => status),
    resolve: vi.fn(
      async (
        _request: string,
        input: LocalCaptureResolution,
        _options?: { signal?: AbortSignal },
      ) => {
        control.status = { ...status, state: "closed" };
        control.actions = [];
        control.resolution = input;
        return structuredClone(control);
      },
    ),
  };
  const client = {
    scope: "original",
    canEdit: true,
    networkMode: "always",
    features: ["local_recording_processing"],
    recordingProcessing: api,
    subscribe: async (listener: (value: VaultClientEvent) => void) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: "always" },
    mutations: { retry: false, networkMode: "always" },
  });
  scopes.push(scope);
  await scope.start();
  return {
    scope,
    api,
    status,
    control,
    emit: (value: VaultClientEvent) => event(value),
  };
}

it("only resumes the original durable capture when recovery is explicitly requested", async () => {
  const f = await fixture();
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  const button = await screen.findByRole("button", {
    name: "Recover recording processing",
  });
  expect(f.api.recover).not.toHaveBeenCalled();
  fireEvent.click(button);
  await waitFor(() =>
    expect(f.api.recover).toHaveBeenCalledWith(
      f.status.declaration.request_id,
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    ),
  );
});

it("never displays parent jobs from a different session", async () => {
  const f = await fixture();
  f.status.declaration.session_id = "other";
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  await waitFor(() => expect(f.api.controls).toHaveBeenCalled());
  expect(screen.queryByText("Original result is retained")).toBeNull();
  expect(f.api.recover).not.toHaveBeenCalled();
});

it("cannot resume a retained capture after access is lost", async () => {
  const f = await fixture();
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  await screen.findByRole("button", { name: "Recover recording processing" });
  act(() =>
    f.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  expect(
    screen.queryByRole("button", { name: "Recover recording processing" }),
  ).toBeNull();
  expect(f.api.recover).not.toHaveBeenCalled();
});

it.each(["conflict", "review"] as const)(
  "does not invent a recovery action for %s",
  async (state) => {
    const f = await fixture();
    f.status.state = state;
    f.control.actions = [];
    render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
    await screen.findByText(f.status.message!);
    expect(
      screen.queryByRole("button", { name: "Recover recording processing" }),
    ).toBeNull();
    expect(f.api.recover).not.toHaveBeenCalled();
  },
);

it("cancels only the original processing request and shows its actual drain state", async () => {
  const f = await fixture();
  f.status.state = "transcribing";
  f.control.actions = ["cancel"];
  f.api.cancel.mockImplementation(async () => {
    f.status.state = "cancelling";
    f.control.actions = [];
    return { ...f.control, status: { ...f.status } };
  });
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Cancel processing" }),
  );
  await screen.findByText("Cancelling recording processing");
  expect(f.api.cancel).toHaveBeenCalledTimes(1);
  expect(f.api.cancel).toHaveBeenCalledWith(
    f.status.declaration.request_id,
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(f.api.recover).not.toHaveBeenCalled();
  expect(
    screen.queryByRole("button", { name: "Cancel processing" }),
  ).toBeNull();
  expect(screen.queryByText("Recording processing cancelled")).toBeNull();
});

it("never cancels processing after the original scope loses access", async () => {
  const f = await fixture();
  f.status.state = "transcribing";
  f.control.actions = ["cancel"];
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  await screen.findByRole("button", { name: "Cancel processing" });
  act(() =>
    f.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  expect(
    screen.queryByRole("button", { name: "Cancel processing" }),
  ).toBeNull();
  expect(f.api.cancel).not.toHaveBeenCalled();
});

it("keeps processing cancellation disabled for a read-only scope", async () => {
  const f = await fixture();
  f.status.state = "summarizing";
  f.control.actions = ["cancel"];
  Object.assign(f.scope.client, { canEdit: false });
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  expect(
    (
      await screen.findByRole("button", { name: "Cancel processing" })
    ).getAttribute("disabled"),
  ).not.toBeNull();
  expect(f.api.cancel).not.toHaveBeenCalled();
});

async function resolutionFixture() {
  const f = await fixture();
  f.status.state = "conflict";
  f.control.actions = ["keep_current_content"];
  return f;
}
async function reviewConflict() {
  fireEvent.click(
    await screen.findByRole("button", { name: "Review processing conflict" }),
  );
  await screen.findByRole("dialog");
}
async function confirmResolution() {
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(
    screen.getByRole("button", {
      name: "Keep current content and close processing",
    }),
  );
}

it("requires explicit reviewed confirmation before closing only the original conflict", async () => {
  const f = await resolutionFixture();
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  await reviewConflict();
  expect(
    screen.getByText(
      /Current notes, images, transcript, summary, audio, retained results and journals stay unchanged/,
    ),
  ).toBeTruthy();
  expect(f.api.resolve).not.toHaveBeenCalled();
  expect(
    screen
      .getByRole("button", {
        name: "Keep current content and close processing",
      })
      .getAttribute("disabled"),
  ).not.toBeNull();
  await confirmResolution();
  await screen.findByText("Recording processing closed by review");
  expect(f.api.resolve).toHaveBeenCalledOnce();
  expect(f.api.resolve).toHaveBeenCalledWith(
    f.status.declaration.request_id,
    {
      resolution_id: expect.any(String),
      expected_revision: "a".repeat(64),
      decision: "keep_current_content",
    },
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(f.api.cancel).not.toHaveBeenCalled();
  expect(f.api.recover).not.toHaveBeenCalled();
});

it("retries an unknown outcome with the same reviewed UUID and revision despite query refresh", async () => {
  const f = await resolutionFixture();
  f.api.resolve.mockRejectedValueOnce(new Error("Reply lost"));
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  await reviewConflict();
  await confirmResolution();
  await screen.findByText("Reply lost");
  const original = f.api.resolve.mock.calls[0][1];
  f.control.revision = "b".repeat(64);
  await act(() =>
    f.scope.queryClient.invalidateQueries({
      queryKey: ["recording_processing", "original", "note"],
    }),
  );
  expect(f.api.resolve).toHaveBeenCalledOnce();
  fireEvent.click(
    screen.getByRole("button", { name: "Retry original resolution" }),
  );
  await screen.findByText("Recording processing closed by review");
  expect(f.api.resolve.mock.calls[1][1]).toEqual(original);
  expect(f.api.resolve.mock.calls[1][1].expected_revision).toBe("a".repeat(64));
});

it("requires a refreshed explicit review after a confirmed stale revision", async () => {
  const f = await resolutionFixture();
  f.api.resolve.mockRejectedValueOnce(
    new ApplicationRequestError({
      code: "conflict",
      message: "State changed",
      retryable: false,
      request_id: null,
      current_revision: "b".repeat(64),
      mutation_outcome: "not_committed",
    }),
  );
  render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  await reviewConflict();
  await confirmResolution();
  await screen.findByText("State changed");
  expect(
    screen
      .getByRole("button", { name: "Retry original resolution" })
      .getAttribute("disabled"),
  ).not.toBeNull();
  const original = f.api.resolve.mock.calls[0][1];
  f.control.revision = "b".repeat(64);
  fireEvent.click(
    screen.getByRole("button", { name: "Refresh and review again" }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(f.api.controls).toHaveBeenCalledTimes(2));
  expect(f.api.resolve).toHaveBeenCalledOnce();
  await reviewConflict();
  await confirmResolution();
  await screen.findByText("Recording processing closed by review");
  expect(f.api.resolve.mock.calls[1][1].resolution_id).not.toBe(
    original.resolution_id,
  );
  expect(f.api.resolve.mock.calls[1][1].expected_revision).toBe("b".repeat(64));
});

it.each(["reconnecting", "access_denied"] as const)(
  "does not resolve after original access changes to %s",
  async (state) => {
    const f = await resolutionFixture();
    render(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
    await reviewConflict();
    fireEvent.click(screen.getByRole("checkbox"));
    act(() => f.emit({ kind: "connection", state, error: null }));
    const confirm = screen.queryByRole("button", {
      name: "Keep current content and close processing",
    });
    if (confirm) {
      expect(confirm.getAttribute("disabled")).not.toBeNull();
      fireEvent.click(confirm);
    }
    expect(f.api.resolve).not.toHaveBeenCalled();
  },
);

it("never offers unknown-producer resolution or enables a read-only conflict decision", async () => {
  const f = await fixture();
  f.control.actions = [];
  const view = render(
    <VaultRecordingProcessing scope={f.scope} sessionId="note" />,
  );
  await screen.findByText(f.status.message!);
  expect(
    screen.queryByRole("button", { name: "Review processing conflict" }),
  ).toBeNull();
  f.status.state = "conflict";
  f.control.actions = ["keep_current_content"];
  Object.assign(f.scope.client, { canEdit: false });
  await act(() =>
    f.scope.queryClient.invalidateQueries({
      queryKey: ["recording_processing", "original", "note"],
    }),
  );
  view.rerender(<VaultRecordingProcessing scope={f.scope} sessionId="note" />);
  expect(
    (
      await screen.findByRole("button", { name: "Review processing conflict" })
    ).getAttribute("disabled"),
  ).not.toBeNull();
  expect(f.api.resolve).not.toHaveBeenCalled();
});

it("aborts the original resolution lifetime on navigation and never applies late UI success", async () => {
  const f = await resolutionFixture();
  let finish!: (value: LocalCaptureControlSnapshot) => void;
  f.api.resolve.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(
    <VaultRecordingProcessing scope={f.scope} sessionId="note" />,
  );
  await reviewConflict();
  await confirmResolution();
  await waitFor(() => expect(f.api.resolve).toHaveBeenCalledOnce());
  const signal = f.api.resolve.mock.calls[0][2]!.signal!;
  view.rerender(
    <VaultRecordingProcessing scope={f.scope} sessionId="another" />,
  );
  expect(signal.aborted).toBe(true);
  const closed: LocalCaptureControlSnapshot = {
    ...f.control,
    status: { ...f.status, state: "closed" },
    resolution: f.api.resolve.mock.calls[0][1],
    actions: [],
  };
  await act(async () => finish(closed));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(
    screen.queryByText("Recording processing closed by review"),
  ).toBeNull();
  expect(f.api.resolve).toHaveBeenCalledOnce();
});
