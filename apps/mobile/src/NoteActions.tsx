import { useMutation } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, MoreHorizontal, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { formatTranscriptExportSegments } from "@hypr/utils";

import { errorMessage, useCommand, type Session } from "./api";

export function NoteActions({
  session,
  notes,
  dirty = false,
  recording,
  processing,
  onDeleted,
}: {
  session: Session;
  notes: string;
  dirty?: boolean;
  recording: boolean;
  processing: boolean;
  onDeleted: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const remove = useCommand("mobile_delete_session");
  const copy = useMutation({
    mutationFn: async (kind: "notes" | "summary" | "transcript") => {
      const text =
        kind === "transcript"
          ? formatTranscriptExportSegments(
              await invoke<{ speaker: string; text: string }[]>(
                "mobile_transcript_export",
                { sessionId: session.id },
              ),
            )
          : kind === "summary"
            ? session.summary || ""
            : notes;
      if (!text.trim()) throw new Error("There is no content to copy yet.");
      await invoke("mobile_copy_text", { text });
      return kind === "notes"
        ? "Notes copied"
        : kind === "summary"
          ? "Summary copied"
          : "Transcript copied";
    },
  });
  useEffect(() => {
    if (!copy.isSuccess) return;
    const timer = window.setTimeout(() => copy.reset(), 1600);
    return () => window.clearTimeout(timer);
  }, [copy.isSuccess, copy.submittedAt, copy.reset]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  useEffect(() => {
    if (confirmDelete) dialog.current?.showModal();
  }, [confirmDelete]);
  const cancelDelete = () => {
    if (remove.isPending) return;
    setConfirmDelete(false);
    trigger.current?.focus();
  };
  return (
    <div className="note-actions" ref={root}>
      <button
        type="button"
        ref={trigger}
        className="icon"
        aria-label="Note actions"
        aria-expanded={open}
        aria-controls="note-actions-panel"
        onClick={() => {
          copy.reset();
          setOpen(!open);
        }}
      >
        <MoreHorizontal size={22} />
      </button>
      {open && (
        <div
          id="note-actions-panel"
          className="note-actions-panel"
          aria-label="Note actions"
        >
          {(
            [
              ["notes", "Copy notes", !!notes.trim()],
              ["summary", "Copy summary", !!session.summary?.trim()],
              ["transcript", "Copy transcript", !!session.transcript.length],
            ] as const
          )
            .filter(([, , hasContent]) => hasContent)
            .map(([kind, label]) => {
              const copied = copy.isSuccess && copy.variables === kind;
              const pending = copy.isPending && copy.variables === kind;
              return (
                <button
                  key={kind}
                  type="button"
                  disabled={copy.isPending}
                  onClick={() => copy.mutate(kind)}
                >
                  {copied ? (
                    <Check size={16} className="copy-confirmed" />
                  ) : (
                    <Copy size={16} />
                  )}
                  <span aria-live="polite" aria-atomic="true">
                    {copied ? copy.data : pending ? "Copying…" : label}
                  </span>
                </button>
              );
            })}
          {copy.error && (
            <p className="error" role="alert">
              {errorMessage(copy.error)}
            </p>
          )}
          <button
            type="button"
            className="danger"
            disabled={recording || processing}
            onClick={() => {
              setOpen(false);
              remove.reset();
              setConfirmDelete(true);
            }}
          >
            <Trash2 size={16} />
            Delete note
          </button>
          {(recording || processing) && (
            <p className="caption">
              {recording
                ? "Stop recording to delete this note."
                : "Pause processing in Background activity to delete this note."}
            </p>
          )}
        </div>
      )}
      {confirmDelete && (
        <dialog
          ref={dialog}
          className="discard-dialog"
          aria-labelledby="delete-note-title"
          aria-describedby="delete-note-description"
          onCancel={(event) => {
            event.preventDefault();
            cancelDelete();
          }}
        >
          <h2 id="delete-note-title">Delete this note?</h2>
          <p id="delete-note-description">
            The note, transcript, summary, recording and attachments will move
            to your vault’s trash. You can undo this after deleting.
          </p>
          {dirty && (
            <p className="error">
              Unsaved edits will be discarded. Undo restores the last saved
              version.
            </p>
          )}
          <div className="dialog-actions">
            <button
              type="button"
              disabled={remove.isPending}
              onClick={cancelDelete}
            >
              Keep note
            </button>
            <button
              type="button"
              className="danger"
              disabled={remove.isPending}
              onClick={() =>
                remove.mutate(
                  { sessionId: session.id },
                  { onSuccess: onDeleted },
                )
              }
            >
              {remove.isPending ? "Deleting…" : "Delete note"}
            </button>
          </div>
          {remove.error && (
            <p className="error" role="alert">
              {errorMessage(remove.error)}
            </p>
          )}
        </dialog>
      )}
    </div>
  );
}
