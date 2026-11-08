import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  deviceJobSubmissions: vi.fn(),
  deviceJobSubmissionPrepare: vi.fn(),
  deviceJobSubmissionGet: vi.fn(),
  deviceJobSubmissionObserve: vi.fn(),
  deviceJobSubmissionForget: vi.fn(),
  draftCacheInventory: vi.fn(),
  toast: vi.fn(),
  local: vi.fn(() => {
    throw new Error("Local AI access is forbidden");
  }),
  target: { kind: "summary" } as object | null,
}));
vi.mock("~/types/tauri.gen", () => ({ commands: native }));
vi.mock("~/ai/hooks", () => ({
  useAITaskTask: native.local,
  useLanguageModel: native.local,
}));
vi.mock("~/session/content-queries", () => ({
  loadSessionContentSnapshot: native.local,
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: native.toast },
}));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("~/session/queries", () => ({
  useEnhancedDocumentTarget: () => native.target,
}));

import { useEnhancedNoteActions } from "./enhanced-actions";

import type {
  DocumentSnapshot,
  ProcessingJobSnapshot,
  SummaryJobRequest,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";
import type { JobSubmissionReceipt } from "~/types/tauri.gen";

const scopes: VaultScope[] = [];
let receipts: JobSubmissionReceipt[];
beforeEach(() => {
  vi.resetAllMocks();
  receipts = [];
  native.target = { kind: "summary" };
  native.draftCacheInventory.mockResolvedValue({ status: "ok", data: [] });
  native.deviceJobSubmissions.mockImplementation(
    async (scope: string, session: string | null) => ({
      status: "ok",
      data: receipts.filter(
        (r) =>
          r.scope === scope && (!session || r.input.session_id === session),
      ),
    }),
  );
  native.deviceJobSubmissionPrepare.mockImplementation(
    async (scope: string, input: SummaryJobRequest) => {
      const receipt: JobSubmissionReceipt = {
        scope,
        input: structuredClone(input),
        request_id: crypto.randomUUID(),
        created_at_ms: Date.now(),
        job: null,
      };
      receipts.push(receipt);
      return { status: "ok", data: receipt };
    },
  );
  native.deviceJobSubmissionGet.mockImplementation(
    async (scope: string, id: string) => ({
      status: "ok",
      data:
        receipts.find((r) => r.scope === scope && r.request_id === id) ?? null,
    }),
  );
  native.deviceJobSubmissionObserve.mockImplementation(
    async (scope: string, id: string, job: ProcessingJobSnapshot) => {
      const receipt = receipts.find(
        (r) => r.scope === scope && r.request_id === id,
      )!;
      receipt.job = job;
      return { status: "ok", data: receipt };
    },
  );
});
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});

