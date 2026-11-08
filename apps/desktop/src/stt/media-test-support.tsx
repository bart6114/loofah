import { QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { vi } from "vitest";

import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";

export async function mediaFixture() {
  const media = {
    source: vi.fn(),
    importAudio: vi.fn(),
    importSubtitles: vi.fn(),
    retranscribe: vi.fn(),
    list: vi.fn(),
    status: vi.fn(),
    recover: vi.fn(),
    cancel: vi.fn(),
    summaryStatus: vi.fn(),
  };
  const prepared = {
    input: { filename: "meeting.mp3", size_bytes: 4, sha256: "a".repeat(64) },
    source: { kind: "staged" as const, id: "source" },
    close: vi.fn(async () => {}),
  };
  let receive!: (event: VaultClientEvent) => void;
  const client = {
    scope: "same-scope",
    networkMode: "always",
    canEdit: true,
    features: ["attachment_upload"],
    media,
    prepareAttachmentSource: vi.fn(async () => prepared),
    transcripts: vi.fn(async () => []),
    close: vi.fn(async () => {}),
    subscribe: vi.fn(async (callback: typeof receive) => {
      receive = callback;
      receive({ kind: "connection", state: "connected", error: null });
      return () => {};
    }),
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  await scope.start();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={scope.queryClient}>
      <VaultHostContext.Provider
        value={{ scope, error: null, retry: () => {} }}
      >
        {children}
      </VaultHostContext.Provider>
    </QueryClientProvider>
  );
  return {
    scope,
    client,
    media,
    prepared,
    wrapper,
    receive: (event: VaultClientEvent) => receive(event),
  };
}
export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
