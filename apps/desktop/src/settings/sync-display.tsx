import { useMemo } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import type { SyncStatus } from "~/types/tauri.gen";

export function deviceLabel(id: string, status?: SyncStatus) {
  const name = status?.devices.find((device) => device.id === id)?.name;
  if (id === status?.currentDeviceId)
    return name ? `${name} (This Mac)` : "This Mac";
  return name || `Device ${id.slice(0, 8)}`;
}

export function componentLabels(files: string[]) {
  return [
    ...new Set(
      files.map((file) => {
        if (file === "notes.md" || file === "_memo.md") return "Notes";
        if (file === "_meta.json") return "Session details";
        if (file === "transcript.json") return "Transcript";
        if (file === "tasks.json" || file === "_tasks.json") return "Tasks";
        if (file.includes("people")) return "People";
        if (file.includes("tags")) return "Tags";
        if (file.startsWith("enhanced/")) return "AI documents";
        if (file.startsWith("attachments/")) return "Attachments";
        if (file.startsWith("audio")) return "Recording";
        return file;
      }),
    ),
  ].join(", ");
}

export function SyncConfirmation({
  title,
  children,
  confirmLabel,
  pending,
  onConfirm,
  onClose,
}: {
  title: string;
  children: React.ReactNode;
  confirmLabel: string;
  pending: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        <DialogDescription asChild>
          <div className="flex flex-col gap-3">{children}</div>
        </DialogDescription>
        <div className="flex justify-end gap-2">
          <Button variant="outline" disabled={pending} onClick={onClose}>
            Cancel
          </Button>
          <Button variant="destructive" disabled={pending} onClick={onConfirm}>
            {pending ? "Please wait…" : confirmLabel}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export function SyncPreviewText({
  kind,
  text,
  deleted,
}: {
  kind: string;
  text: string;
  deleted: boolean;
}) {
  const records = useMemo(() => {
    if (kind === "session" || !text || deleted) return null;
    try {
      const parsed: unknown = JSON.parse(text);
      if (typeof parsed !== "object" || parsed === null) return null;
      const list = (parsed as Record<string, unknown>)[kind];
      if (!Array.isArray(list)) return null;
      return list.map((item: unknown) => {
        if (typeof item === "string") return item;
        if (!item || typeof item !== "object") return "Unnamed item";
        const record = item as Record<string, unknown>;
        const label =
          typeof record.name === "string"
            ? record.name
            : typeof record.text === "string"
              ? record.text
              : "Unnamed item";
        return `${label}${kind === "tasks" && typeof record.status === "string" ? ` · ${{ todo: "To do", in_progress: "In progress", done: "Done" }[record.status] ?? record.status}` : ""}`;
      });
    } catch {
      return null;
    }
  }, [kind, text, deleted]);
  if (records)
    return (
      <div className="flex flex-col gap-2">
        <p>
          {records.length} {kind}
        </p>
        <ul className="max-h-64 list-inside list-disc overflow-auto text-sm">
          {records.slice(0, 200).map((label, index) => (
            <li key={index}>{label}</li>
          ))}
        </ul>
        {records.length > 200 && (
          <p className="text-sm">
            Showing the first 200. Export this version to review all items.
          </p>
        )}
        <details>
          <summary className="cursor-pointer text-sm">
            View complete saved data
          </summary>
          <pre className="max-h-64 overflow-auto text-sm whitespace-pre-wrap">
            {text}
          </pre>
        </details>
      </div>
    );
  return (
    <pre className="max-h-64 overflow-auto rounded border p-3 text-sm whitespace-pre-wrap">
      {text ||
        (deleted
          ? "Deleted version"
          : "No note text. Check the included content or export this version to review its files.")}
    </pre>
  );
}