async function fixture(
  features = ["summary_body_jobs", "generated_titles"],
  canEdit = true,
  name: string = crypto.randomUUID(),
) {
  let listener!: (event: VaultClientEvent) => void;
  let jobs: ProcessingJobSnapshot[] = [];
  const calls = {
    readDocument: vi.fn(
      async (): Promise<DocumentSnapshot> => ({
        document: null,
        revision: "saved-revision",
      }),
    ),
    submitSummaryJob: vi.fn(async (input: SummaryJobRequest) => {
      const job: ProcessingJobSnapshot = {
        id: crypto.randomUUID(),
        session_id: "note",
        target: input.target,
        state: "queued",
        phase: null,
        failure: null,
        created_at: "2026-11-08T10:00:00Z",
        updated_at: "2026-11-08T10:00:00Z",
      };
      jobs = [job];
      return job;
    }),
    processingJobs: vi.fn(async () => ({ items: jobs, next_cursor: null })),
    processingJob: vi.fn(
      async (id: string) => jobs.find((job) => job.id === id)!,
    ),
    cancelProcessingJob: vi.fn(async (id: string) => {
      const job = jobs.find((job) => job.id === id)!;
      jobs = [{ ...job, state: "cancelled" }];
      return jobs[0];
    }),
  };
  const client = {
    scope: name,
    networkMode: "always",
    features,
    canEdit,
    ...calls,
    subscribe: async (receive: (event: VaultClientEvent) => void) => {
      listener = receive;
      receive({ kind: "connection", state: "connected", error: null });
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
  return { scope, calls, emit: (event: VaultClientEvent) => listener(event) };
}
function mount(scope: VaultScope, noteId = "note") {
  let host = scope;
  const wrapper = ({ children }: { children: ReactNode }) => (
    <VaultHostContext.Provider
      value={{ scope: host, error: null, retry: () => {} }}
    >
      {children}
    </VaultHostContext.Provider>
  );
  const hook = renderHook(
    () => useEnhancedNoteActions({ sessionId: "note", enhancedNoteId: noteId }),
    { wrapper },
  );
  return {
    ...hook,
    replace: (scope: VaultScope) => {
      host = scope;
      hook.rerender();
    },
  };
}

it.each([{ extra: [] }, { extra: ["legacy_desktop_enhance"] }])(
  "submits shared immutable jobs with identical Local/Remote capability behavior %j",
  async ({ extra }) => {
    const context = await fixture([
      "summary_body_jobs",
      "generated_titles",
      ...extra,
    ]);
    const hook = mount(context.scope);
    await waitFor(() => expect(hook.result.current.canRegenerate).toBe(true));
    await act(() => hook.result.current.onRegenerate());
    expect(context.calls.submitSummaryJob).toHaveBeenCalledWith(
      {
        session_id: "note",
        target: { kind: "summary" },
        expected_revision: "saved-revision",
        title_policy: "generate_if_empty",
      },
      { requestId: receipts[0].request_id, signal: expect.any(AbortSignal) },
    );
    expect(native.local).not.toHaveBeenCalled();
    await waitFor(() => expect(hook.result.current.isGenerating).toBe(true));
    expect(hook.result.current.canRegenerate).toBe(false);
    await act(() => hook.result.current.onCancel());
    expect(context.calls.cancelProcessingJob).toHaveBeenCalledWith(
      receipts[0].job!.id,
      { signal: expect.any(AbortSignal) },
    );
  },
);
it.each([
  { features: [], canEdit: true },
  { features: ["legacy_desktop_enhance"], canEdit: true },
  { features: ["summary_body_jobs"], canEdit: false },
])(
  "refuses unsupported or read-only generation %j",
  async ({ features, canEdit }) => {
    const context = await fixture(features, canEdit);
    const hook = mount(context.scope);
    await act(() => hook.result.current.onRegenerate());
    expect(context.calls.readDocument).not.toHaveBeenCalled();
    expect(context.calls.submitSummaryJob).not.toHaveBeenCalled();
    expect(native.local).not.toHaveBeenCalled();
  },
);
it("keeps older server requests compatible by preserving titles without that capability", async () => {
  const context = await fixture(["summary_body_jobs"]);
  const hook = mount(context.scope);
  await waitFor(() => expect(hook.result.current.canRegenerate).toBe(true));
  await act(() => hook.result.current.onRegenerate());
  expect(receipts[0].input).not.toHaveProperty("title_policy");
});
it("validates the selected template target through the shared baseline read", async () => {
  native.target = { kind: "template_output", document_id: "output" };
  const context = await fixture(["summary_body_jobs", "template_output_jobs"]);
  context.calls.readDocument.mockResolvedValue({
    document: {
      id: "output",
      session_id: "note",
      kind: "template_output",
      title: "Template",
      template_id: "source-template",
      sort_order: 2,
      markdown: "Old result",
    },
    revision: "output-revision",
  });
  const hook = mount(context.scope, "output");
  await waitFor(() => expect(hook.result.current.canRegenerate).toBe(true));
  await act(() => hook.result.current.onRegenerate());
  expect(receipts[0].input).toEqual({
    session_id: "note",
    target: native.target,
    expected_revision: "output-revision",
  });
});
it("preserves unknown outcomes for explicit UUID recovery instead of replaying generation", async () => {
  const context = await fixture();
  context.calls.submitSummaryJob.mockRejectedValueOnce(new Error("Reply lost"));
  const hook = mount(context.scope);
  await waitFor(() => expect(hook.result.current.canRegenerate).toBe(true));
  await act(() => hook.result.current.onRegenerate());
  expect(native.toast).toHaveBeenCalledWith("Reply lost");
  await waitFor(() => expect(hook.result.current.canRegenerate).toBe(false));
  await act(() => hook.result.current.onRegenerate());
  expect(context.calls.submitSummaryJob).toHaveBeenCalledOnce();
  expect(receipts[0].job).toBeNull();
});
it("aborts a delayed baseline read when the same-string host is replaced", async () => {
  const first = await fixture(undefined, true, "same-owner-string");
  const second = await fixture(undefined, true, "same-owner-string");
  let release!: (snapshot: DocumentSnapshot) => void;
  first.calls.readDocument.mockReturnValueOnce(
    new Promise((done) => {
      release = done;
    }),
  );
  const hook = mount(first.scope);
  await waitFor(() => expect(hook.result.current.canRegenerate).toBe(true));
  let pending!: Promise<void>;
  act(() => {
    pending = hook.result.current.onRegenerate();
  });
  await waitFor(() => expect(first.calls.readDocument).toHaveBeenCalledOnce());
  hook.replace(second.scope);
  await act(async () => {
    release({ document: null, revision: "stale" });
    await pending;
  });
  expect(first.calls.submitSummaryJob).not.toHaveBeenCalled();
  expect(second.calls.submitSummaryJob).not.toHaveBeenCalled();
  expect(native.deviceJobSubmissionPrepare).not.toHaveBeenCalled();
  expect(native.toast).not.toHaveBeenCalled();
});
it("stops generation after disconnect without consulting a Local fallback", async () => {
  const context = await fixture();
  const hook = mount(context.scope);
  await waitFor(() => expect(hook.result.current.canRegenerate).toBe(true));
  act(() =>
    context.emit({ kind: "connection", state: "reconnecting", error: null }),
  );
  await act(() => hook.result.current.onRegenerate());
  expect(hook.result.current.canRegenerate).toBe(false);
  expect(context.calls.submitSummaryJob).not.toHaveBeenCalled();
  expect(native.local).not.toHaveBeenCalled();
});
