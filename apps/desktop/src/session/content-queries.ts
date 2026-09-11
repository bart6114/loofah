import { DEFAULT_USER_ID } from "~/shared/utils";
import type { SpeakerHintWithId, WordWithId } from "~/stt/types";
import { commands } from "~/types/tauri.gen";

export type SessionContentSnapshot = {
  sessionId: string;
  ownerUserId: string;
  title: string;
  createdAt: string;
  rawNoteId: string | null;
  rawContent: string;
  rawContentFormat: string;
  rawMarkdown: string;
  enhancedNotes: Array<{
    id: string;
    title: string;
    markdown: string;
    content: string;
    contentFormat: string;
    templateId: string;
    kind: string;
    position: number;
  }>;
  transcripts: Array<{
    id: string;
    started_at: number;
    ended_at: number | null;
    memo: string;
    wordsJson: string;
    words: WordWithId[];
    speaker_hints: SpeakerHintWithId[];
  }>;
};

/**
 * One coherent read of everything the export/AI flows need for a session, off the
 * file-backed index (the store-command equivalent of the old single-row SQL join).
 * The three commands are synchronous index reads, so the combined result is as
 * consistent as the old one-statement snapshot for all practical purposes.
 */
export async function loadSessionContentSnapshot(
  sessionId: string,
): Promise<SessionContentSnapshot | null> {
  if (!sessionId) return null;

  const sessionRead = await commands.sessionGet(sessionId);
  if (sessionRead.status === "error") {
    throw new Error(sessionRead.error);
  }
  const session = sessionRead.data;
  if (!session) return null;

  const docsRead = await commands.sessionEnhancedDocs(sessionId);
  if (docsRead.status === "error") {
    throw new Error(docsRead.error);
  }
  const transcriptsRead = await commands.sessionTranscripts(sessionId);
  if (transcriptsRead.status === "error") {
    throw new Error(transcriptsRead.error);
  }

  // `session_enhanced_docs` already orders (sort_order, id) and only returns live
  // (file-backed) docs -- the old `deleted_at IS NULL` filter is inherent now.
  const enhancedNotes = docsRead.data.map((doc) => ({
    id: doc.id,
    title: doc.title,
    markdown: doc.markdown,
    // The file body is markdown-canonical; `content`/`contentFormat` keep the old
    // row-shaped fields alive for consumers that still pass them around.
    content: doc.markdown,
    contentFormat: "md",
    templateId: doc.template_id,
    kind: doc.kind,
    position: Number(doc.sort_order),
  }));

  // `session_transcripts` already orders (started_at, id); everything in the file is
  // live -- superseded transcripts leave the file via the supersede primitive, so file
  // truth matches the old `deleted_at IS NULL` filtered truth.
  const transcripts = transcriptsRead.data.map((transcript) => ({
    id: transcript.id,
    started_at: Number(transcript.started_at),
    ended_at: transcript.ended_at == null ? null : Number(transcript.ended_at),
    memo: transcript.memo_md ?? "",
    wordsJson: JSON.stringify(transcript.words ?? []),
    words: (transcript.words ?? []) as unknown as WordWithId[],
    speaker_hints: (transcript.speaker_hints ??
      []) as unknown as SpeakerHintWithId[],
  }));

  const rawMarkdown = session.note_markdown ?? "";

  return {
    sessionId: session.meta.id,
    // The owner concept died with the workspaces removal (D10).
    ownerUserId: DEFAULT_USER_ID,
    title: session.meta.title,
    createdAt: session.meta.created_at,
    rawNoteId: session.note_markdown == null ? null : `${sessionId}:note`,
    rawContent: rawMarkdown,
    rawContentFormat: "md",
    rawMarkdown,
    enhancedNotes,
    transcripts,
  };
}

export async function loadActiveSessionIds(): Promise<string[]> {
  const result = await commands.sessionIds();
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}
