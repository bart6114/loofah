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
  draftCacheInventory: vi.fn(),
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
  DocumentSnapshot,
  ProcessingJobResult,
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
  native.draftCacheInventory.mockResolvedValue({ status: "ok", data: [] });
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
    readDocument: vi.fn<VaultClient["readDocument"]>(
      async (): Promise<DocumentSnapshot> => ({
        document: null,
        revision: "baseline",
      }),
    ),
    submitSummaryJob: vi.fn(
      async (input: SummaryJobRequest, _options: unknown) => {
        const submitted = { ...job, target: input.target };
        data.jobs = [submitted];
        return submitted;
      },
    ),
    processingJob: vi.fn(async () => job),
    cancelProcessingJob: vi.fn(async () => {
      const cancelled = { ...job, state: "cancelled" as const };
      data.jobs = [cancelled];
      return cancelled;
    }),
    processingJobResult: vi.fn(
      async (): Promise<ProcessingJobResult | null> => ({
        vault_id: "vault",
        job_id: job.id,
        session_id: "note",
        target: job.target,
        markdown: "# Recovered\nKeep this result.",
        suggested_tags: null,
      }),
    ),
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

it("only opts into title generation when explicitly checked and captures that policy before device persistence", async () => {
  const { scope, calls, data } = await fixture({
    features: ["summary_body_jobs", "generated_titles"],
  });
  render(<VaultProcessing scope={scope} sessionId="note" />);
  const checkbox = screen.getByRole("checkbox", {
    name: "Generate a title if this note is untitled",
  });
  expect(checkbox).toHaveProperty("checked", false);
  const generate = screen.getByRole("button", {
    name: "Summarize saved notes",
  });
  await waitFor(() => expect(generate).toHaveProperty("disabled", false));
  fireEvent.click(checkbox);
  fireEvent.click(generate);
  await screen.findByText("Summary saved");
  expect(data.receipts[0].input).toEqual({
    session_id: "note",
    target: { kind: "summary" },
    expected_revision: "baseline",
    title_policy: "generate_if_empty",
  });
  expect(calls.submitSummaryJob.mock.calls[0][0]).toEqual(
    data.receipts[0].input,
  );
  expect(calls.readDocument.mock.calls[0][2]?.signal).toBeInstanceOf(
    AbortSignal,
  );
});

it.each([
  { canEdit: true, features: ["summary_body_jobs"] },
  { canEdit: false, features: ["summary_body_jobs", "generated_titles"] },
])(
  "does not offer generated titles without support and write permission $canEdit/$features",
  async (options) => {
    const { scope } = await fixture(options);
    render(<VaultProcessing scope={scope} sessionId="note" />);
    expect(screen.queryByRole("checkbox")).toBeNull();
  },
);

it("keeps default preserve requests byte-compatible even when generated titles are supported", async () => {
  const { scope, data } = await fixture({
    features: ["summary_body_jobs", "generated_titles"],
  });
  render(<VaultProcessing scope={scope} sessionId="note" />);
  const button = screen.getByRole("button", { name: "Summarize saved notes" });
  await waitFor(() => expect(button).toHaveProperty("disabled", false));
  fireEvent.click(button);
  await screen.findByText("Summary saved");
  expect(data.receipts[0].input).not.toHaveProperty("title_policy");
});

it.each(["generating", "applying"] as const)(
  "keeps the saved body inspectable during the optional title %s stage without eager result requests",
  async (status) => {
    const { scope, calls, job, data } = await fixture({
      features: ["summary_body_jobs", "generated_titles"],
    });
    data.jobs = [{ ...job, state: "running", phase: "applying" }];
    calls.processingJobResult.mockResolvedValue({
      vault_id: "vault",
      job_id: job.id,
      session_id: "note",
      target: job.target,
      markdown: "Saved body",
      title: { status },
      suggested_tags: null,
    });
    render(<VaultProcessing scope={scope} sessionId="note" />);
    await screen.findByText("Processing request");
    expect(screen.queryByText("Saving summary")).toBeNull();
    expect(calls.processingJobResult).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Cancel processing" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Generated text" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("Saved body");
    expect(within(dialog).getByText("Summary saved")).toBeTruthy();
    expect(
      within(dialog).getByText(
        status === "generating" ? "Generating title..." : "Saving title…",
      ),
    ).toBeTruthy();
  },
);

