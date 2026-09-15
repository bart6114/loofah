import { Trans, useLingui } from "@lingui/react/macro";
import { downloadDir } from "@tauri-apps/api/path";
import { open as selectFile } from "@tauri-apps/plugin-dialog";
import { CheckIcon, RotateCcwIcon } from "lucide-react";
import { useCallback, useRef, useState } from "react";

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";
import { Button } from "@hypr/ui/components/ui/button";
import { Checkbox } from "@hypr/ui/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";
import { Progress } from "@hypr/ui/components/ui/progress";
import { Spinner } from "@hypr/ui/components/ui/spinner";
import { cn } from "@hypr/utils";

import { useNativeFileDrop } from "~/shared/hooks/useNativeFileDrop";
import {
  type AudioImportItem,
  type AudioImportSource,
  isFinishedAudioImportStatus,
  useAudioImport,
} from "~/store/zustand/audio-import";
import { AUDIO_EXTENSIONS } from "~/stt/useUploadFile";

type Candidate = {
  key: string;
  source: AudioImportSource;
  size: number | null;
  selected: boolean;
};

export function AudioImportDialog() {
  const { t } = useLingui();
  const dialogOpen = useAudioImport((state) => state.dialogOpen);
  const setDialogOpen = useAudioImport((state) => state.setDialogOpen);
  const items = useAudioImport((state) => state.items);
  const enqueue = useAudioImport((state) => state.enqueue);
  const retryItem = useAudioImport((state) => state.retryItem);
  const clearFinished = useAudioImport((state) => state.clearFinished);

  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const dropTargetRef = useRef<HTMLDivElement>(null);

  const addCandidates = useCallback((incoming: Candidate[]) => {
    setCandidates((current) => {
      const seen = new Set(current.map((candidate) => candidate.key));
      const fresh = incoming.filter((candidate) => {
        if (seen.has(candidate.key)) {
          return false;
        }
        seen.add(candidate.key);
        return true;
      });
      return fresh.length ? [...current, ...fresh] : current;
    });
  }, []);

  const handleChooseFiles = useCallback(async () => {
    try {
      const selection = await selectFile({
        title: t`Import audio files`,
        multiple: true,
        directory: false,
        defaultPath: await downloadDir(),
        filters: [{ name: "Audio", extensions: AUDIO_EXTENSIONS }],
      });

      const paths = Array.isArray(selection)
        ? selection
        : selection
          ? [selection]
          : [];
      addCandidates(
        paths.map((path) => ({
          key: path,
          source: { kind: "path", path, name: fileNameFromPath(path) },
          size: null,
          selected: true,
        })),
      );
    } catch (error) {
      console.error("[audio-import] file dialog failed:", error);
    }
  }, [addCandidates, t]);

  const { isHovering: isDragActive } = useNativeFileDrop(dropTargetRef, {
    onDrop: (paths) => {
      void fsSyncCommands
        .audioCollectImportSources(paths)
        .then((result) => {
          if (result.status === "error") throw new Error(result.error);
          addCandidates(
            result.data.map((source) => ({
              key: source.path,
              source: {
                kind: "path",
                path: source.path,
                name: source.name,
              },
              size: source.size,
              selected: true,
            })),
          );
        })
        .catch((error) => {
          console.error(
            "[audio-import] failed to inspect dropped paths:",
            error,
          );
        });
    },
  });

  const toggleCandidate = useCallback((key: string) => {
    setCandidates((current) =>
      current.map((candidate) =>
        candidate.key === key
          ? { ...candidate, selected: !candidate.selected }
          : candidate,
      ),
    );
  }, []);

  const selectedCount = candidates.filter(
    (candidate) => candidate.selected,
  ).length;

  const handleImport = useCallback(() => {
    const selected = candidates.filter((candidate) => candidate.selected);
    if (selected.length) {
      enqueue(selected.map((candidate) => candidate.source));
    }
    setCandidates([]);
    // The sidebar toast takes over as the progress surface; reopen via its
    // "View" action to see per-file status or retry failures.
    setDialogOpen(false);
  }, [candidates, enqueue, setDialogOpen]);

  const finishedCount = items.filter((item) =>
    isFinishedAudioImportStatus(item.status),
  ).length;
  const allFinished = items.length > 0 && finishedCount === items.length;

  return (
    <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
      <DialogContent ref={dropTargetRef} className="max-w-xl">
        <DialogHeader>
          <DialogTitle>
            <Trans>Import audio files</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Each file becomes its own note and is transcribed and summarized
              one at a time.
            </Trans>
          </DialogDescription>
        </DialogHeader>

        <div
          className={cn([
            "flex flex-col items-center gap-2 rounded-lg border border-dashed p-6 text-center",
            "transition-none",
            isDragActive ? "border-primary bg-accent" : "border-border",
          ])}
        >
          <p className="text-muted-foreground text-sm">
            <Trans>Drop audio files or a folder here</Trans>
          </p>
          <Button variant="outline" size="sm" onClick={handleChooseFiles}>
            <Trans>Choose files</Trans>
          </Button>
        </div>

        {candidates.length > 0 && (
          <div className="flex max-h-56 flex-col gap-1 overflow-y-auto">
            {candidates.map((candidate) => (
              <label
                key={candidate.key}
                className={cn([
                  "flex cursor-pointer items-center gap-3 rounded-md px-2 py-1.5",
                  "hover:bg-accent",
                ])}
              >
                <Checkbox
                  checked={candidate.selected}
                  onCheckedChange={() => toggleCandidate(candidate.key)}
                />
                <span className="min-w-0 flex-1 truncate text-sm">
                  {candidate.source.name}
                </span>
                {candidate.size != null && (
                  <span className="text-muted-foreground shrink-0 text-xs">
                    {formatFileSize(candidate.size)}
                  </span>
                )}
              </label>
            ))}
          </div>
        )}

        {items.length > 0 && (
          <div className="flex flex-col gap-1">
            <div className="flex items-center justify-between">
              <span className="text-muted-foreground text-xs font-medium">
                {allFinished ? (
                  <Trans>
                    Imported {finishedCount} of {items.length}
                  </Trans>
                ) : (
                  <Trans>
                    Importing {finishedCount + 1} of {items.length}
                  </Trans>
                )}
              </span>
              {allFinished && (
                <Button variant="ghost" size="sm" onClick={clearFinished}>
                  <Trans>Clear</Trans>
                </Button>
              )}
            </div>
            <div className="flex max-h-56 flex-col gap-1 overflow-y-auto">
              {items.map((item) => (
                <QueueRow key={item.id} item={item} onRetry={retryItem} />
              ))}
            </div>
            {!allFinished && (
              <p className="text-muted-foreground text-xs">
                <Trans>
                  Importing continues in the background — you can close this
                  window and we'll notify you when it's done.
                </Trans>
              </p>
            )}
          </div>
        )}

        <DialogFooter>
          {candidates.length > 0 ? (
            <Button onClick={handleImport} disabled={selectedCount === 0}>
              {selectedCount === 1 ? (
                <Trans>Import 1 file</Trans>
              ) : (
                <Trans>Import {selectedCount} files</Trans>
              )}
            </Button>
          ) : (
            <Button variant="outline" onClick={() => setDialogOpen(false)}>
              <Trans>Close</Trans>
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function QueueRow({
  item,
  onRetry,
}: {
  item: AudioImportItem;
  onRetry: (itemId: string) => void;
}) {
  return (
    <div className="flex items-center gap-3 rounded-md px-2 py-1.5">
      <span className="min-w-0 flex-1 truncate text-sm">
        {item.source.name}
      </span>
      {item.status === "pending" && (
        <span className="text-muted-foreground shrink-0 text-xs">
          <Trans>Queued</Trans>
        </span>
      )}
      {(item.status === "preparing" || item.status === "importing") && (
        <div className="flex w-32 shrink-0 items-center gap-2">
          <span className="text-muted-foreground text-xs">
            <Trans>Importing</Trans>
          </span>
          <Progress value={item.percentage} className="flex-1" />
        </div>
      )}
      {item.status === "transcribing" && (
        <div className="flex w-40 shrink-0 items-center gap-2">
          <span className="text-muted-foreground text-xs">
            <Trans>Transcribing</Trans>
          </span>
          <Progress value={item.percentage} className="flex-1" />
        </div>
      )}
      {item.status === "done" && (
        <CheckIcon className="text-muted-foreground size-4 shrink-0" />
      )}
      {item.status === "failed" && (
        <div className="flex min-w-0 shrink-0 items-center gap-2">
          <span
            className="text-destructive max-w-48 truncate text-xs"
            title={item.error ?? undefined}
          >
            {item.error}
          </span>
          <Button variant="ghost" size="sm" onClick={() => onRetry(item.id)}>
            <RotateCcwIcon className="size-3.5" />
            <Trans>Retry</Trans>
          </Button>
        </div>
      )}
      {(item.status === "preparing" ||
        item.status === "importing" ||
        item.status === "transcribing") && <Spinner size={12} />}
    </div>
  );
}

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

function formatFileSize(bytes: number) {
  if (bytes >= 1_000_000_000) {
    return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  }
  if (bytes >= 1_000_000) {
    return `${(bytes / 1_000_000).toFixed(1)} MB`;
  }
  return `${Math.max(1, Math.round(bytes / 1_000))} KB`;
}
