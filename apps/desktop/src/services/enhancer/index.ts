import type { LanguageModel } from "ai";

import { type EnhanceEligibilitySkipCode, getEligibility } from "./eligibility";
import { EMPTY_SUMMARY_SOURCE_MESSAGE, hasSummarySource } from "./source";
import {
  type EnhancerNote,
  ensureSummaryDocument,
  selectSummaryDocument,
} from "./storage";

import {
  loadSessionContentSnapshot,
  type SessionContentSnapshot,
} from "~/session/content-queries";
import { flushDatabaseWrites } from "~/shared/write-queue";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";
import type { TasksActions } from "~/store/zustand/ai-task/tasks";
import { listenerStore } from "~/store/zustand/listener/instance";

export type EnhanceResult =
  | { type: "started"; noteId: string }
  | { type: "already_active"; noteId: string }
  | { type: "no_model" };

type QueueEmptySummaryResult =
  | { type: "queued" }
  | { type: "summary_exists"; noteId: string };

type EnhanceOpts = {
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

type EnhancerDeps = {
  aiTaskStore: {
    getState: () => Pick<TasksActions, "generate" | "getState" | "reset">;
  };
  getModel: () => LanguageModel | null;
};

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
  private pendingEnhance = new Map<string, Promise<EnhanceResult>>();
  private activeAutoEnhance = new Set<string>();
  private pendingRetries = new Map<string, ReturnType<typeof setTimeout>>();
  private unsubscribe: (() => void) | null = null;
  private eventListeners = new Set<(event: EnhancerEvent) => void>();

  constructor(private deps: EnhancerDeps) {}

  start() {
    this.unsubscribe = listenerStore.subscribe((state) => {
      const { status, sessionId } = state.live;

      if (status === "active" && sessionId) {
        this.activeAutoEnhance.delete(sessionId);
        this.clearRetry(sessionId);
      }
    });
  }

  dispose() {
    this.unsubscribe?.();
    this.unsubscribe = null;
    for (const timer of this.pendingRetries.values()) clearTimeout(timer);
    this.pendingRetries.clear();
    this.activeAutoEnhance.clear();
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
    const snapshot = await this.loadSession(sessionId);
    return getEligibility(snapshot.transcripts);
  }

  queueAutoEnhance(sessionId: string) {
    if (this.activeAutoEnhance.has(sessionId)) return;
    this.activeAutoEnhance.add(sessionId);
    void this.tryAutoEnhance(sessionId, 0).catch((error) => {
      this.handleAutoEnhanceError(sessionId, error);
    });
  }

  async queueAutoEnhanceIfSummaryEmpty(
    sessionId: string,
  ): Promise<QueueEmptySummaryResult> {
    const snapshot = await this.loadSession(sessionId);
    const existingNote = selectSummaryDocument(snapshot.enhancedNotes);

    if (existingNote && hasSummaryContent(existingNote.content)) {
      return { type: "summary_exists", noteId: existingNote.id };
    }

    if (!existingNote) {
      const eligibility = getEligibility(snapshot.transcripts);
      if (!eligibility.eligible && eligibility.wordCount > 0) {
        await this.ensureNote(sessionId);
      }
    }

    this.queueAutoEnhance(sessionId);
    return { type: "queued" };
  }

  private async tryAutoEnhance(sessionId: string, attempt: number) {
    if (!this.activeAutoEnhance.has(sessionId)) return;

    const eligibility = await this.checkEligibility(sessionId);
    if (!this.activeAutoEnhance.has(sessionId)) return;

    if (!eligibility.eligible) {
      if (attempt < 20) {
        const timer = setTimeout(() => {
          this.pendingRetries.delete(sessionId);
          void this.tryAutoEnhance(sessionId, attempt + 1).catch((error) => {
            this.handleAutoEnhanceError(sessionId, error);
          });
        }, 500);
        this.pendingRetries.set(sessionId, timer);
        return;
      }

      this.activeAutoEnhance.delete(sessionId);
      this.emit({
        type: "auto-enhance-skipped",
        sessionId,
        reason: eligibility.reason,
        reasonCode: eligibility.code,
      });
      return;
    }

    const result = await this.enhance(sessionId, { isAuto: true });
    if (!this.activeAutoEnhance.has(sessionId)) return;

    if (result.type === "no_model") {
      this.activeAutoEnhance.delete(sessionId);
      this.emit({ type: "auto-enhance-no-model", sessionId });
      return;
    }

    this.activeAutoEnhance.delete(sessionId);
    this.emit({
      type: "auto-enhance-started",
      sessionId,
      noteId: result.noteId,
    });
  }

  private handleAutoEnhanceError(sessionId: string, error: unknown) {
    this.activeAutoEnhance.delete(sessionId);
    this.clearRetry(sessionId);
    const reason = error instanceof Error ? error.message : String(error);
    console.error("[enhancer] auto-enhance failed", error);
    this.emit({
      type: "auto-enhance-skipped",
      sessionId,
      reason,
      reasonCode: "error",
    });
  }

  private clearRetry(sessionId: string) {
    const timer = this.pendingRetries.get(sessionId);
    if (timer) {
      clearTimeout(timer);
      this.pendingRetries.delete(sessionId);
    }
  }

  async resetEnhanceTasks(sessionId: string): Promise<void> {
    const snapshot = await this.loadSession(sessionId);
    const { aiTaskStore } = this.deps;
    for (const note of snapshot.enhancedNotes) {
      aiTaskStore.getState().reset(createTaskId(note.id, "enhance"));
    }
  }

  async enhance(sessionId: string, opts?: EnhanceOpts): Promise<EnhanceResult> {
    const key = `${sessionId}:${opts?.targetNoteId ?? "default"}`;
    const pending = this.pendingEnhance.get(key);
    if (pending) return pending;
    const request = this.startEnhance(sessionId, opts);
    this.pendingEnhance.set(key, request);
    try {
      return await request;
    } finally {
      if (this.pendingEnhance.get(key) === request)
        this.pendingEnhance.delete(key);
    }
  }

  private async startEnhance(
    sessionId: string,
    opts?: EnhanceOpts,
  ): Promise<EnhanceResult> {
    const { aiTaskStore, getModel } = this.deps;

    const model = getModel();
    if (!model) return { type: "no_model" };

    await flushDatabaseWrites([`session:${sessionId}:note`]);
    const snapshot = await this.loadSession(sessionId);
    if (!hasSummarySource(snapshot.rawMarkdown, snapshot.transcripts)) {
      throw new Error(EMPTY_SUMMARY_SOURCE_MESSAGE);
    }
    const targetNote = opts?.targetNoteId
      ? getSessionEnhancedNote(snapshot, opts.targetNoteId)
      : undefined;
    if (opts?.targetNoteId && !targetNote)
      throw new Error("Summary no longer exists");
    const note =
      targetNote ??
      selectSummaryDocument(snapshot.enhancedNotes) ??
      (await ensureSummaryDocument(sessionId));
    const enhanceTaskId = createTaskId(note.id, "enhance");
    const existingTask = aiTaskStore.getState().getState(enhanceTaskId);
    if (existingTask?.status === "generating") {
      return { type: "already_active", noteId: note.id };
    }

    if (
      !targetNote &&
      !opts?.isAuto &&
      existingTask?.status === "success" &&
      hasSummaryContent(note.content)
    ) {
      return { type: "already_active", noteId: note.id };
    }

    void aiTaskStore.getState().generate(enhanceTaskId, {
      model,
      taskType: "enhance",
      args: { sessionId, enhancedNoteId: note.id },
    });

    return { type: "started", noteId: note.id };
  }

  async ensureNote(sessionId: string): Promise<string> {
    return (await ensureSummaryDocument(sessionId)).id;
  }

  private async loadSession(sessionId: string) {
    const snapshot = await loadSessionContentSnapshot(sessionId);
    if (!snapshot) {
      throw new Error(`Session ${sessionId} no longer exists`);
    }
    return snapshot;
  }
}

function getSessionEnhancedNote(
  snapshot: SessionContentSnapshot,
  noteId: string,
): EnhancerNote | undefined {
  return snapshot.enhancedNotes.find((note) => note.id === noteId);
}
