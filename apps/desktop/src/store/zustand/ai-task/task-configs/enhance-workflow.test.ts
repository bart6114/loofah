import type { LanguageModel } from "ai";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { enhanceWorkflow } from "./enhance-workflow";

const mocks = vi.hoisted(() => ({ render: vi.fn(), streamText: vi.fn() }));
vi.mock("@hypr/plugin-template", () => ({
  commands: { render: mocks.render },
}));
vi.mock("ai", () => ({
  streamText: mocks.streamText,
  smoothStream: () => undefined,
}));
vi.mock("~/store/zustand/ai-task/shared/validate", () => ({
  withEarlyValidationRetry: (
    run: (signal: AbortSignal, feedback: object) => AsyncIterable<unknown>,
  ) => run(new AbortController().signal, {}),
}));

describe("shared summary generation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.render.mockImplementation(async (input) => ({
      status: "ok",
      data: JSON.stringify(input),
    }));
    mocks.streamText.mockReturnValue({
      fullStream: (async function* () {
        yield { type: "text-delta", text: "# Decisions\n- Ship Friday" };
      })(),
    });
  });
  it.each(["", "Use a short paragraph in {{ language }}."])(
    "sends source and images separately from prompt %j",
    async (promptOverride) => {
      const controller = new AbortController();
      const workflow = enhanceWorkflow.executeWorkflow!({
        model: {} as LanguageModel,
        args: {
          language: "en",
          promptOverride,
          session: {
            title: "Pilot",
            startedAt: null,
            endedAt: null,
            event: null,
          },
          participants: [{ name: "Alice", jobTitle: null }],
          preMeetingMemo: "Discuss the pilot",
          postMeetingMemo: "Ship Friday",
          transcripts: [
            {
              segments: [
                { speaker: "Alice", text: "Check migration Thursday." },
              ],
              startedAt: null,
              endedAt: null,
            },
          ],
          imageContext: [{ base64: "image-bytes", mimeType: "image/png" }],
        },
        onProgress: vi.fn(),
        signal: controller.signal,
      });
      const chunks = [];
      for await (const chunk of workflow) chunks.push(chunk);
      expect(chunks).toHaveLength(1);
      expect(mocks.render.mock.calls[0][0]).toEqual({
        enhanceSystem: { language: "en", promptOverride },
      });
      expect(mocks.render.mock.calls[1][0].enhanceUser).toEqual({
        session: {
          title: "Pilot",
          startedAt: null,
          endedAt: null,
          event: null,
        },
        participants: [{ name: "Alice", jobTitle: null }],
        preMeetingMemo: "Discuss the pilot",
        postMeetingMemo: "Ship Friday",
        transcripts: [
          {
            segments: [{ speaker: "Alice", text: "Check migration Thursday." }],
            startedAt: null,
            endedAt: null,
          },
        ],
      });
      const request = mocks.streamText.mock.calls[0][0];
      expect(request.messages[0].content[0].text).toContain("Ship Friday");
      expect(request.messages[0].content[1]).toEqual({
        type: "image",
        image: "image-bytes",
        mediaType: "image/png",
      });
      expect(request.maxOutputTokens).toBe(8192);
    },
  );
});
