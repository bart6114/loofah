import { applicationResult } from "./errors";
import type {
  TranscriptionJobRequest,
  TranscriptionJobSnapshot,
} from "./protocol.gen";
import type { RequestOptions } from "./vault-client";
import type { VaultScope } from "./vault-scope";

import {
  commands,
  type TranscriptionSubmissionReceipt,
} from "~/types/tauri.gen";

export const activeTranscription = (job: TranscriptionJobSnapshot) =>
  ["queued", "running", "cancelling"].includes(job.state);

export class NativeTranscriptionSubmissions {
  constructor(readonly scope: string) {}

  async list(sessionId?: string) {
    const values = applicationResult(
      await commands.deviceTranscriptionSubmissions(
        this.scope,
        sessionId ?? null,
      ),
    );
    for (const value of values) this.check(value);
    return values;
  }
  async prepare(input: TranscriptionJobRequest) {
    return this.check(
      applicationResult(
        await commands.deviceTranscriptionSubmissionPrepare(this.scope, input),
      ),
    );
  }
  async load(requestId: string) {
    const value = applicationResult(
      await commands.deviceTranscriptionSubmissionGet(this.scope, requestId),
    );
    return value ? this.check(value) : null;
  }
  async observe(requestId: string, job: TranscriptionJobSnapshot) {
    return this.check(
      applicationResult(
        await commands.deviceTranscriptionSubmissionObserve(
          this.scope,
          requestId,
          job,
        ),
      ),
    );
  }
  private check(value: TranscriptionSubmissionReceipt) {
    if (value.scope !== this.scope)
      throw new Error(
        "This saved transcription request belongs to another account or vault",
      );
    return value;
  }
}

export class TranscriptionSubmissions {
  constructor(
    readonly scope: VaultScope,
    readonly store: Pick<
      NativeTranscriptionSubmissions,
      "list" | "prepare" | "load" | "observe"
    > = new NativeTranscriptionSubmissions(scope.client.scope),
  ) {}

  async transcribe(sessionId: string, options?: RequestOptions) {
    const guard = this.guard(options);
    try {
      const api = this.api();
      const receipts = await this.store.list(sessionId);
      guard.check();
      if (
        receipts.some(
          (receipt) => !receipt.job || activeTranscription(receipt.job),
        )
      )
        throw new Error(
          "Recover or finish the previous transcription request before starting another",
        );
      const policy = await this.scope.run(() => api.policy(options));
      guard.check();
      if (
        !policy.batch_enabled ||
        policy.availability !== "ready" ||
        !policy.model
      )
        throw new Error("Transcription is not ready for this connection");
      const metadata = await this.scope.run(() =>
        this.scope.client.audioMetadata(sessionId, options),
      );
      guard.check();
      if (!metadata) throw new Error("This note has no saved recording");
      const receipt = await this.store.prepare({
        session_id: sessionId,
        expected_recording_sha256: metadata.sha256,
        expected_transcript: "absent",
        model: policy.model,
      });
      guard.check();
      return await this.dispatch(receipt, guard.check, options);
    } finally {
      guard.dispose();
    }
  }

  async resume(requestId: string, options?: RequestOptions) {
    const guard = this.guard(options);
    try {
      const receipt = await this.store.load(requestId);
      guard.check();
      if (!receipt || receipt.scope !== this.scope.client.scope)
        throw new Error(
          "Saved transcription request not found for this account and vault",
        );
      return await this.dispatch(receipt, guard.check, options);
    } finally {
      guard.dispose();
    }
  }

  private async dispatch(
    receipt: TranscriptionSubmissionReceipt,
    check: () => void,
    options?: RequestOptions,
  ) {
    const api = this.api();
    check();
    const job = await this.scope.run(() =>
      receipt.job
        ? api.get(receipt.job.id, options)
        : api.submit(receipt.input, {
            ...options,
            requestId: receipt.request_id,
          }),
    );
    return this.store.observe(receipt.request_id, job);
  }

  private api() {
    const api = this.scope.client.transcriptionJobs;
    if (
      !api ||
      !["batch_transcription_jobs", "transcription_policy"].every((feature) =>
        this.scope.client.features.includes(feature),
      )
    )
      throw new Error("This connection does not support transcription jobs");
    return api;
  }

  private guard(options?: RequestOptions) {
    const client = this.scope.client;
    let invalid = false;
    const check = () => {
      if (
        invalid ||
        options?.signal?.aborted ||
        this.scope.client !== client ||
        !client.canEdit ||
        this.scope.getSnapshot().state !== "connected"
      ) {
        invalid = true;
        throw new Error(
          "This connection is no longer available for transcription",
        );
      }
      this.api();
    };
    check();
    const stop = this.scope.subscribe(() => {
      if (this.scope.getSnapshot().state !== "connected") invalid = true;
    });
    return { check, dispose: stop };
  }
}
