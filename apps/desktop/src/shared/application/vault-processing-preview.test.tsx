import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

vi.mock("~/types/tauri.gen", () => ({ commands: {} }));

import { ApplicationRequestError } from "./errors";
import type {
  ProcessingJobPreview,
  ProcessingJobSnapshot,
} from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultProcessingPreview } from "./vault-processing-preview";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.restoreAllMocks();
});
const job: ProcessingJobSnapshot = {
  id: "8af22194-6a2d-41ae-8c5f-5b97aa1e50db",
  session_id: "note",
  target: { kind: "summary" },
  state: "running",
  phase: "generating",
  failure: null,
  created_at: "2026-11-08T00:00:00Z",
  updated_at: "2026-11-08T00:00:00Z",
};
function snapshot(markdown = "Partial original text"): ProcessingJobPreview {
  return {
    vault_id: "vault",
    job: structuredClone(job),
    preview: {
      cursor: "c03e735b-cb8a-46ea-bcd7-cfbd49467264:1",
      markdown,
      truncated: false,
    },
  };
}
async function fixture(enabled = true) {
  let receive!: (event: VaultClientEvent) => void;
  const read = vi.fn<NonNullable<VaultClient["processingJobPreview"]>>(
    async () => snapshot(),
  );
  const scope = new VaultScope({
    scope: "same-string-scope",
    features: enabled ? ["processing_previews"] : [],
    networkMode: "always",
    processingJobPreview: enabled ? read : undefined,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      receive = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient);
  scopes.push(scope);
  await scope.start();
  return { scope, read, emit: (event: VaultClientEvent) => receive(event) };
}

it("shows unsaved partial text, coalesces notifications and keeps completed text in the result path", async () => {
  const f = await fixture();
  const view = render(<VaultProcessingPreview scope={f.scope} job={job} />);
  await screen.findByText("Partial original text");
  expect(
    screen.getByText("Generating… Partial text is not saved."),
  ).toBeTruthy();
  const canonical = [
    f.scope.noteOptions("note").queryKey,
    ["vault", f.scope.client.scope, "processing_jobs"],
  ];
  for (const key of canonical)
    f.scope.queryClient.setQueryData(key, { unchanged: true });
  f.read.mockResolvedValue(snapshot("Latest coalesced text"));
  act(() => {
    for (let i = 0; i < 1000; i++)
      f.emit({
        kind: "change",
        change: { kind: "job_preview_changed", job_id: job.id },
      });
  });
  await screen.findByText("Latest coalesced text");
  expect(f.read).toHaveBeenCalledTimes(2);
  for (const key of canonical)
    expect(f.scope.queryClient.getQueryState(key)?.isInvalidated).toBe(false);
  view.rerender(
    <VaultProcessingPreview
      scope={f.scope}
      job={{ ...job, state: "succeeded" }}
    />,
  );
  expect(screen.queryByText("Latest coalesced text")).toBeNull();
});

it("retains clearly incomplete terminal text and refuses another target", async () => {
  const f = await fixture();
  const retained = snapshot();
  retained.job.state = "cancelled";
  retained.preview!.truncated = true;
  f.read.mockResolvedValue(retained);
  const view = render(
    <VaultProcessingPreview scope={f.scope} job={retained.job} />,
  );
  await screen.findByText("Partial original text");
  expect(
    screen.getByText("Incomplete generation. This partial text was not saved."),
  ).toBeTruthy();
  expect(screen.getByText("The partial preview is shortened.")).toBeTruthy();
  view.rerender(
    <VaultProcessingPreview
      scope={f.scope}
      job={{
        ...retained.job,
        target: { kind: "template_output", document_id: "other" },
      }}
    />,
  );
  expect(screen.queryByText("Partial original text")).toBeNull();
});

it("hides cached text on disconnect and a forbidden refresh clears the original cache", async () => {
  const f = await fixture();
  render(<VaultProcessingPreview scope={f.scope} job={job} />);
  await screen.findByText("Partial original text");
  act(() => f.emit({ kind: "connection", state: "reconnecting", error: null }));
  expect(screen.queryByText("Partial original text")).toBeNull();
  f.read.mockRejectedValue(
    new ApplicationRequestError({
      code: "forbidden",
      message: "Access removed",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: null,
    }),
  );
  act(() => f.emit({ kind: "connection", state: "connected", error: null }));
  await waitFor(() =>
    expect(f.scope.getSnapshot().state).toBe("access_denied"),
  );
  expect(screen.queryByText("Partial original text")).toBeNull();
  expect(
    f.scope.queryClient.getQueryData(
      f.scope.processingJobPreviewOptions(job.id).queryKey,
    ),
  ).toBeUndefined();
});

it("same-string scope replacement cannot show a late response from the original owner", async () => {
  const old = await fixture();
  let resolve!: (value: ProcessingJobPreview) => void;
  old.read.mockReturnValue(
    new Promise((yes) => {
      resolve = yes;
    }),
  );
  const view = render(<VaultProcessingPreview scope={old.scope} job={job} />);
  await waitFor(() => expect(old.read).toHaveBeenCalledOnce());
  const fresh = await fixture();
  fresh.read.mockResolvedValue(snapshot("New owner text"));
  view.rerender(<VaultProcessingPreview scope={fresh.scope} job={job} />);
  await screen.findByText("New owner text");
  await act(async () => resolve(snapshot("Old owner late text")));
  expect(screen.queryByText("Old owner late text")).toBeNull();
  expect(screen.getByText("New owner text")).toBeTruthy();
});

it("missing capability never reads a preview", async () => {
  const f = await fixture(false);
  render(<VaultProcessingPreview scope={f.scope} job={job} />);
  expect(f.read).not.toHaveBeenCalled();
  expect(screen.queryByRole("region")).toBeNull();
});