it("reports title failure separately from body success and only copies recovered title candidates", async () => {
  const { scope, calls, job, data } = await fixture({
    features: ["summary_body_jobs", "generated_titles"],
  });
  data.jobs = [job];
  calls.processingJobResult.mockResolvedValue({
    vault_id: "vault",
    job_id: job.id,
    session_id: "note",
    target: job.target,
    markdown: "Saved summary",
    title: {
      status: "failed",
      candidate: "Recovered title",
      failure: "storage",
    },
    suggested_tags: null,
  });
  render(<VaultProcessing scope={scope} sessionId="note" />);
  await screen.findByText("Summary saved");
  fireEvent.click(screen.getByRole("button", { name: "Generated text" }));
  const dialog = await screen.findByRole("dialog");
  await within(dialog).findByText("Title generation failed.");
  expect(within(dialog).getByText("Saved summary")).toBeTruthy();
  expect(screen.queryByText("Summary failed")).toBeNull();
  fireEvent.click(
    within(dialog).getByRole("button", { name: "Copy generated title" }),
  );
  await waitFor(() =>
    expect(native.copy).toHaveBeenCalledWith("Recovered title"),
  );
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
  expect(calls.readDocument).not.toHaveBeenCalled();
  expect(native.deviceJobSubmissionPrepare).not.toHaveBeenCalled();
});

it("resets title opt-in when the session or immutable target changes", async () => {
  const { scope, calls } = await fixture({
    features: ["summary_body_jobs", "generated_titles", "template_output_jobs"],
  });
  const mounted = render(<VaultProcessing scope={scope} sessionId="note" />);
  const checkbox = screen.getByRole("checkbox");
  fireEvent.click(checkbox);
  expect(checkbox).toHaveProperty("checked", true);
  mounted.rerender(<VaultProcessing scope={scope} sessionId="other" />);
  expect(screen.getByRole("checkbox")).toHaveProperty("checked", false);
  fireEvent.click(screen.getByRole("checkbox"));
  mounted.rerender(
    <VaultProcessing
      scope={scope}
      sessionId="other"
      target={{ kind: "template_output", document_id: "output" }}
    />,
  );
  expect(screen.getByRole("checkbox")).toHaveProperty("checked", false);
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
});

it("never displays or caches a late private title result after access loss", async () => {
  const { scope, calls, job, data, emit } = await fixture({
    features: ["summary_body_jobs", "generated_titles"],
  });
  data.jobs = [job];
  let finish!: (result: ProcessingJobResult) => void;
  calls.processingJobResult.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  render(<VaultProcessing scope={scope} sessionId="note" />);
  await screen.findByText("Summary saved");
  fireEvent.click(screen.getByRole("button", { name: "Generated text" }));
  await waitFor(() => expect(calls.processingJobResult).toHaveBeenCalledOnce());
  act(() => emit({ kind: "connection", state: "access_denied", error: null }));
  await act(async () =>
    finish({
      vault_id: "vault",
      job_id: job.id,
      session_id: "note",
      target: job.target,
      markdown: "PRIVATE BODY",
      title: { status: "applied", candidate: "PRIVATE TITLE" },
      suggested_tags: null,
    }),
  );
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.queryByText("PRIVATE TITLE")).toBeNull();
  expect(
    scope.queryClient.getQueryData(
      scope.processingJobResultOptions(job.id).queryKey,
    ),
  ).toBeUndefined();
  expect(native.copy).not.toHaveBeenCalled();
});

