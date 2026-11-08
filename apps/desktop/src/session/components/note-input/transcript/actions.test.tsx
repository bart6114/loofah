import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { useRegenerateTranscript } from "./actions";

import type { VaultScope } from "~/shared/application/vault-scope";
import { deferred, mediaFixture } from "~/stt/media-test-support";
const mocks = vi.hoisted(() => ({ confirm: vi.fn(), toast: vi.fn() }));
vi.mock("./regenerate-confirm", () => ({
  confirmRegenerateSpeakerReset: mocks.confirm,
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.toast },
}));
const scopes: VaultScope[] = [];
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.clearAllMocks();
});
async function fixture() {
  const f = await mediaFixture();
  scopes.push(f.scope);
  f.media.source.mockResolvedValue({
    session_id: "note",
    session_created_at: "original",
    recording_sha256: "a".repeat(64),
    transcript_revision: "b".repeat(64),
    transcript_present: true,
    audio: { layout: "mixed" },
  });
  return f;
}
it("submits the exact source snapshot captured before speaker confirmation", async () => {
  const f = await fixture();
  const expected = await f.media.source();
  f.media.source.mockClear();
  vi.mocked(f.client.transcripts).mockResolvedValue([
    {
      id: "transcript",
      session_id: "note",
      speaker_hints: [
        { id: "hint", word_id: "word", type: "speaker_label", value: "Alice" },
      ],
    },
  ] as never);
  mocks.confirm.mockResolvedValue(true);
  const { result } = renderHook(() => useRegenerateTranscript("note"), {
    wrapper: f.wrapper,
  });
  await act(async () => {
    await result.current();
  });
  expect(mocks.confirm).toHaveBeenCalledWith(1);
  expect(f.media.retranscribe).toHaveBeenCalledWith(expected, {
    requestId: expect.any(String),
  });
});
it.each(["source", "speakers", "confirmation"])(
  "does not submit after the %s await loses its original scope",
  async (boundary) => {
    const f = await fixture();
    const expected = await f.media.source();
    f.media.source.mockClear();
    const held = deferred<void>();
    vi.mocked(f.client.transcripts).mockResolvedValue([
      {
        id: "t",
        speaker_hints: [
          { id: "h", word_id: "w", type: "speaker_label", value: "Alice" },
        ],
      },
    ] as never);
    if (boundary === "source")
      f.media.source.mockImplementation(async () => {
        await held.promise;
        return expected;
      });
    if (boundary === "speakers")
      vi.mocked(f.client.transcripts).mockImplementation(async () => {
        await held.promise;
        return [];
      });
    if (boundary === "confirmation")
      mocks.confirm.mockImplementation(async () => {
        await held.promise;
        return true;
      });
    const { result } = renderHook(() => useRegenerateTranscript("note"), {
      wrapper: f.wrapper,
    });
    let pending!: Promise<unknown>;
    act(() => {
      pending = result.current();
    });
    const rejected = expect(pending).rejects.toThrow();
    await waitFor(() =>
      expect(
        boundary === "source"
          ? f.media.source
          : boundary === "speakers"
            ? f.client.transcripts
            : mocks.confirm,
      ).toHaveBeenCalledOnce(),
    );
    act(() => f.scope.dispose());
    held.resolve();
    await act(async () => {
      await rejected;
    });
    expect(f.media.retranscribe).not.toHaveBeenCalled();
  },
);
it("declining speaker reset preserves existing transcript and does not admit model work", async () => {
  const f = await fixture();
  vi.mocked(f.client.transcripts).mockResolvedValue([
    {
      id: "t",
      speaker_hints: [
        { id: "h", word_id: "w", type: "speaker_label", value: "Alice" },
      ],
    },
  ] as never);
  mocks.confirm.mockResolvedValue(false);
  const { result } = renderHook(() => useRegenerateTranscript("note"), {
    wrapper: f.wrapper,
  });
  await act(async () => {
    await result.current();
  });
  expect(f.media.retranscribe).not.toHaveBeenCalled();
});
