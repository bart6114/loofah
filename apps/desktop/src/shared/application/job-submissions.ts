import { applicationResult, ApplicationRequestError } from "./errors";
import type {
  DocumentTarget,
  SummaryJobRequest,
  ProcessingJobSnapshot,
  TitlePolicy,
} from "./protocol.gen";
import { captureSessionGenerationGuard } from "./session-generation-guard";
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
      await commands.deviceJobSubmissionPrepare(this.scope, input, true),
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

  async generateSummary(
    sessionId: string,
    options?: RequestOptions & {
      titlePolicy?: TitlePolicy;
      onlyIfEmpty?: boolean;
    },
  ) {
    return this.generateDocument(sessionId, { kind: "summary" }, options);
  }

  async generateDocument(
    sessionId: string,
    input: DocumentTarget,
    options?: RequestOptions & {
      titlePolicy?: TitlePolicy;
      onlyIfEmpty?: boolean;
    },
  ) {
    const {
      titlePolicy = "preserve",
      onlyIfEmpty = false,
      ...requestOptions
    } = options ?? {};
    const transportOptions = Object.keys(requestOptions).length
      ? requestOptions
      : undefined;
    const target: DocumentTarget =
      input.kind === "summary"
        ? { kind: "summary" }
        : { kind: "template_output", document_id: input.document_id };
    this.requireFeature(target, titlePolicy, onlyIfEmpty);
    const guard = captureSessionGenerationGuard(
      this.scope,
      sessionId,
      target,
      transportOptions?.signal,
    );
    try {
      await guard.prepare();
      // Refresh only acknowledged active receipts. Unknown submissions are resumed
      // explicitly; loading device recovery never dispatches a generation.
      const receipts = await this.store.list(sessionId);
      guard.assertReady();
      if (
        receipts.some(
          (receipt) =>
            !receipt.job &&
            receipt.input.session_id === sessionId &&
            receipt.input.target.kind === target.kind &&
            (target.kind === "summary" ||
              (receipt.input.target.kind === "template_output" &&
                receipt.input.target.document_id === target.document_id)),
        )
      )
        throw new ApplicationRequestError({
          code: "conflict",
          message:
            "Recover the previous summary request before starting another",
          retryable: false,
          request_id: null,
          current_revision: null,
          mutation_outcome: "not_committed",
        });
      for (const receipt of receipts) {
        if (receipt.job && activeJob(receipt.job))
          await this.resume(receipt.request_id, transportOptions);
        guard.assertReady();
      }
      const baseline = await this.scope.run(() =>
        this.scope.client.readDocument(sessionId, target, transportOptions),
      );
      guard.assertReady();
      if (
        target.kind === "template_output" &&
        (!baseline.document ||
          baseline.document.kind !== "template_output" ||
          baseline.document.session_id !== sessionId ||
          baseline.document.id !== target.document_id)
      )
        throw new Error(
          "This template output is no longer available for regeneration",
        );
      transportOptions?.signal?.throwIfAborted();
      const receipt = await this.store.prepare({
        session_id: sessionId,
        target,
        expected_revision: baseline.revision,
        ...(onlyIfEmpty ? { only_if_empty: true } : {}),
        ...(titlePolicy === "generate_if_empty"
          ? { title_policy: titlePolicy }
          : {}),
      });
      guard.assertReady();
      transportOptions?.signal?.throwIfAborted();
      return await this.dispatch(receipt.request_id, transportOptions, guard);
    } finally {
      guard.dispose();
    }
  }

  async resume(requestId: string, options?: RequestOptions) {
    return this.dispatch(requestId, options);
  }

  private async dispatch(
    requestId: string,
    options?: RequestOptions,
    prepared?: ReturnType<typeof captureSessionGenerationGuard>,
  ) {
    this.requireFeature();
    const receipt = await this.store.load(requestId);
    if (!receipt || receipt.scope !== this.scope.client.scope)
      throw new Error(
        "Saved summary request not found for this account and vault",
      );
    prepared?.assertReady();
    if (!receipt.job)
      this.requireFeature(
        receipt.input.target,
        receipt.input.title_policy,
        receipt.input.only_if_empty,
      );
    options?.signal?.throwIfAborted();
    const guard = receipt.job
      ? null
      : (prepared ??
        captureSessionGenerationGuard(
          this.scope,
          receipt.input.session_id,
          receipt.input.target,
          options?.signal,
        ));
    try {
      if (guard && !prepared) await guard.prepare();
      guard?.assertReady();
      const job = await this.scope.run(() =>
        receipt.job
          ? this.scope.client.processingJob(receipt.job.id, options)
          : this.scope.client.submitSummaryJob(receipt.input, {
              ...options,
              requestId: receipt.request_id,
            }),
      );
      return this.store.observe(requestId, job);
    } finally {
      if (!prepared) guard?.dispose();
    }
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

  private requireFeature(
    target?: DocumentTarget,
    titlePolicy?: TitlePolicy,
    onlyIfEmpty = false,
  ) {
    if (
      !this.scope.client.features.includes("summary_body_jobs") ||
      (target?.kind === "template_output" &&
        !this.scope.client.features.includes("template_output_jobs"))
    )
      throw new ApplicationRequestError({
        code: "incompatible_version",
        message: "This connection does not support summary generation",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      });
    if (
      onlyIfEmpty &&
      !this.scope.client.features.includes("conditional_summary_jobs")
    )
      throw new ApplicationRequestError({
        code: "incompatible_version",
        message:
          "This connection does not support conditional summary generation",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      });
    if (
      titlePolicy === "generate_if_empty" &&
      !this.scope.client.features.includes("generated_titles")
    )
      throw new ApplicationRequestError({
        code: "incompatible_version",
        message: "This connection does not support generated titles",
        retryable: false,
        request_id: null,
        current_revision: null,
        mutation_outcome: "not_committed",
      });
  }
}
