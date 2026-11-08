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

import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultRecordingProcessing } from "./vault-recording-processing";
import { VaultScope } from "./vault-scope";

import type { LocalCaptureStatus } from "~/types/tauri.gen";

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
  const api = {
    list: vi.fn(async () => [status]),
    recover: vi.fn(async () => status),
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
  await waitFor(() => expect(f.api.list).toHaveBeenCalled());
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
