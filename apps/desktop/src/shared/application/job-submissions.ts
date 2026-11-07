import { applicationResult, ApplicationRequestError } from "./errors";
import type { SummaryJobRequest, ProcessingJobSnapshot } from "./protocol.gen";
import type { RequestOptions } from "./vault-client";
import type { VaultScope } from "./vault-scope";

import { commands, type JobSubmissionReceipt } from "~/types/tauri.gen";

export const activeJob = (job: ProcessingJobSnapshot) =>
  ["queued", "running", "cancelling"].includes(job.state);

export class NativeJobSubmissions {
  constructor(readonly scope: string) {}

  async list(sessionId?: string): Promise<JobSubmissionReceipt[]> {
    const values = applicationResult(
      await commands.deviceJobSubmissions(this.scope, sessionId ?? null),
    );
    for (const value of values) this.checkScope(value);
    return values;
  }

  async prepare(input: SummaryJobRequest): Promise<JobSubmissionReceipt> {
    const value = applicationResult(
      await commands.deviceJobSubmissionPrepare(this.scope, input),
    );
    this.checkScope(value);
    return value;
  }

  async load(requestId: string): Promise<JobSubmissionReceipt | null> {
    const value = applicationResult(
      await commands.deviceJobSubmissionGet(this.scope, requestId),
    );
    if (value) this.checkScope(value);
    return value;
  }

  async observe(
    requestId: string,
    snapshot: ProcessingJobSnapshot,
  ): Promise<JobSubmissionReceipt> {
    const value = applicationResult(
      await commands.deviceJobSubmissionObserve(
        this.scope,
        requestId,
        snapshot,
      ),
    );
    this.checkScope(value);
    return value;
  }

  async forget(requestId: string): Promise<void> {
    applicationResult(
      await commands.deviceJobSubmissionForget(this.scope, requestId),
    );
  }

  private checkScope(value: JobSubmissionReceipt) {
    if (value.scope !== this.scope)
      throw new Error("This saved request belongs to another account or vault");
  }
}

export class JobSubmissions {
  constructor(
    readonly scope: VaultScope,
    readonly store: Pick<
      NativeJobSubmissions,
      "list" | "load" | "prepare" | "observe" | "forget"
    > = new NativeJobSubmissions(scope.client.scope),
  ) {}

  async generateSummary(sessionId: string, options?: RequestOptions) {
    this.requireFeature();
    // Refresh only acknowledged active receipts. Unknown submissions are resumed
    // explicitly; loading device recovery never dispatches a generation.
    for (const receipt of await this.store.list(sessionId)) {
      if (receipt.job && activeJob(receipt.job))
        await this.resume(receipt.request_id, options);
    }
    const baseline = await this.scope.run(() =>
      this.scope.client.readDocument(sessionId, { kind: "summary" }, options),
    );
    options?.signal?.throwIfAborted();
    const receipt = await this.store.prepare({
      session_id: sessionId,
      target: { kind: "summary" },
      expected_revision: baseline.revision,
    });
    options?.signal?.throwIfAborted();
    return this.resume(receipt.request_id, options);
  }

  async resume(requestId: string, options?: RequestOptions) {
    this.requireFeature();
    const receipt = await this.store.load(requestId);
    if (!receipt || receipt.scope !== this.scope.client.scope)
      throw new Error(
        "Saved summary request not found for this account and vault",
      );
    options?.signal?.throwIfAborted();
    const job = await this.scope.run(() =>
      receipt.job
        ? this.scope.client.processingJob(receipt.job.id, options)
        : this.scope.client.submitSummaryJob(receipt.input, {
            ...options,
            requestId: receipt.request_id,
          }),
    );
    return this.store.observe(requestId, job);
  }

  async cancel(jobId: string, options?: RequestOptions) {
    this.requireFeature();
    const job = await this.scope.run(() =>
      this.scope.client.cancelProcessingJob(jobId, options),
    );
    const receipts = await this.store.list();
    for (const receipt of receipts) {
      if (receipt.job?.id === job.id)
        await this.store.observe(receipt.request_id, job);
    }
    return job;
  }

  private requireFeature() {
    if (!this.scope.client.features.includes("summary_body_jobs"))
      throw new ApplicationRequestError({
        code: "incompatible_version",
        message: "This connection does not support summary generation",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      });
  }
}
