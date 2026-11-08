import type { DeviceRecordingInfo } from "~/types/tauri.gen";

export function deviceCaptionPreview(item: DeviceRecordingInfo | null) {
  const processing = item?.processing;
  const preview = processing?.captions;
  const status = processing?.status;
  if (
    !item ||
    !processing ||
    !preview ||
    !status ||
    !["connected", "reconnecting"].includes(preview.connection) ||
    preview.binding.capture_id !== item.identity.capture_id ||
    status.capture_id !== item.identity.capture_id ||
    preview.binding.capture_fingerprint !==
      processing.declaration.request.capture_fingerprint ||
    status.capture_fingerprint !== preview.binding.capture_fingerprint ||
    status.live_generation_id !== preview.binding.generation_id ||
    status.session_id !== preview.binding.session_id ||
    JSON.stringify(preview.binding.format) !== JSON.stringify(processing.format)
  )
    return null;
  const seen = new Set<string>();
  const words = [
    ...preview.words.slice(-128).map((word) => ({ word, final: true })),
    ...preview.partials.slice(-64).map((word) => ({ word, final: false })),
  ]
    .filter(({ word, final }) => {
      const key = `${final}:${word.id ?? `${word.channel}:${word.start_ms}:${word.end_ms}:${word.text}`}`;
      if (
        seen.has(key) ||
        !word.text.trim() ||
        word.text.length > 16384 ||
        !Number.isFinite(word.start_ms) ||
        !Number.isFinite(word.end_ms) ||
        word.end_ms < word.start_ms
      )
        return false;
      seen.add(key);
      return true;
    })
    .sort(
      (a, b) =>
        a.word.start_ms - b.word.start_ms || a.word.channel - b.word.channel,
    );
  let bytes = 0;
  const retained = words
    .reverse()
    .filter(
      ({ word }) =>
        (bytes +=
          new TextEncoder().encode(word.text).byteLength +
          (bytes > 0 ? 1 : 0)) <= 16384,
    )
    .reverse();
  return {
    generation: preview.binding.generation_id,
    interrupted: preview.connection === "reconnecting",
    text: retained.map(({ word }) => word.text).join(" "),
    bubbles: retained.map(({ word, final }, index) => ({
      id: `${preview.binding.generation_id}:${word.id ?? `${word.channel}:${word.start_ms}:${index}`}:${final}`,
      speakerLabel:
        word.role === "microphone"
          ? "Microphone"
          : word.role === "system_audio"
            ? "System audio"
            : "Mixed audio",
      text: word.text,
      isSelf: word.role === "microphone",
      isFinal: final,
      startMs: word.start_ms,
      endMs: word.end_ms,
      overlapsPrevious: false,
      overlapsNext: false,
    })),
  };
}
