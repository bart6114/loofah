import { type EnhanceEligibilitySkipCode, getEligibility } from "./eligibility";

import { beginSummaryActivity } from "~/session/summary-activity";
import {
  activeJob,
  JobSubmissions,
} from "~/shared/application/job-submissions";
import type { DocumentTarget } from "~/shared/application/protocol.gen";
import type { VaultScope } from "~/shared/application/vault-scope";
import type { ListenerStore } from "~/store/zustand/listener";

export type EnhanceResult =
  | { type: "started"; noteId: string }
  | { type: "already_active"; noteId: string }
  | { type: "no_model" };

type QueueEmptySummaryResult =
  | { type: "queued" }
  | { type: "summary_exists"; noteId: string };

type EnhanceOpts = {
  regenerate?: boolean;
  isAuto?: boolean;
  targetNoteId?: string;
};

type EnhancerEvent =
  | {
      type: "auto-enhance-skipped";
      sessionId: string;
      reason: string;
      reasonCode: EnhanceEligibilitySkipCode | "error";
    }
  | { type: "auto-enhance-started"; sessionId: string; noteId: string }
  | { type: "auto-enhance-no-model"; sessionId: string };

type EnhancerDeps = { scope: VaultScope; capture?: ListenerStore };

type TiptapNode = {
  content?: TiptapNode[];
  text?: string;
};

function hasTiptapText(node: TiptapNode): boolean {
  if (typeof node.text === "string" && node.text.trim()) {
    return true;
  }

  return node.content?.some(hasTiptapText) ?? false;
}

function hasSummaryContent(value: unknown): boolean {
  if (typeof value !== "string") {
    return false;
  }

  const trimmed = value.trim();
  if (!trimmed) {
    return false;
  }

  if (!trimmed.startsWith("{")) {
    return true;
  }

  try {
    const parsed = JSON.parse(trimmed);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      (parsed as { type?: unknown }).type === "doc"
    ) {
      return hasTiptapText(parsed);
    }
    return true;
  } catch {
    return true;
  }
}

let instance: EnhancerService | null = null;

export function getEnhancerService(): EnhancerService | null {
  return instance;
}

export function initEnhancerService(deps: EnhancerDeps): EnhancerService {
  instance?.dispose();
  instance = new EnhancerService(deps);
  instance.start();
  return instance;
}

export class EnhancerService {
  readonly ownerId = crypto.randomUUID();
  readonly scope: VaultScope;
  private readonly controller: JobSubmissions;
  private readonly stop = new AbortController();
  private pendingEnhance = new Map<string, Promise<EnhanceResult>>();
  private activeAutoEnhance = new Map<
    string,
    { activity: ReturnType<typeof beginSummaryActivity>; stop: AbortController }
  >();
  private pendingRetries = new Map<string, ReturnType<typeof setTimeout>>();
  private unsubscribe: (() => void) | null = null;
  private eventListeners = new Set<(event: EnhancerEvent) => void>();

  constructor(private deps: EnhancerDeps) {
    this.scope = deps.scope;
    this.controller = new JobSubmissions(deps.scope);
  }

  assertCurrent() {
    if (
      this.stop.signal.aborted ||
      this.scope.getSnapshot().state !== "connected" ||
      !this.scope.client.canEdit
    )
      throw new Error("The summary owner is no longer available.");
  }

  start() {
    this.unsubscribe =
      this.deps.capture?.subscribe((state) => {
        if (state.live.status === "active" && state.live.sessionId) {
          this.finishAutoEnhance(state.live.sessionId);
          this.clearRetry(state.live.sessionId);
        }
      }) ?? null;
  }

  dispose() {
    this.stop.abort();
    this.unsubscribe?.();
    this.unsubscribe = null;
    for (const timer of this.pendingRetries.values()) clearTimeout(timer);
    this.pendingRetries.clear();
    for (const id of [...this.activeAutoEnhance.keys()])
      this.finishAutoEnhance(id);
    this.eventListeners.clear();
    if (instance === this) instance = null;
  }

  on(listener: (event: EnhancerEvent) => void): () => void {
    this.eventListeners.add(listener);
    return () => this.eventListeners.delete(listener);
  }

  private emit(event: EnhancerEvent) {
    this.eventListeners.forEach((listener) => listener(event));
  }

  async checkEligibility(sessionId: string) {
    this.assertCurrent();
    const transcripts = await this.scope.run(() =>
      this.scope.client.transcripts(sessionId, { signal: this.stop.signal }),
    );
    this.assertCurrent();
    return getEligibility(
      transcripts.map((entry) => ({ words: entry.words ?? [] })),
    );
  }

  queueAutoEnhance(sessionId: string, regenerate = true) {
    this.assertCurrent();
    if (this.activeAutoEnhance.has(sessionId)) return;
    const owner = {
      activity: beginSummaryActivity(sessionId, "processing"),
      stop: new AbortController(),
    };
    this.activeAutoEnhance.set(sessionId, owner);
    void this.tryAutoEnhance(sessionId, 0, regenerate, owner).catch((error) => {
      if (this.activeAutoEnhance.get(sessionId) === owner)
        this.handleAutoEnhanceError(sessionId, error);
    });
  }

  async queueAutoEnhanceIfSummaryEmpty(
    sessionId: string,
  ): Promise<QueueEmptySummaryResult> {
    this.assertCurrent();
    const current = await this.scope.run(() =>
      this.scope.client.readDocument(
        sessionId,
        { kind: "summary" },
        { signal: this.stop.signal },
      ),
    );
    this.assertCurrent();
    if (hasSummaryContent(current.document?.markdown))
      return { type: "summary_exists", noteId: sessionId };
    this.queueAutoEnhance(sessionId, false);
    return { type: "queued" };
  }

