import { expect, it } from "vitest";

import { deviceCaptionPreview } from "./device-caption-preview";

import type { DeviceRecordingInfo } from "~/types/tauri.gen";

export function captionItem() {
  const format = {
    sample_rate: 16000,
    channels: ["microphone", "system_audio"],
    encoding: "pcm_s16le",
  };
  return {
    profile_id: "profile",
    identity: {
      scope: {
        server: "https://server/",
        member_id: "member",
        vault_id: "vault",
      },
      capture_id: "first",
      request_id: "first",
      target: { kind: "pending_session", create_request_id: "first" },
    },
    phase: "capturing",
    failure: null,
    processing: {
      format,
      declaration: { request: { capture_fingerprint: "a".repeat(64) } },
      status: {
        capture_id: "first",
        capture_fingerprint: "a".repeat(64),
        live_generation_id: "generation",
        session_id: "original",
      },
      captions: {
        binding: {
          capture_id: "first",
          capture_fingerprint: "a".repeat(64),
          generation_id: "generation",
          session_id: "original",
          format,
        },
        connection: "connected",
        words: [
          {
            id: "word1",
            text: "Original speech",
            start_ms: 0,
            end_ms: 1000,
            channel: 0,
            role: "microphone",
          },
        ],
        partials: [],
      },
    },
  } as unknown as DeviceRecordingInfo;
}
it("keeps bounded original captions across reconnect but clears on scope retirement or identity mismatch", () => {
  const original = captionItem();
  expect(deviceCaptionPreview(original)?.text).toBe("Original speech");
  original.processing!.captions!.connection = "reconnecting";
  expect(deviceCaptionPreview(original)).toMatchObject({
    text: "Original speech",
    interrupted: true,
  });
  for (const state of [
    "closed",
    "access_denied",
    "login_required",
    "incompatible",
  ] as const) {
    const changed = structuredClone(original);
    changed.processing!.captions!.connection = state;
    expect(deviceCaptionPreview(changed)).toBeNull();
  }
  for (const field of [
    "capture_id",
    "capture_fingerprint",
    "generation_id",
    "session_id",
  ] as const) {
    const changed = structuredClone(original);
    changed.processing!.captions!.binding[field] = "replacement";
    expect(deviceCaptionPreview(changed)).toBeNull();
  }
  const changed = structuredClone(original);
  changed.processing!.captions!.binding.format = {
    ...changed.processing!.format,
    sample_rate: 8000,
  } as any;
  expect(deviceCaptionPreview(changed)).toBeNull();
});
it("deduplicates retained words and enforces the existing preview text budget", () => {
  const original = captionItem();
  const preview = original.processing!.captions!;
  preview.words.push({ ...preview.words[0] });
  expect(deviceCaptionPreview(original)?.bubbles).toHaveLength(1);
  preview.words = Array.from({ length: 200 }, (_, index) => ({
    ...preview.words[0],
    id: String(index),
    text: "x".repeat(1000),
    start_ms: index,
    end_ms: index + 1,
  }));
  expect(
    new TextEncoder().encode(deviceCaptionPreview(original)!.text).byteLength,
  ).toBeLessThanOrEqual(16384);
});
