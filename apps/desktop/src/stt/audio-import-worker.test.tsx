import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { AudioImportWorker } from "./audio-import-worker";
import { deferred, mediaFixture } from "./media-test-support";

import type { VaultScope } from "~/shared/application/vault-scope";
import { useAudioImport } from "~/store/zustand/audio-import";
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isFocused: async () => true,
    isVisible: async () => true,
  }),
}));
vi.mock("@hypr/plugin-notification", () => ({
  commands: { showNotification: vi.fn() },
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: vi.fn(), warning: vi.fn(), success: vi.fn() },
}));
const scopes: VaultScope[] = [];
beforeEach(() =>
  useAudioImport.setState({
    items: [],
    activeItemId: null,
    completionAnnounced: false,
  }),
);
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.clearAllMocks();
});
async function fixture() {
  const f = await mediaFixture();
  scopes.push(f.scope);
  return f;
}
it("fresh queue import lets Core create the session and owns its original request through status", async () => {
  const f = await fixture();
  f.media.importAudio.mockResolvedValue({
    request_id: "original",
    session_id: "core-created",
    state: "running",
  });
  f.media.status.mockResolvedValue({
    session_id: "core-created",
    state: "succeeded",
  });
  useAudioImport
    .getState()
    .enqueue(
      [{ kind: "path", name: "meeting.mp3", path: "/meeting.mp3" }],
      f.scope,
    );
  const original = useAudioImport.getState().items[0].id;
  render(<AudioImportWorker />, { wrapper: f.wrapper });
  await waitFor(() =>
    expect(useAudioImport.getState().items[0].status).toBe("done"),
  );
  expect(f.media.importAudio).toHaveBeenCalledOnce();
  expect(f.media.importAudio).toHaveBeenCalledWith(
    {
      sessionId: null,
      prepared: expect.objectContaining({
        input: f.prepared.input,
        source: f.prepared.source,
      }),
      createdAt: undefined,
      contentType: undefined,
    },
    { requestId: original },
  );
  expect(useAudioImport.getState().items[0].sessionId).toBe("core-created");
  expect(f.prepared.close).toHaveBeenCalledOnce();
});
it("explicit retry recovers the original request without importing or replaying model work", async () => {
  const f = await fixture();
  useAudioImport
    .getState()
    .enqueue(
      [{ kind: "path", name: "meeting.mp3", path: "/meeting.mp3" }],
      f.scope,
    );
  const original = useAudioImport.getState().items[0].id;
  useAudioImport.getState().finishItem(original, "Review original outcome");
  useAudioImport.getState().retryItem(original);
  f.media.recover.mockResolvedValue({
    session_id: "original-note",
    state: "succeeded",
  });
  render(<AudioImportWorker />, { wrapper: f.wrapper });
  await waitFor(() =>
    expect(useAudioImport.getState().items[0].status).toBe("done"),
  );
  expect(f.media.recover).toHaveBeenCalledWith(original);
  expect(f.media.importAudio).not.toHaveBeenCalled();
  expect(f.client.prepareAttachmentSource).not.toHaveBeenCalled();
});
it("same-string vault replacement never imports an old queue item into the new owner", async () => {
  const old = await fixture();
  const replacement = await fixture();
  useAudioImport
    .getState()
    .enqueue(
      [{ kind: "path", name: "private.mp3", path: "/old/private.mp3" }],
      old.scope,
    );
  render(<AudioImportWorker />, { wrapper: replacement.wrapper });
  await waitFor(() =>
    expect(useAudioImport.getState().items[0].status).toBe("failed"),
  );
  expect(old.media.importAudio).not.toHaveBeenCalled();
  expect(replacement.media.importAudio).not.toHaveBeenCalled();
  expect(replacement.client.prepareAttachmentSource).not.toHaveBeenCalled();
});
it("scope loss during browser staging closes bytes and preserves the original queue identity", async () => {
  const f = await fixture();
  const pending = deferred<typeof f.prepared>();
  vi.mocked(f.client.prepareAttachmentSource).mockReturnValue(pending.promise);
  useAudioImport.getState().enqueue(
    [
      {
        kind: "file",
        name: "meeting.mp3",
        file: new File(["wave"], "meeting.mp3"),
      },
    ],
    f.scope,
  );
  const original = useAudioImport.getState().items[0].id;
  render(<AudioImportWorker />, { wrapper: f.wrapper });
  await waitFor(() =>
    expect(f.client.prepareAttachmentSource).toHaveBeenCalledOnce(),
  );
  act(() => f.scope.dispose());
  pending.resolve(f.prepared);
  await waitFor(() => expect(f.prepared.close).toHaveBeenCalledOnce());
  expect(f.media.importAudio).not.toHaveBeenCalled();
  expect(useAudioImport.getState().items[0].id).toBe(original);
  expect(useAudioImport.getState().items[0].status).toBe("failed");
});
