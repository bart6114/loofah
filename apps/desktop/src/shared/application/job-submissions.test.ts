import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("~/types/tauri.gen", () => ({ commands: {} }));

import { NativeDraftStore } from "./draft-store";
import { ApplicationRequestError } from "./errors";
import { JobSubmissions } from "./job-submissions";
import { noteDrafts } from "./note-draft";
import type {
  DocumentSnapshot,
  DocumentTarget,
  ProcessingJobSnapshot,
  SummaryJobRequest,
} from "./protocol.gen";
import type { RequestOptions } from "./vault-client";
import type { VaultScope } from "./vault-scope";

import type { JobSubmissionReceipt } from "~/types/tauri.gen";

const inventory = vi.spyOn(NativeDraftStore.prototype, "inventory");
beforeEach(() => inventory.mockResolvedValue([]));

function fixture(scopeId = "scope") {
  let state = "connected";
  const watches = new Set<() => void>();
  const disk: JobSubmissionReceipt[] = [];
  const order: string[] = [];
  const job: ProcessingJobSnapshot = {
    id: crypto.randomUUID(),
    session_id: "note",
    target: { kind: "summary" },
    state: "queued",
    phase: null,
    failure: null,
    created_at: "time",
    updated_at: "time",
  };
  const store = {
    load: vi.fn(async (id: string) =>
      structuredClone(
        disk.find((receipt) => receipt.request_id === id) ?? null,
      ),
    ),
    list: vi.fn(async (sessionId?: string) =>
      structuredClone(
        disk.filter(
          (receipt) => !sessionId || receipt.input.session_id === sessionId,
        ),
      ),
    ),
    prepare: vi.fn(async (input: SummaryJobRequest) => {
      order.push("persist");
      const value: JobSubmissionReceipt = {
        scope: scopeId,
        request_id: crypto.randomUUID(),
        input,
        created_at_ms: 1,
        job: null,
      };
      disk.push(value);
      return value;
    }),
    observe: vi.fn(async (requestId: string, job: ProcessingJobSnapshot) => {
      order.push("acknowledge");
      const receipt = disk.find((receipt) => receipt.request_id === requestId)!;
      receipt.job = job;
      return receipt;
    }),
    forget: vi.fn(),
  };
  const client = {
    scope: scopeId,
    canEdit: true,
    networkMode: "always",
    features: ["summary_body_jobs"],
    readDocument: vi.fn(async (): Promise<DocumentSnapshot> => {
      order.push("read");
      return { document: null, revision: "baseline" };
    }),
    submitSummaryJob: vi.fn(
      async (
        _input: SummaryJobRequest,
        _options: RequestOptions & { requestId: string },
      ) => {
        order.push("dispatch");
        return job;
      },
    ),
    processingJob: vi.fn(
      async (_jobId: string, _options?: RequestOptions) => job,
    ),
    cancelProcessingJob: vi.fn(async () => ({
      ...job,
      state: "cancelled" as const,
    })),
  };
  const scope = {
    client,
    getSnapshot: () => ({ state, error: null }),
    subscribe: (changed: () => void) => {
      watches.add(changed);
      return () => watches.delete(changed);
    },
    run: vi.fn(async (operation: () => Promise<unknown>) => operation()),
  } as unknown as VaultScope;
  return {
    controller: new JobSubmissions(scope, store),
    scope,
    store,
    disk,
    client,
    job,
    order,
    state(next: string) {
      state = next;
      for (const changed of watches) changed();
    },
  };
}