  private async tryAutoEnhance(
    sessionId: string,
    attempt: number,
    regenerate: boolean,
    owner: {
      activity: ReturnType<typeof beginSummaryActivity>;
      stop: AbortController;
    },
  ) {
    if (this.activeAutoEnhance.get(sessionId) !== owner) return;
    const eligibility = await this.checkEligibility(sessionId);
    if (this.activeAutoEnhance.get(sessionId) !== owner) return;
    if (!eligibility.eligible) {
      if (attempt < 20) {
        this.pendingRetries.set(
          sessionId,
          setTimeout(() => {
            this.pendingRetries.delete(sessionId);
            void this.tryAutoEnhance(
              sessionId,
              attempt + 1,
              regenerate,
              owner,
            ).catch((error) => {
              if (this.activeAutoEnhance.get(sessionId) === owner)
                this.handleAutoEnhanceError(sessionId, error);
            });
          }, 500),
        );
        return;
      }
      this.finishAutoEnhance(sessionId);
      this.emit({
        type: "auto-enhance-skipped",
        sessionId,
        reason: eligibility.reason,
        reasonCode: eligibility.code,
      });
      return;
    }
    const signal = AbortSignal.any([this.stop.signal, owner.stop.signal]);
    const result = await this.enhance(
      sessionId,
      { isAuto: true, regenerate },
      signal,
    );
    if (this.activeAutoEnhance.get(sessionId) !== owner) return;
    this.finishAutoEnhance(sessionId);
    if (result.type === "no_model")
      this.emit({ type: "auto-enhance-no-model", sessionId });
    else
      this.emit({
        type: "auto-enhance-started",
        sessionId,
        noteId: result.noteId,
      });
  }

  private handleAutoEnhanceError(sessionId: string, error: unknown) {
    this.finishAutoEnhance(sessionId);
    this.clearRetry(sessionId);
    this.emit({
      type: "auto-enhance-skipped",
      sessionId,
      reason: error instanceof Error ? error.message : String(error),
      reasonCode: "error",
    });
  }

  private finishAutoEnhance(sessionId: string) {
    const owner = this.activeAutoEnhance.get(sessionId);
    this.activeAutoEnhance.delete(sessionId);
    owner?.stop.abort();
    owner?.activity.finish();
  }

  private clearRetry(sessionId: string) {
    const timer = this.pendingRetries.get(sessionId);
    if (timer) {
      clearTimeout(timer);
      this.pendingRetries.delete(sessionId);
    }
  }

  async resetEnhanceTasks(sessionId: string): Promise<void> {
    this.assertCurrent();
    this.finishAutoEnhance(sessionId);
    this.clearRetry(sessionId);
    const receipts = await this.controller.store.list(sessionId);
    this.assertCurrent();
    if (receipts.some((receipt) => !receipt.job))
      throw new Error(
        "Recover the previous summary request before starting another.",
      );
    const jobs = await this.scope.run(() =>
      this.scope.client.processingJobs(
        { session_id: sessionId, active_only: true, after: null, limit: 200 },
        { signal: this.stop.signal },
      ),
    );
    this.assertCurrent();
    for (const job of jobs.items.filter(activeJob)) {
      await this.controller.cancel(job.id, { signal: this.stop.signal });
      this.assertCurrent();
    }
  }

  async enhance(
    sessionId: string,
    opts?: EnhanceOpts,
    signal = this.stop.signal,
  ): Promise<EnhanceResult> {
    this.assertCurrent();
    const key = JSON.stringify([sessionId, opts?.targetNoteId ?? sessionId]);
    const pending = this.pendingEnhance.get(key);
    if (pending) return pending;
    const activity = beginSummaryActivity(sessionId, "processing");
    const request = this.startEnhance(sessionId, opts, signal);
    this.pendingEnhance.set(key, request);
    try {
      return await request;
    } finally {
      activity.finish();
      if (this.pendingEnhance.get(key) === request)
        this.pendingEnhance.delete(key);
    }
  }

  private async startEnhance(
    sessionId: string,
    opts: EnhanceOpts | undefined,
    signal: AbortSignal,
  ): Promise<EnhanceResult> {
    const target: DocumentTarget =
      !opts?.targetNoteId || opts.targetNoteId === sessionId
        ? { kind: "summary" }
        : { kind: "template_output", document_id: opts.targetNoteId };
    const noteId = target.kind === "summary" ? sessionId : target.document_id;
    this.assertCurrent();
    const current = await this.scope.run(() =>
      this.scope.client.readDocument(sessionId, target, { signal }),
    );
    this.assertCurrent();
    signal.throwIfAborted();
    if (
      target.kind === "template_output" &&
      (!current.document ||
        current.document.kind !== "template_output" ||
        current.document.id !== noteId ||
        current.document.session_id !== sessionId)
    )
      throw new Error(
        "This template output is no longer available for regeneration.",
      );
    if (
      !opts?.targetNoteId &&
      !opts?.regenerate &&
      hasSummaryContent(current.document?.markdown)
    )
      return { type: "already_active", noteId };
    await this.controller.generateDocument(sessionId, target, {
      signal,
      titlePolicy: this.scope.client.features.includes("generated_titles")
        ? "generate_if_empty"
        : "preserve",
      onlyIfEmpty: !!opts?.isAuto && !opts.regenerate,
    });
    this.assertCurrent();
    return { type: "started", noteId };
  }
}
