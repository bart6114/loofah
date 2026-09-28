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

describe("shared summary generation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.render.mockImplementation(async (input) => ({
      status: "ok",
      data: JSON.stringify(input),
    }));
    mocks.streamText.mockReturnValue({
      fullStream: (async function* () {
        yield {
          type: "text-delta",
          text: '# Decisions\n- Ship Friday\n<loofah-tags>{"tags":[{"name":"Release","confidence":0.93}]}</loofah-tags>',
        };
      })(),
    });
  });
  it.each(["", "Use a short paragraph in {{ language }}."])(
    "sends source and images separately from prompt %j",
    async (promptOverride) => {
      const controller = new AbortController();
      const onResult = vi.fn();
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
          tagContext: {
            available: ["Release"],
            attached: ["Work"],
            dismissed: ["Planning"],
          },
          expectedMarkdown: "",
          imageContext: [{ base64: "image-bytes", mimeType: "image/png" }],
        },
        onProgress: vi.fn(),
        onResult,
        signal: controller.signal,
      });
      const chunks = [];
      for await (const chunk of workflow) chunks.push(chunk);
      expect(chunks).toHaveLength(1);
      expect(chunks[0]).toMatchObject({ text: "# Decisions\n- Ship Friday\n" });
      expect(onResult).toHaveBeenLastCalledWith({
        suggestedTags: [{ name: "Release", confidence: 0.93 }],
      });
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
        tagContext: {
          available: ["Release"],
          attached: ["Work"],
          dismissed: ["Planning"],
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
      expect(request.messages[0].content[0].text).toContain(
        "Keep the summary concise and proportional",
      );
      expect(request.messages[0].content[0].text).not.toContain(
        "characters overall",
      );
      expect(request.maxOutputTokens).toBe(8192);
    },
  );
});

it("resets metadata for retries and uses only the accepted attempt", async () => {
  mocks.streamText.mockReset();
  mocks.render.mockResolvedValue({ status: "ok", data: "Prompt" });
  mocks.streamText
    .mockReturnValueOnce({
      fullStream: (async function* () {
        yield {
          type: "text-delta",
          text: 'Invalid summary structure<loofah-tags>{"tags":[{"name":"Wrong","confidence":0.91}]}</loofah-tags>',
        };
      })(),
    })
    .mockReturnValueOnce({
      fullStream: (async function* () {
        yield {
          type: "text-delta",
          text: '# Decisions\n- Ship Friday<loofah-tags>{"tags":[]}</loofah-tags>',
        };
      })(),
    });
  const onResult = vi.fn();
  const onProgress = vi.fn();
  const chunks = [];
  for await (const chunk of enhanceWorkflow.executeWorkflow({
    model: {} as LanguageModel,
    args: {
      promptOverride: "",
      imageContext: [],
      tagContext: { available: [], attached: [], dismissed: [] },
    } as any,
    signal: new AbortController().signal,
    onResult,
    onProgress,
  }))
    chunks.push(chunk);
  expect(chunks).toEqual([
    { type: "text-delta", text: "# Decisions\n- Ship Friday" },
  ]);
  expect(onResult.mock.calls).toEqual([[{}], [{}], [{ suggestedTags: [] }]]);
  expect(onProgress).toHaveBeenCalledWith(
    expect.objectContaining({ type: "retrying" }),
  );
});