describe("durable device summary submissions", () => {
  it("does not dispatch until the original UUID and baseline have been saved", async () => {
    const { controller, store, client, order } = fixture();
    let persist!: () => void;
    const prepare = store.prepare.getMockImplementation()!;
    store.prepare.mockImplementationOnce(async (input) => {
      await new Promise<void>((resolve) => {
        persist = resolve;
      });
      return prepare(input);
    });
    const pending = controller.generateSummary("note");
    await vi.waitFor(() => expect(store.prepare).toHaveBeenCalledOnce());
    expect(client.submitSummaryJob).not.toHaveBeenCalled();
    persist();
    const receipt = await pending;
    expect(order).toEqual(["read", "persist", "dispatch", "acknowledge"]);
    expect(client.submitSummaryJob).toHaveBeenCalledWith(
      {
        session_id: "note",
        target: { kind: "summary" },
        expected_revision: "baseline",
      },
      { requestId: receipt.request_id },
    );
  });

  it("retains a lost reply across controller restart and explicitly replays the original request", async () => {
    const { controller, scope, store, client, disk, job } = fixture();
    client.submitSummaryJob.mockRejectedValueOnce(
      new ApplicationRequestError({
        code: "unavailable",
        message: "Lost reply",
        request_id: null,
        retryable: false,
        mutation_outcome: "unknown",
        current_revision: null,
      }),
    );
    await expect(controller.generateSummary("note")).rejects.toThrow(
      "Lost reply",
    );
    expect(disk).toHaveLength(1);
    expect(disk[0].job).toBeNull();
    const original = structuredClone(disk[0]);
    const reopened = new JobSubmissions(scope, store);
    await reopened.store.list();
    expect(client.submitSummaryJob).toHaveBeenCalledTimes(1);
    client.readDocument.mockResolvedValue({
      document: null,
      revision: "new-canonical-body",
    });
    const recovered = await reopened.resume(original.request_id);
    expect(recovered.job).toEqual(job);
    expect(client.readDocument).toHaveBeenCalledTimes(1);
    expect(client.submitSummaryJob.mock.calls[1]).toEqual([
      original.input,
      { requestId: original.request_id },
    ]);
  });

  it("never sends when device persistence fails", async () => {
    const { controller, store, client } = fixture();
    store.prepare.mockRejectedValueOnce(new Error("Device disk is full"));
    await expect(controller.generateSummary("note")).rejects.toThrow(
      "disk is full",
    );
    expect(client.submitSummaryJob).not.toHaveBeenCalled();
    expect(store.observe).not.toHaveBeenCalled();
  });

  it("does not dispatch when the pre-dispatch durability confirmation fails", async () => {
    const { controller, store, client, disk } = fixture();
    store.load.mockRejectedValueOnce(
      new Error("Cannot confirm device durability"),
    );
    await expect(controller.generateSummary("note")).rejects.toThrow(
      "durability",
    );
    expect(disk).toHaveLength(1);
    expect(client.submitSummaryJob).not.toHaveBeenCalled();
    await controller.resume(disk[0].request_id);
    expect(client.submitSummaryJob).toHaveBeenCalledOnce();
  });

  it("reuses the receipt when acknowledgement storage fails after the server accepts the job", async () => {
    const { controller, store, client, disk } = fixture();
    store.observe.mockRejectedValueOnce(
      new Error("Cannot save acknowledgement"),
    );
    await expect(controller.generateSummary("note")).rejects.toThrow(
      "acknowledgement",
    );
    const id = disk[0].request_id;
    expect(disk[0].job).toBeNull();
    await controller.resume(id);
    expect(client.submitSummaryJob.mock.calls.map((call) => call[1])).toEqual([
      { requestId: id },
      { requestId: id },
    ]);
  });

  it("uses a read to refresh an acknowledged job and never resubmits it", async () => {
    const { controller, store, client, job } = fixture();
    const receipt = await controller.generateSummary("note");
    client.processingJob.mockResolvedValue({ ...job, state: "succeeded" });
    await controller.resume(receipt.request_id);
    expect(client.submitSummaryJob).toHaveBeenCalledTimes(1);
    expect(client.processingJob).toHaveBeenCalledWith(job.id, undefined);
    expect(store.observe).toHaveBeenLastCalledWith(receipt.request_id, {
      ...job,
      state: "succeeded",
    });
  });

  it("keeps an unsent persisted receipt when the caller aborts before dispatch", async () => {
    const { controller, store, client, disk } = fixture();
    const abort = new AbortController();
    const prepare = store.prepare.getMockImplementation()!;
    store.prepare.mockImplementationOnce(async (input) => {
      const result = await prepare(input);
      abort.abort();
      return result;
    });
    await expect(
      controller.generateSummary("note", { signal: abort.signal }),
    ).rejects.toThrow();
    expect(disk).toHaveLength(1);
    expect(client.submitSummaryJob).not.toHaveBeenCalled();
    await controller.resume(disk[0].request_id);
    expect(client.submitSummaryJob).toHaveBeenCalledOnce();
  });

  it("rejects a foreign scope or unsupported connection before dispatch", async () => {
    const { controller, client, disk, store } = fixture();
    await controller.generateSummary("note");
    disk[0].scope = "different-account";
    await expect(controller.resume(disk[0].request_id)).rejects.toThrow(
      "account and vault",
    );
    expect(client.submitSummaryJob).toHaveBeenCalledOnce();
    client.features = [];
    store.list.mockClear();
    await expect(controller.generateSummary("note")).rejects.toMatchObject({
      detail: {
        code: "incompatible_version",
        mutation_outcome: "not_committed",
      },
    });
    expect(store.list).not.toHaveBeenCalled();
  });

  it.each([
    { features: [] },
    { features: ["summary_body_jobs"] },
    { features: ["template_output_jobs"] },
    {
      features: ["summary_body_jobs", "template_library", "template_creation"],
    },
  ])(
    "requires both base and existing-output capabilities with features $features",
    async ({ features }) => {
      const { controller, client, store } = fixture();
      client.features = features;
      await expect(
        controller.generateDocument("note", {
          kind: "template_output",
          document_id: "output",
        }),
      ).rejects.toMatchObject({ detail: { code: "incompatible_version" } });
      expect(store.list).not.toHaveBeenCalled();
      expect(store.prepare).not.toHaveBeenCalled();
      expect(client.readDocument).not.toHaveBeenCalled();
    },
  );

  it.each([
    null,
    { id: "output", session_id: "note", kind: "summary" },
    { id: "different", session_id: "note", kind: "template_output" },
    { id: "output", session_id: "different", kind: "template_output" },
  ])(
    "refuses missing, legacy and mismatched canonical outputs %j",
    async (document) => {
      const { controller, client, store } = fixture();
      client.features.push("template_output_jobs");
      client.readDocument.mockResolvedValue({
        document: document && {
          ...document,
          title: "Keep",
          template_id: "template",
          sort_order: 7,
          markdown: "Body",
        },
        revision: "d0",
      });
      await expect(
        controller.generateDocument("note", {
          kind: "template_output",
          document_id: "output",
        }),
      ).rejects.toThrow("no longer available");
      expect(store.prepare).not.toHaveBeenCalled();
      expect(client.submitSummaryJob).not.toHaveBeenCalled();
    },
  );

  it("captures the existing target and resumes its exact lost-reply request without rebuilding its revision or policy", async () => {
    const { controller, client, store, scope, disk } = fixture();
    client.features.push("template_output_jobs");
    const target: DocumentTarget = {
      kind: "template_output",
      document_id: "output",
    };
    client.readDocument.mockResolvedValue({
      document: {
        id: "output",
        session_id: "note",
        kind: "template_output",
        title: "Keep title",
        template_id: "keep-template",
        sort_order: 7,
        markdown: "Saved output",
      },
      revision: "d0",
    });
    const list = store.list.getMockImplementation()!;
    store.list.mockImplementationOnce(async (id) => {
      target.document_id = "changed-selection";
      return list(id);
    });
    client.submitSummaryJob.mockRejectedValueOnce(new Error("Lost reply"));
    await expect(controller.generateDocument("note", target)).rejects.toThrow(
      "Lost reply",
    );
    const original = structuredClone(disk[0]);
    expect(original.input).toEqual({
      session_id: "note",
      target: { kind: "template_output", document_id: "output" },
      expected_revision: "d0",
    });
    const reopened = new JobSubmissions(scope, store);
    await reopened.store.list();
    expect(client.submitSummaryJob).toHaveBeenCalledOnce();
    await reopened.resume(original.request_id);
    expect(client.readDocument).toHaveBeenCalledOnce();
    expect(client.submitSummaryJob.mock.calls[1]).toEqual([
      original.input,
      { requestId: original.request_id },
    ]);
    expect(store.prepare).toHaveBeenCalledOnce();
  });

  it.each(["baseline", "receipt"])(
    "refuses newer typing during the %s await and retains any unsent receipt",
    async (stage) => {
      const { controller, client, store, disk } = fixture(`typing-${stage}`);
      const draft = noteDrafts.open(
        client as unknown as import("./vault-client").VaultClient,
        "note",
        { markdown: "Saved", title: "", revision: "n0" },
      );
      if (stage === "baseline")
        client.readDocument.mockImplementationOnce(async () => {
          draft.edit("New private input", "");
          return { document: null, revision: "d0" };
        });
      else {
        const prepare = store.prepare.getMockImplementation()!;
        store.prepare.mockImplementationOnce(async (input) => {
          const receipt = await prepare(input);
          draft.edit("New private input", "");
          return receipt;
        });
      }
      await expect(controller.generateSummary("note")).rejects.toThrow(
        "Confirm or recover",
      );
      expect(client.submitSummaryJob).not.toHaveBeenCalled();
      expect(draft.getSnapshot().draft.markdown).toBe("New private input");
      expect(disk).toHaveLength(stage === "receipt" ? 1 : 0);
      await controller.store.list();
      expect(client.submitSummaryJob).not.toHaveBeenCalled();
    },
  );

  it("latches lost permission while preserving an unsent receipt after durability confirmation", async () => {
    const context = fixture("lost-permission");
    const prepare = context.store.prepare.getMockImplementation()!;
    context.store.prepare.mockImplementationOnce(async (input) => {
      const receipt = await prepare(input);
      context.state("access_denied");
      context.state("connected");
      return receipt;
    });
    await expect(context.controller.generateSummary("note")).rejects.toThrow(
      "Confirm or recover",
    );
    expect(context.disk).toHaveLength(1);
    expect(context.client.submitSummaryJob).not.toHaveBeenCalled();
  });
});