it("isolates late historical title candidates from a replacement scope with identical IDs", async () => {
  const first = await fixture({
    features: ["summary_body_jobs", "generated_titles"],
  });
  first.data.jobs = [first.job];
  let finish!: (result: ProcessingJobResult) => void;
  first.calls.processingJobResult.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const mounted = render(
    <VaultProcessing scope={first.scope} sessionId="note" />,
  );
  await screen.findByText("Summary saved");
  fireEvent.click(screen.getByRole("button", { name: "Generated text" }));
  await waitFor(() =>
    expect(first.calls.processingJobResult).toHaveBeenCalledOnce(),
  );
  const canonical: ProcessingJobResult = {
    vault_id: "other-vault",
    job_id: first.job.id,
    session_id: "note",
    target: first.job.target,
    markdown: "Replacement body",
    title: { status: "already_named" },
    suggested_tags: null,
  };
  const second = await activate({
    ...first.client,
    processingJobs: vi.fn(async () => ({
      items: [first.job],
      next_cursor: null,
    })),
    processingJobResult: vi.fn(async () => canonical),
  });
  mounted.rerender(<VaultProcessing scope={second} sessionId="note" />);
  act(() => first.scope.dispose());
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(
    await screen.findByRole("button", { name: "Generated text" }),
  );
  await screen.findByText("Replacement body");
  await act(async () =>
    finish({
      ...canonical,
      markdown: "PRIVATE OLD BODY",
      title: { status: "applied", candidate: "PRIVATE OLD TITLE" },
    }),
  );
  expect(screen.queryByText("PRIVATE OLD TITLE")).toBeNull();
  expect(
    second.queryClient.getQueryData(
      second.processingJobResultOptions(first.job.id).queryKey,
    ),
  ).toEqual(canonical);
  expect(
    first.scope.queryClient.getQueryData(
      first.scope.processingJobResultOptions(first.job.id).queryKey,
    ),
  ).toBeUndefined();
});

it.each([
  { features: ["summary_body_jobs"], canEdit: true },
  { features: ["template_output_jobs"], canEdit: true },
  { features: ["summary_body_jobs", "template_library"], canEdit: true },
  { features: ["summary_body_jobs", "template_output_jobs"], canEdit: false },
])(
  "never offers output regeneration to unsupported connections or readers $features/$canEdit",
  async (options) => {
    const { scope, calls } = await fixture(options);
    render(
      <VaultProcessing
        scope={scope}
        sessionId="note"
        target={{ kind: "template_output", document_id: "output" }}
      />,
    );
    expect(screen.queryByRole("button", { name: "Regenerate" })).toBeNull();
    expect(calls.readDocument).not.toHaveBeenCalled();
    expect(calls.submitSummaryJob).not.toHaveBeenCalled();
  },
);

it("regenerates an existing target and keeps summary jobs and ambiguous receipts isolated", async () => {
  const { scope, client, calls, job, data } = await fixture({
    features: ["summary_body_jobs", "template_output_jobs"],
  });
  data.jobs = [{ ...job, state: "running" }];
  data.receipts = [
    {
      scope: client.scope,
      request_id: "summary-request",
      created_at_ms: Date.now(),
      input: {
        session_id: "note",
        target: { kind: "summary" },
        expected_revision: "summary-r0",
      },
      job: null,
    },
  ];
  const target = { kind: "template_output" as const, document_id: "output" };
  calls.readDocument.mockResolvedValue({
    document: {
      id: "output",
      session_id: "note",
      kind: "template_output",
      template_id: "template",
      title: "Keep output",
      sort_order: 8,
      markdown: "Saved output",
    },
    revision: "output-r0",
  });
  render(<VaultProcessing scope={scope} sessionId="note" target={target} />);
  const button = screen.getByRole("button", { name: "Regenerate" });
  await waitFor(() => expect(button.hasAttribute("disabled")).toBe(false));
  expect(screen.queryByRole("button", { name: "Resume request" })).toBeNull();
  expect(screen.queryByText("Preparing summary")).toBeNull();
  fireEvent.click(button);
  await screen.findByText("Summary saved");
  expect(calls.submitSummaryJob).toHaveBeenCalledWith(
    { session_id: "note", target, expected_revision: "output-r0" },
    { requestId: data.receipts[1].request_id, signal: expect.any(AbortSignal) },
  );
  expect(calls.submitSummaryJob).toHaveBeenCalledOnce();
});

