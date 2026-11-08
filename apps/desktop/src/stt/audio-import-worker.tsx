import { useMutation, useQuery } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";

import { commands as notificationCommands } from "@hypr/plugin-notification";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY } from "./audio-import-notification";

import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { mediaPending } from "~/shared/application/vault-client";
import type { VaultScope } from "~/shared/application/vault-scope";
import {
  type AudioImportItem,
  isFinishedAudioImportStatus,
  useAudioImport,
} from "~/store/zustand/audio-import";

const startedSteps = new WeakMap<VaultScope, Set<string>>();
export function AudioImportWorker() {
  const owner = useVaultReadScope();
  const activeItem = useAudioImport(
    (state) =>
      state.items.find((item) => item.id === state.activeItemId) ?? null,
  );
  const hasPending = useAudioImport((state) =>
    state.items.some((item) => item.status === "pending"),
  );
  const announce = useAudioImport(
    (state) =>
      !state.completionAnnounced &&
      state.activeItemId === null &&
      state.items.length > 0 &&
      state.items.every((item) => isFinishedAudioImportStatus(item.status)),
  );
  useEffect(() => {
    if (!activeItem && hasPending) useAudioImport.getState().claimNext();
  }, [activeItem, hasPending]);
  useEffect(() => {
    if (!announce) return;
    useAudioImport.getState().markCompletionAnnounced();
    const items = useAudioImport
      .getState()
      .items.filter((item) => item.owner === owner.scope);
    if (!items.length || !owner.current()) return;
    const done = items.filter((item) => item.status === "done").length;
    const failed = items.filter((item) => item.status === "failed").length;
    const last =
      [...items].reverse().find((item) => item.status === "done")?.sessionId ??
      null;
    void announceImportCompleted(done, failed, last);
  }, [announce, owner.current, owner.scope]);
  return activeItem ? (
    <ItemPipeline
      key={`${activeItem.id}:${activeItem.attempt}`}
      item={activeItem}
    />
  ) : null;
}
function ItemPipeline({ item }: { item: AudioImportItem }) {
  const owner = useVaultReadScope();
  const check = () => {
    if (
      !owner.current() ||
      owner.scope !== item.owner ||
      owner.scope.getSnapshot().state !== "connected" ||
      !owner.scope.client.media
    )
      throw new Error(
        "The original vault is no longer available. Open its media status to recover this request.",
      );
  };
  const mutation = useMutation({
    retry: false,
    networkMode: "always",
    mutationFn: async () => {
      check();
      const media = owner.scope.client.media!;
      if (item.attempt > 0) {
        const status = await owner.scope.run(() => media.recover(item.id));
        if (status.state !== "receiving" || !media.resumeSource) return status;
        check();
        const prepared = await owner.scope.prepareAttachmentSource(item.source);
        try {
          check();
          return await owner.scope.run(() =>
            media.resumeSource!(item.id, prepared),
          );
        } finally {
          await prepared.close();
        }
      }
      const source = item.source;
      const prepared = await owner.scope.prepareAttachmentSource(
        source.kind === "path"
          ? { kind: "path", path: source.path, name: source.name }
          : { kind: "file", file: source.file, name: source.name },
      );
      try {
        check();
        useAudioImport.getState().setItemStatus(item.id, "importing");
        const createdAt =
          source.kind === "file" &&
          Number.isFinite(source.file.lastModified) &&
          source.file.lastModified > 0
            ? new Date(source.file.lastModified).toISOString()
            : undefined;
        return await owner.scope.run(() =>
          media.importAudio(
            {
              sessionId: null,
              prepared,
              createdAt,
              contentType:
                source.kind === "file" ? source.file.type : undefined,
            },
            { requestId: item.id },
          ),
        );
      } finally {
        await prepared.close();
      }
    },
    onSuccess: (status) => {
      if (!owner.current() || owner.scope !== item.owner) {
        useAudioImport
          .getState()
          .finishItem(
            item.id,
            "The original vault is no longer available. Recover this request from its media status.",
          );
        return;
      }
      useAudioImport.getState().setItemSession(item.id, status.session_id);
      if (mediaPending(status))
        useAudioImport.getState().setItemStatus(item.id, "transcribing");
      else
        useAudioImport
          .getState()
          .finishItem(
            item.id,
            status.state === "succeeded"
              ? undefined
              : (status.error?.message ?? "Media processing requires review."),
          );
    },
    onError: (error) =>
      useAudioImport
        .getState()
        .finishItem(
          item.id,
          error instanceof Error ? error.message : String(error),
        ),
  });
  useEffect(() => {
    if (!owner.current() || owner.scope !== item.owner) {
      useAudioImport
        .getState()
        .finishItem(
          item.id,
          "The original vault is no longer available. Recover this request from its media status.",
        );
    }
  }, [owner.current, owner.scope, owner.connection.state, item.id, item.owner]);
  const mutate = mutation.mutate;
  useEffect(() => {
    let steps = startedSteps.get(item.owner);
    if (!steps) {
      steps = new Set();
      startedSteps.set(item.owner, steps);
    }
    const key = `${item.id}:${item.attempt}`;
    if (steps.has(key)) return;
    steps.add(key);
    mutate();
  }, [item.owner, item.id, item.attempt, mutate]);
  const status = useQuery(
    {
      queryKey: ["vault", owner.scope.client.scope, "media-import", item.id],
      queryFn: ({ signal }) => {
        check();
        return owner.scope.run(() =>
          owner.scope.client.media!.status(item.id, { signal }),
        );
      },
      enabled:
        owner.scope === item.owner &&
        owner.current() &&
        !!owner.scope.client.media &&
        !!mutation.data &&
        mediaPending(mutation.data),
      retry: false,
      networkMode: "always",
      refetchInterval: (query) =>
        !query.state.data || mediaPending(query.state.data) ? 500 : false,
    },
    owner.scope.queryClient,
  );
  useEffect(() => {
    if (!status.data || mediaPending(status.data)) return;
    useAudioImport
      .getState()
      .finishItem(
        item.id,
        status.data.state === "succeeded"
          ? undefined
          : (status.data.error?.message ?? "Media processing requires review."),
      );
  }, [status.data, item.id]);
  useEffect(() => {
    if (status.error)
      useAudioImport
        .getState()
        .finishItem(
          item.id,
          status.error instanceof Error
            ? status.error.message
            : String(status.error),
        );
  }, [status.error, item.id]);
  return null;
}

