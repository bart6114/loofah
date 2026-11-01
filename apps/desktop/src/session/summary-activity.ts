import { create } from "zustand";

export type SummaryActivity = "recording" | "processing";

export const useSummaryActivity = create(() => ({
  sessions: {} as Record<string, SummaryActivity>,
  remoteSessions: {} as Record<string, SummaryActivity>,
  remoteReady: false,
}));

const operations = new Map<
  symbol,
  { sessionId: string; phase: SummaryActivity }
>();

function publish() {
  const sessions: Record<string, SummaryActivity> = {};
  for (const { sessionId, phase } of operations.values()) {
    if (sessions[sessionId] !== "recording") sessions[sessionId] = phase;
  }
  useSummaryActivity.setState({ sessions });
}

export function beginSummaryActivity(
  sessionId: string,
  phase: SummaryActivity,
) {
  // A stale completion must only release its own operation, never a resumed capture.
  const token = Symbol(sessionId);
  operations.set(token, { sessionId, phase });
  publish();
  return {
    setPhase(next: SummaryActivity) {
      if (!operations.has(token)) return;
      operations.set(token, { sessionId, phase: next });
      publish();
    },
    finish() {
      if (operations.delete(token)) publish();
    },
  };
}
