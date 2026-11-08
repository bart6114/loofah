import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  deviceJobSubmissions: vi.fn(),
  deviceJobSubmissionPrepare: vi.fn(),
  deviceJobSubmissionGet: vi.fn(),
  deviceJobSubmissionObserve: vi.fn(),
  deviceJobSubmissionForget: vi.fn(),
  draftCacheInventory: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));

import { EnhancerService, initEnhancerService } from ".";

import { JobSubmissions } from "~/shared/application/job-submissions";
import type {
  DocumentSnapshot,
  ProcessingJobSnapshot,
  SummaryJobRequest,
  TranscriptWithData,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultScope } from "~/shared/application/vault-scope";
import type { JobSubmissionReceipt } from "~/types/tauri.gen";

const services: EnhancerService[] = [];
const scopes: VaultScope[] = [];
beforeEach(() => {
  vi.resetAllMocks();
  native.draftCacheInventory.mockResolvedValue({ status: "ok", data: [] });
});
afterEach(() => {
  for (const service of services.splice(0)) service.dispose();
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.useRealTimers();
});

async function fixture(scopeId: string = crypto.randomUUID()) {
  const receipts: JobSubmissionReceipt[] = [];
  const state = {
    document: null as DocumentSnapshot["document"],
    transcripts: [] as TranscriptWithData[],
    jobs: [] as ProcessingJobSnapshot[],
  };
  native.deviceJobSubmissions.mockImplementation(
    async (scope: string, session: string | null) => ({
      status: "ok",
      data: structuredClone(
        receipts.filter(
          (r) =>
            r.scope === scope && (!session || r.input.session_id === session),
        ),
      ),
    }),
  );
  native.deviceJobSubmissionPrepare.mockImplementation(
    async (scope: string, input: SummaryJobRequest) => {
      if (
        receipts.some(
          (r) =>
            r.scope === scope &&
            r.input.session_id === input.session_id &&
            JSON.stringify(r.input.target) === JSON.stringify(input.target) &&
            (!r.job ||
              ["queued", "running", "cancelling"].includes(r.job.state)),
        )
      )
        return {
          status: "error",
          error: {
            code: "conflict",
            message: "Recover the previous summary request",
            retryable: false,
            request_id: null,
            current_revision: null,
          },
        };
      const receipt: JobSubmissionReceipt = {
        scope,
        input,
        request_id: crypto.randomUUID(),
        created_at_ms: 1,
        job: null,
      };
      receipts.push(receipt);
      return { status: "ok", data: structuredClone(receipt) };
    },
  );
  native.deviceJobSubmissionGet.mockImplementation(
    async (_scope: string, id: string) => ({
      status: "ok",
      data: structuredClone(receipts.find((r) => r.request_id === id) ?? null),
    }),
  );
  native.deviceJobSubmissionObserve.mockImplementation(
    async (_scope: string, id: string, job: ProcessingJobSnapshot) => {
      const receipt = receipts.find((r) => r.request_id === id)!;
      receipt.job = job;
      return { status: "ok", data: structuredClone(receipt) };
    },
  );
  const calls = {
    readDocument: vi.fn(
      async (): Promise<DocumentSnapshot> => ({
        document: state.document,
        revision: "baseline",
      }),
    ),
    transcripts: vi.fn(async () => state.transcripts),
    processingJobs: vi.fn(async () => ({
      items: state.jobs,
      next_cursor: null,
    })),
    processingJob: vi.fn(
      async (id: string) => state.jobs.find((j) => j.id === id)!,
    ),
    submitSummaryJob: vi.fn(async (input: SummaryJobRequest) => {
      const job: ProcessingJobSnapshot = {
        id: crypto.randomUUID(),
        session_id: input.session_id,
        target: input.target,
        state: "queued",
        phase: null,
        failure: null,
        created_at: "now",
        updated_at: "now",
      };
      state.jobs.push(job);
      return job;
    }),
    cancelProcessingJob: vi.fn(async (id: string) => {
      const job = state.jobs.find((j) => j.id === id)!;
      job.state = "cancelled";
      return job;
    }),
  };
  const client = {
    scope: scopeId,
    canEdit: true,
    networkMode: "always",
    features: [
      "summary_body_jobs",
      "template_output_jobs",
      "generated_titles",
      "conditional_summary_jobs",
    ],
    ...calls,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scopes.push(scope);
  await scope.start();
  const service = new EnhancerService({ scope });
  services.push(service);
  return { scope, service, state, calls, receipts };
}

function eligible() {
  return [
    {
      words: Array.from({ length: 80 }, () => ({
        text: "meaningful transcript material",
      })),
    },
  ] as TranscriptWithData[];
}

it("uses the scoped shared controller with a durable UUID and title policy", async () => {
  const { service, calls, receipts } = await fixture();
  await expect(service.enhance("note")).resolves.toEqual({
    type: "started",
    noteId: "note",
  });
  expect(calls.submitSummaryJob).toHaveBeenCalledOnce();
  expect(receipts[0].input).toEqual({
    session_id: "note",
    target: { kind: "summary" },
    expected_revision: "baseline",
    title_policy: "generate_if_empty",
  });
  expect(calls.submitSummaryJob.mock.calls[0][0]).toEqual(receipts[0].input);
});

it("coalesces simultaneous callers and shares unresolved receipt exclusion with manual UI submissions", async () => {
  const { service, calls, scope } = await fixture();
  await Promise.all([service.enhance("note"), service.enhance("note")]);
  expect(calls.submitSummaryJob).toHaveBeenCalledOnce();
  await expect(
    new JobSubmissions(scope).generateSummary("note"),
  ).rejects.toThrow("Recover the previous");
  expect(calls.submitSummaryJob).toHaveBeenCalledOnce();
});

it("skips nonempty canonical summaries without native request persistence", async () => {
  const { service, state, calls } = await fixture();
  state.document = {
    id: "note",
    session_id: "note",
    kind: "summary",
    title: "Summary",
    markdown: "Already saved",
    template_id: "",
    sort_order: 0,
  };
  await expect(service.queueAutoEnhanceIfSummaryEmpty("note")).resolves.toEqual(
    { type: "summary_exists", noteId: "note" },
  );
  await expect(service.enhance("note")).resolves.toEqual({
    type: "already_active",
    noteId: "note",
  });
  expect(native.deviceJobSubmissionPrepare).not.toHaveBeenCalled();
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
});

it("passes automatic only-if-empty intent into the same durable request", async () => {
  const { service, state, calls, receipts } = await fixture();
  state.transcripts = eligible();
  await service.queueAutoEnhanceIfSummaryEmpty("note");
  await vi.waitFor(() => expect(calls.submitSummaryJob).toHaveBeenCalledOnce());
  expect(receipts[0].input).toMatchObject({
    only_if_empty: true,
    target: { kind: "summary" },
  });
});

it("retains bounded transcript eligibility skips without starting a model", async () => {
  vi.useFakeTimers();
  const { service, calls } = await fixture();
  const event = vi.fn();
  service.on(event);
  service.queueAutoEnhance("note");
  await vi.advanceTimersByTimeAsync(10_500);
  expect(event).toHaveBeenCalledWith(
    expect.objectContaining({
      type: "auto-enhance-skipped",
      reasonCode: "no_transcript",
    }),
  );
  expect(calls.submitSummaryJob).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it("requires an existing correctly bound output before dispatching regeneration", async () => {
  const { service, state, calls } = await fixture();
  await expect(
    service.enhance("note", { targetNoteId: "output" }),
  ).rejects.toThrow("no longer available");
  state.document = {
    id: "output",
    session_id: "note",
    kind: "template_output",
    title: "Output",
    markdown: "Before",
    template_id: "template",
    sort_order: 3,
  };
  await service.enhance("note", { targetNoteId: "output" });
  expect(calls.submitSummaryJob.mock.calls[0][0].target).toEqual({
    kind: "template_output",
    document_id: "output",
  });
});

it("disposal fences delayed old reads even when a new service has the same scope string", async () => {
  const first = await fixture("same");
  let finish!: (value: DocumentSnapshot) => void;
  first.calls.readDocument.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const pending = first.service.enhance("note");
  const rejection = expect(pending).rejects.toThrow("no longer available");
  const replacement = initEnhancerService({ scope: first.scope });
  services.push(replacement);
  first.service.dispose();
  finish({ document: null, revision: "baseline" });
  await rejection;
  expect(first.calls.submitSummaryJob).not.toHaveBeenCalled();
  expect(replacement.ownerId).not.toBe(first.service.ownerId);
});

it("reset cancels acknowledged jobs but refuses to replay unknown receipts", async () => {
  const { service, calls, receipts } = await fixture();
  await service.enhance("note");
  await service.resetEnhanceTasks("note");
  expect(calls.cancelProcessingJob).toHaveBeenCalledOnce();
  receipts[0].job = null;
  await expect(service.resetEnhanceTasks("note")).rejects.toThrow(
    "Recover the previous",
  );
  expect(calls.submitSummaryJob).toHaveBeenCalledOnce();
  expect(calls.processingJob).not.toHaveBeenCalled();
});
