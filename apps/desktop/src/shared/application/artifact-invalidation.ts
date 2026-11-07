import type { ArtifactTarget, Change } from "./protocol.gen";

export function artifactInvalidation(
  change: Extract<Change, { kind: "index_changed" }>,
) {
  const ids = new Set(change.ids);
  const bySession = new Map<string, ArtifactTarget[]>();
  for (const detail of change.artifacts ?? []) {
    const targets = bySession.get(detail.session_id) ?? [];
    targets.push(detail.target);
    bySession.set(detail.session_id, targets);
  }
  return (
    kind: "recording" | "attachments",
    sessionId: string,
    attachmentId?: string,
  ) => {
    if (change.entity !== "artifacts" || (ids.size && !ids.has(sessionId)))
      return false;
    const targets = bySession.get(sessionId);
    // Legacy events, dropped detail budgets and broad rebuilds need a safe refresh.
    if (!targets?.length) return true;
    return targets.some((target) => {
      switch (target.kind) {
        case "recording":
          return kind === "recording";
        case "attachments":
          return kind === "attachments";
        case "attachment":
          return (
            kind === "attachments" &&
            (attachmentId === undefined ||
              attachmentId === target.attachment_id)
          );
        default:
          return true;
      }
    });
  };
}
