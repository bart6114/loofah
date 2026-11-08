import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { deferred, mediaFixture } from "./media-test-support";
import { isAudioUploadFile, useUploadFile } from "./useUploadFile";

import type { VaultScope } from "~/shared/application/vault-scope";
const toast = vi.hoisted(() => vi.fn());
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: toast },
}));
vi.mock("@tauri-apps/api/path", () => ({
  downloadDir: vi.fn(async () => "/downloads"),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.clearAllMocks();
});
async function fixture() {
  const result = await mediaFixture();
  scopes.push(result.scope);
  return result;
}
it.each([
  ["voice.qta", ""],
  ["recording", "audio/webm"],
  ["meeting.wav", ""],
])("preserves supported audio transfer %s", (name, type) => {
  expect(isAudioUploadFile({ name, type })).toBe(true);
});
it("uses owned bounded browser staging and immutable original request without frontend persistence", async () => {
  const f = await fixture();
  f.media.importAudio.mockResolvedValue({
    state: "running",
    request_id: "accepted",
    session_id: "note",
  });
  const file = new File(["wave"], "voice.qta", {
    type: "audio/quicktime",
    lastModified: 123456,
  });
  const { result } = renderHook(() => useUploadFile("note"), {
    wrapper: f.wrapper,
  });
  await act(async () => {
    await result.current.processAudioFile(file);
  });
  expect(f.client.prepareAttachmentSource).toHaveBeenCalledWith(
    { kind: "file", file, name: "voice.qta" },
    expect.any(Object),
  );
  expect(f.media.importAudio).toHaveBeenCalledWith(
    {
      sessionId: "note",
      prepared: expect.objectContaining({
        input: f.prepared.input,
        source: f.prepared.source,
      }),
      createdAt: new Date(123456).toISOString(),
      contentType: "audio/quicktime",
    },
    { requestId: expect.stringMatching(/^[0-9a-f-]{36}$/) },
  );
  expect(f.prepared.close).toHaveBeenCalledOnce();
});
it("sends subtitle parsing and checked publication to the same scoped Core adapter", async () => {
  const f = await fixture();
  const { result } = renderHook(() => useUploadFile("note"), {
    wrapper: f.wrapper,
  });
  await act(async () => {
    await result.current.processFile("/private/transcript.srt", "transcript");
  });
  expect(f.media.importSubtitles).toHaveBeenCalledWith(
    "note",
    expect.objectContaining({
      input: f.prepared.input,
      source: f.prepared.source,
    }),
    {
      requestId: expect.any(String),
    },
  );
  expect(f.media.importAudio).not.toHaveBeenCalled();
});
it("retired scope during staging closes input and never targets a new vault", async () => {
  const f = await fixture();
  const held = deferred<typeof f.prepared>();
  vi.mocked(f.client.prepareAttachmentSource).mockReturnValue(held.promise);
  const { result } = renderHook(() => useUploadFile("note"), {
    wrapper: f.wrapper,
  });
  let pending: Promise<unknown> | undefined;
  act(() => {
    pending = result.current.processAudioPath("/old/source.mp3");
  });
  const rejection = expect(pending).rejects.toThrow();
  await waitFor(() =>
    expect(f.client.prepareAttachmentSource).toHaveBeenCalledOnce(),
  );
  act(() => f.scope.dispose());
  held.resolve(f.prepared);
  await act(async () => {
    await rejection;
  });
  expect(f.media.importAudio).not.toHaveBeenCalled();
  expect(f.prepared.close).toHaveBeenCalledOnce();
});
it("does not probe device sources when media capability is absent", async () => {
  const f = await fixture();
  f.client.media = undefined;
  const { result } = renderHook(() => useUploadFile("note"), {
    wrapper: f.wrapper,
  });
  expect(() => result.current.processAudioPath("/private/source.mp3")).toThrow(
    "unavailable",
  );
  expect(f.client.prepareAttachmentSource).not.toHaveBeenCalled();
});
it("does not retry an unknown admitted write", async () => {
  const f = await fixture();
  f.media.importAudio.mockRejectedValue(new Error("Unknown original result"));
  const { result } = renderHook(() => useUploadFile("note"), {
    wrapper: f.wrapper,
  });
  await act(async () => {
    await expect(
      result.current.processAudioPath("/source.mp3"),
    ).rejects.toThrow("Unknown");
  });
  expect(f.media.importAudio).toHaveBeenCalledOnce();
  expect(f.prepared.close).toHaveBeenCalledOnce();
});
