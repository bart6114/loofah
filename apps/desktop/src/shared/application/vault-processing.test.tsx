import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  deviceJobSubmissions: vi.fn(),
  deviceJobSubmissionPrepare: vi.fn(),
  deviceJobSubmissionGet: vi.fn(),
  deviceJobSubmissionObserve: vi.fn(),
  deviceJobSubmissionForget: vi.fn(),
  copy: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: native.copy,
}));

import type {
  ProcessingJobSnapshot,
  ListProcessingJobsInput,
  SummaryJobRequest,
} from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultProcessing } from "./vault-processing";
import { VaultScope } from "./vault-scope";

import type { JobSubmissionReceipt } from "~/types/tauri.gen";

const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});
beforeEach(() => {
  vi.resetAllMocks();
  native.copy.mockResolvedValue(undefined);
});

async function activate(client: VaultClient) {
  const scope = new VaultScope(client);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, staleTime: Infinity, networkMode: "always" },
    mutations: { retry: false, networkMode: "always" },
  });
  scopes.push(scope);
  await scope.start();
  return scope;
}

async function fixture(
  options: { canEdit?: boolean; features?: string[] } = {},
) {
  let event!: (event: VaultClientEvent) => void;
  const data = {
    receipts: [] as JobSubmissionReceipt[],
    jobs: [] as ProcessingJobSnapshot[],
  };
  const job: ProcessingJobSnapshot = {
    id: crypto.randomUUID(),
    session_id: "note",
    target: { kind: "summary" },
    state: "succeeded",
    phase: null,
    failure: null,
    created_at: "2026-11-07T10:00:00Z",
    updated_at: "2026-11-07T10:00:00Z",
  };
  native.deviceJobSubmissions.mockImplementation(
    async (scope: string, sessionId: string | null) => ({
      status: "ok",
      data: structuredClone(
        data.receipts.filter(
          (receipt) =>
            receipt.scope === scope &&
            (!sessionId || receipt.input.session_id === sessionId),
        ),
      ),
    }),
  );
  native.deviceJobSubmissionPrepare.mockImplementation(
    async (scope: string, input: SummaryJobRequest) => {
      const receipt = {
        scope,
        input,
        request_id: crypto.randomUUID(),
        created_at_ms: Date.now(),
        job: null,
      };
      data.receipts.push(receipt);
      return { status: "ok", data: receipt };
    },
  );
  native.deviceJobSubmissionGet.mockImplementation(
    async (scope: string, id: string) => ({
      status: "ok",
      data: structuredClone(
        data.receipts.find(
          (receipt) => receipt.scope === scope && receipt.request_id === id,
        ) ?? null,
      ),
    }),
  );
  native.deviceJobSubmissionObserve.mockImplementation(
    async (_scope: string, id: string, job: ProcessingJobSnapshot) => {
      const receipt = data.receipts.find(
        (receipt) => receipt.request_id === id,
      )!;
      receipt.job = job;
      return { status: "ok", data: receipt };
    },
  );
  native.deviceJobSubmissionForget.mockImplementation(
    async (_scope: string, id: string) => {
      data.receipts = data.receipts.filter(
        (receipt) => receipt.request_id !== id,
      );
      return { status: "ok", data: true };
    },
  );
  const calls = {
    processingJobs: vi.fn(async (input: ListProcessingJobsInput) => ({
      items: data.jobs.filter(
        (job) =>
          !input.active_only ||
          ["queued", "running", "cancelling"].includes(job.state),
      ),
      next_cursor: null,
    })),
    readDocument: vi.fn(async () => ({ document: null, revision: "baseline" })),
    submitSummaryJob: vi.fn(
      async (_input: SummaryJobRequest, _options: unknown) => {
        data.jobs = [job];
        return job;
      },
    ),
    processingJob: vi.fn(async () => job),
    cancelProcessingJob: vi.fn(async () => {
      const cancelled = { ...job, state: "cancelled" as const };
      data.jobs = [cancelled];
      return cancelled;
    }),
    processingJobResult: vi.fn(async () => ({
      vault_id: "vault",
      job_id: job.id,
      session_id: "note",
      target: job.target,
      markdown: "# Recovered\nKeep this result.",
      suggested_tags: null,
    })),
  };
  const client = {
    scope: crypto.randomUUID(),
    networkMode: "always",
    canEdit: options.canEdit ?? true,
    features: options.features ?? ["summary_body_jobs"],
    ...calls,
    subscribe: async (listener: (value: VaultClientEvent) => void) => {
      event = listener;
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = await activate(client);
  return {
    client,
    calls,
    scope,
    job,
    data,
    emit: (value: VaultClientEvent) => event(value),
  };
}

it("hides unsupported processing without touching device request storage", async () => {
  const { scope } = await fixture({ features: [] });
  render(<VaultProcessing scope={scope} sessionId="note" />);
  expect(screen.queryByRole("region")).toBeNull();
  expect(native.deviceJobSubmissions).not.toHaveBeenCalled();
});

it("persists before dispatch and disables repeat clicks while the device write is pending", async () => {
  const { scope, calls } = await fixture();
  const prepare = native.deviceJobSubmissionPrepare.getMockImplementation()!;
  let release!: () => void;
  native.deviceJobSubmissionPrepare.mockImplementationOnce(async (...args) => {
    await new Promise<void>((resolve) => {
      release = resolve;
    });
    return prepare(...args);
  });
  render(<VaultProcessing scope={scope} sessionId="note" />);
  const generate = screen.getByRole("button", {
    name: "Summarize saved notes",
  });
  await waitFor(() => expect(generate.hasAttribute("disabled")).toBe(false));
  fireEvent.click(generate);
  await waitFor(() =>
    expect(native.deviceJobSubmissionPrepare).toHaveBeenCalledOnce(),
  );
  expect(generate.hasAttribute("disabled")).toBe(true);
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
  release();
  await screen.findByText("Summary saved");
  expect(calls.submitSummaryJob).toHaveBeenCalledOnce();
  expect(native.deviceJobSubmissionObserve).toHaveBeenCalledOnce();
});

it("loads a saved unresolved request after remount without auto-dispatch and resumes the same UUID", async () => {
  const { scope, client, calls, data, job } = await fixture();
  calls.submitSummaryJob.mockImplementationOnce(async () => {
    data.jobs = [job];
    throw new Error("Reply was lost");
  });
  const mounted = render(<VaultProcessing scope={scope} sessionId="note" />);
  const generate = screen.getByRole("button", {
    name: "Summarize saved notes",
  });
  await waitFor(() => expect(generate.hasAttribute("disabled")).toBe(false));
  fireEvent.click(generate);
  await screen.findByText("Reply was lost");
  const original = structuredClone(data.receipts[0]);
  mounted.unmount();
  scope.dispose();
  const restarted = await activate(client);
  render(<VaultProcessing scope={restarted} sessionId="note" />);
  const resume = await screen.findByRole("button", { name: "Resume request" });
  expect(calls.submitSummaryJob).toHaveBeenCalledOnce();
  expect(
    screen
      .getByRole("button", { name: "Summarize saved notes" })
      .hasAttribute("disabled"),
  ).toBe(true);
  fireEvent.click(resume);
  await waitFor(() =>
    expect(screen.queryByRole("button", { name: "Resume request" })).toBeNull(),
  );
  expect(calls.submitSummaryJob.mock.calls[1]).toEqual([
    original.input,
    { requestId: original.request_id },
  ]);
  expect(native.deviceJobSubmissionPrepare).toHaveBeenCalledOnce();
  expect(screen.queryByText("Reply was lost")).toBeNull();
});

it("does not revive a finished job when an older active-list response arrives", async () => {
  const { scope, calls, job } = await fixture();
  calls.processingJobs.mockImplementation(async (input) => ({
    items: [{ ...job, state: input.active_only ? "running" : "succeeded" }],
    next_cursor: null,
  }));
  render(<VaultProcessing scope={scope} sessionId="note" />);
  await screen.findByText("Summary saved");
  expect(screen.queryByRole("button", { name: "Cancel summary" })).toBeNull();
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "Summarize saved notes" })
        .hasAttribute("disabled"),
    ).toBe(false),
  );
});