it("remounts target status and errors when selection changes while a previous target read is delayed", async () => {
  const { scope, calls, job, data } = await fixture({
    features: ["summary_body_jobs", "template_output_jobs"],
  });
  let release!: (value: DocumentSnapshot) => void;
  calls.readDocument.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const target = { kind: "template_output" as const, document_id: "output" };
  const mounted = render(
    <VaultProcessing scope={scope} sessionId="note" target={target} />,
  );
  const button = screen.getByRole("button", { name: "Regenerate" });
  await waitFor(() => expect(button.hasAttribute("disabled")).toBe(false));
  fireEvent.click(button);
  await waitFor(() => expect(calls.readDocument).toHaveBeenCalledOnce());
  data.jobs = [
    {
      ...job,
      target: { kind: "summary" },
      state: "failed",
      failure: "source_unavailable",
    },
  ];
  mounted.rerender(<VaultProcessing scope={scope} sessionId="note" />);
  await screen.findByText("Saved notes or transcript could not be read.");
  await act(async () =>
    release({
      document: {
        id: "output",
        session_id: "note",
        kind: "template_output",
        title: "Output",
        template_id: "template",
        sort_order: 2,
        markdown: "Existing",
      },
      revision: "output-r0",
    }),
  );
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
  expect(screen.queryByText("Summary saved")).toBeNull();
  expect(screen.queryByRole("button", { name: "Regenerate" })).toBeNull();
});

it("replaces QueryObservers for a new immutable scope even when its scope string and note IDs match", async () => {
  const first = await fixture();
  first.data.jobs = [
    { ...first.job, state: "failed", failure: "provider_failed" },
  ];
  const mounted = render(
    <VaultProcessing scope={first.scope} sessionId="note" />,
  );
  await screen.findByText("The summary provider failed.");
  const secondJob = {
    ...first.job,
    state: "failed" as const,
    failure: "source_unavailable" as const,
  };
  const second = await activate({
    ...first.client,
    processingJobs: vi.fn(async () => ({
      items: [secondJob],
      next_cursor: null,
    })),
  });
  mounted.rerender(<VaultProcessing scope={second} sessionId="note" />);
  await screen.findByText("Saved notes or transcript could not be read.");
  expect(screen.queryByText("The summary provider failed.")).toBeNull();
  expect(first.scope.queryClient).not.toBe(second.queryClient);
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

it.each([true, false])(
  "uses the main control's default title policy only when supported (%s)",
  async (titles) => {
    const { scope, data } = await fixture({
      features: ["summary_body_jobs", ...(titles ? ["generated_titles"] : [])],
    });
    render(
      <VaultProcessing
        scope={scope}
        sessionId="note"
        defaultTitlePolicy="generate_if_empty"
      />,
    );
    const generate = screen.getByRole("button", {
      name: "Summarize saved notes",
    });
    await waitFor(() => expect(generate).toHaveProperty("disabled", false));
    if (titles)
      expect(screen.getByRole("checkbox")).toHaveProperty("checked", true);
    fireEvent.click(generate);
    await screen.findByText("Summary saved");
    expect(data.receipts[0].input.title_policy).toBe(
      titles ? "generate_if_empty" : undefined,
    );
  },
);

it("keeps empty main source disabled without inspecting provider credentials or creating a receipt", async () => {
  const { scope, calls } = await fixture();
  render(
    <VaultProcessing scope={scope} sessionId="note" sourceAvailable={false} />,
  );
  const generate = screen.getByRole("button", {
    name: "Summarize saved notes",
  });
  await waitFor(() => expect(calls.processingJobs).toHaveBeenCalled());
  expect(generate).toHaveProperty("disabled", true);
  fireEvent.click(generate);
  expect(native.deviceJobSubmissionPrepare).not.toHaveBeenCalled();
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
});
