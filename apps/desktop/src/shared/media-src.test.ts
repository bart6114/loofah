import { expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset:${path}`,
}));
import { mediaSrc } from "./media-src";

it("preserves remote media URLs and resolves local files", () => {
  expect(mediaSrc("https://company.example/v1/media/ticket")).toBe(
    "https://company.example/v1/media/ticket",
  );
  expect(mediaSrc("/vault/audio.mp3")).toBe("asset:/vault/audio.mp3");
});