it("allows a downgraded reader to cancel their existing job while generation stays disabled", async () => {
  const { scope, calls, data, job } = await fixture({ canEdit: false });
  data.jobs = [{ ...job, state: "running", phase: "generating" }];
  render(<VaultProcessing scope={scope} sessionId="note" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Cancel summary" }),
  );
  await screen.findByText("Cancelled");
  expect(calls.cancelProcessingJob).toHaveBeenCalledWith(job.id, undefined);
  expect(
    screen
      .getByRole("button", { name: "Summarize saved notes" })
      .hasAttribute("disabled"),
  ).toBe(true);
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
});

it("revalidates a job notification and copies a retained result without replacing current content", async () => {
  const { scope, calls, data, job, emit } = await fixture();
  data.jobs = [{ ...job, state: "running", phase: "generating" }];
  render(<VaultProcessing scope={scope} sessionId="note" />);
  await screen.findByText("Generating summary");
  data.jobs = [{ ...job, state: "conflict" }];
  act(() =>
    emit({ kind: "change", change: { kind: "job_changed", job_id: job.id } }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "Generated text" }),
  );
  await screen.findByText(/Keep this result/);
  fireEvent.click(screen.getByRole("button", { name: "Copy generated text" }));
  await waitFor(() =>
    expect(native.copy).toHaveBeenCalledWith("# Recovered\nKeep this result."),
  );
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
  act(() => emit({ kind: "connection", state: "access_denied", error: null }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(
    scope.queryClient.getQueryData(
      scope.processingJobResultOptions(job.id).queryKey,
    ),
  ).toBeUndefined();
});

it("requires explicit confirmation to forget an unresolved request and does not cancel or generate", async () => {
  const { scope, client, calls, data } = await fixture();
  data.receipts = [
    {
      scope: client.scope,
      input: {
        session_id: "note",
        target: { kind: "summary" },
        expected_revision: "baseline",
      },
      request_id: crypto.randomUUID(),
      created_at_ms: Date.now(),
      job: null,
    },
  ];
  const requestId = data.receipts[0].request_id;
  render(<VaultProcessing scope={scope} sessionId="note" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Forget device request" }),
  );
  expect(native.deviceJobSubmissionForget).not.toHaveBeenCalled();
  const dialog = await screen.findByRole("dialog");
  expect(within(dialog).getByText(/does not stop processing/)).toBeTruthy();
  fireEvent.click(
    within(dialog).getByRole("button", { name: "Forget device request" }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(native.deviceJobSubmissionForget).toHaveBeenCalledWith(
    client.scope,
    requestId,
  );
  expect(calls.cancelProcessingJob).not.toHaveBeenCalled();
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
});
