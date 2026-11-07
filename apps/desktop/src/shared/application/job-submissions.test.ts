import { describe, expect, it, vi } from "vitest";

vi.mock("~/types/tauri.gen", () => ({ commands: {} }));

import { ApplicationRequestError } from "./errors";
import { JobSubmissions } from "./job-submissions";
import type { ProcessingJobSnapshot, SummaryJobRequest } from "./protocol.gen";
import type { RequestOptions } from "./vault-client";
import type { VaultScope } from "./vault-scope";

import type { JobSubmissionReceipt } from "~/types/tauri.gen";

function fixture() {
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
        scope: "scope",
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
    scope: "scope",
    features: ["summary_body_jobs"],
    readDocument: vi.fn(async () => {
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
});
