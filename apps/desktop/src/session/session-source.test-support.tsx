import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { vi } from "vitest";

import type {
  MetadataSnapshot,
  NoteEditorSnapshot,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";

export function sessionMetadata(
  title = "Original title",
  id = "session-1",
  revision = "m0",
): MetadataSnapshot {
  return {
    meta: {
      id,
      title,
      created_at: "2026-11-08T10:00:00Z",
      started_at: null,
      ended_at: null,
      tags: ["keep"],
      author: "writer",
      folder: "folder",
    },
    revision,
  };
}
export function sessionNote(
  markdown: string | null = "Original body",
  title = "Original title",
  revision = "n0",
): NoteEditorSnapshot {
  return {
    note: { markdown, title, revision },
    tasks: { tasks: [], revision: "t0" },
  };
}
export function createSessionSourceHost(
  mode: "always" | "online" = "always",
  overrides: Partial<VaultClient> = {},
) {
  let metadata = sessionMetadata();
  let note = sessionNote();
  let sequence = 0;
  const listeners = new Set<(event: VaultClientEvent) => void>();
  const readMetadata = vi.fn<VaultClient["readMetadata"]>(async () =>
    structuredClone(metadata),
  );
  const readNoteEditor = vi.fn<VaultClient["readNoteEditor"]>(async () =>
    structuredClone(note),
  );
  const saveMetadata = vi.fn<VaultClient["saveMetadata"]>(
    async (_id, input) => {
      if (input.expected_revision !== metadata.revision)
        return { kind: "conflict", snapshot: structuredClone(metadata) };
      metadata = {
        meta: {
          ...metadata.meta,
          title: input.patch.title ?? metadata.meta.title,
        },
        revision: `m${++sequence}`,
      };
      note = {
        ...note,
        note: {
          ...note.note,
          title: metadata.meta.title,
          revision: `n${sequence}`,
        },
      };
      return { kind: "saved", snapshot: structuredClone(metadata) };
    },
  );
  const client = {
    scope: crypto.randomUUID(),
    networkMode: mode,
    canEdit: true,
    features: ["content_read", "note_editor_read", "metadata_edit"],
    readMetadata,
    readNoteEditor,
    saveMetadata,
    subscribe: async (listener: (event: VaultClientEvent) => void) => {
      listeners.add(listener);
      listener({ kind: "connection", state: "connected", error: null });
      return () => {
        listeners.delete(listener);
      };
    },
    ...overrides,
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scope.queryClient.setDefaultOptions({
    queries: { ...scope.queryClient.getDefaultOptions().queries, retry: false },
  });
  const rootClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  void scope.start();
  function Host({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={rootClient}>
        <VaultHostContext.Provider
          value={{ scope, error: null, retry: () => {} }}
        >
          {children}
        </VaultHostContext.Provider>
      </QueryClientProvider>
    );
  }
  return {
    scope,
    client,
    Host,
    rootClient,
    readMetadata,
    readNoteEditor,
    saveMetadata,
    metadata: () => metadata,
    setMetadata: (value: MetadataSnapshot) => {
      metadata = value;
    },
    setNote: (value: NoteEditorSnapshot) => {
      note = value;
    },
    emit: (event: VaultClientEvent) => {
      for (const listener of listeners) listener(event);
    },
    dispose: () => {
      scope.dispose();
      rootClient.clear();
    },
  };
}