async function announceImportCompleted(
  done: number,
  failed: number,
  lastImportedSessionId: string | null,
) {
  const total = done + failed;
  const message =
    failed > 0
      ? `Imported ${done} of ${total} files. ${failed} failed.`
      : done === 1
        ? "Imported 1 file."
        : `Imported ${done} files.`;

  if (await isMainWindowVisibleAndFocused()) {
    if (failed > 0) {
      sonnerToast.warning("Audio import finished", { description: message });
    } else {
      sonnerToast.success("Audio import finished", { description: message });
    }
    return;
  }

  try {
    // A session source makes the notification click open that note; without
    // one (nothing succeeded) the click reopens the import dialog instead.
    const result = await notificationCommands.showNotification({
      key: AUDIO_IMPORT_COMPLETED_NOTIFICATION_KEY,
      title: "Audio import finished",
      message,
      timeout: null,
      source: lastImportedSessionId
        ? { type: "session", session_id: lastImportedSessionId }
        : null,
      start_time: null,
      participants: null,
      action_label: lastImportedSessionId ? "Open note" : "Open",
      action_variant: null,
      options: null,
      footer: null,
      icon: null,
    });
    if (result.status === "error") {
      console.error(
        "[audio-import] failed to show completion notification",
        result.error,
      );
    }
  } catch (error) {
    console.error(
      "[audio-import] failed to show completion notification",
      error,
    );
  }
}

async function isMainWindowVisibleAndFocused() {
  try {
    const window = getCurrentWindow();
    const [focused, visible] = await Promise.all([
      window.isFocused(),
      window.isVisible(),
    ]);
    return focused && visible;
  } catch (error) {
    console.error("[audio-import] failed to inspect window state", error);
    return false;
  }
}
