import { describe, expect, test } from "vitest";

import {
  getDefaultSttModel,
  getDefaultSttSelection,
  getLanguageSupportIssue,
  getPreferredProviderModel,
  resolveLiveLanguageSupportMode,
} from "./selection";

describe("getDefaultSttModel", () => {
  test("never invents a model — STT is on-device only, model comes from local discovery", () => {
    expect(getDefaultSttModel("fmtr")).toBeUndefined();
    expect(getDefaultSttModel(undefined)).toBeUndefined();
  });
});

describe("getPreferredProviderModel", () => {
  test("returns the remembered model when it is still available", () => {
    expect(
      getPreferredProviderModel("soniqo-parakeet-batch", [
        { id: "soniqo-parakeet-streaming" },
        { id: "soniqo-parakeet-batch" },
      ]),
    ).toBe("soniqo-parakeet-batch");
  });

  test("falls back to the first available model when none is remembered", () => {
    expect(
      getPreferredProviderModel(undefined, [
        { id: "soniqo-parakeet-streaming" },
        { id: "soniqo-parakeet-batch" },
      ]),
    ).toBe("soniqo-parakeet-streaming");
  });

  test("falls back to the first available model when the remembered model is gone", () => {
    expect(
      getPreferredProviderModel("soniqo-omnilingual", [
        { id: "soniqo-parakeet-streaming" },
        { id: "soniqo-parakeet-batch" },
      ]),
    ).toBe("soniqo-parakeet-streaming");
  });

  test("skips models that are not selectable", () => {
    expect(
      getPreferredProviderModel(undefined, [
        { id: "soniqo-omnilingual", isDownloaded: false },
        { id: "soniqo-qwen3-small", isDownloaded: true },
      ]),
    ).toBe("soniqo-qwen3-small");
  });

  test("can keep a saved model visible even when it is not selectable", () => {
    expect(
      getPreferredProviderModel(
        "soniqo-omnilingual",
        [
          { id: "soniqo-omnilingual", isDownloaded: false },
          { id: "soniqo-parakeet-streaming", isDownloaded: true },
        ],
        { keepUnavailableSavedModel: true },
      ),
    ).toBe("soniqo-omnilingual");
  });

  test("clears the selection when a provider has no selectable models", () => {
    expect(
      getPreferredProviderModel("soniqo-omnilingual", [
        { id: "soniqo-omnilingual", isDownloaded: false },
      ]),
    ).toBe("");
  });

  test("keeps the remembered value when the provider does not expose a static list", () => {
    expect(
      getPreferredProviderModel("some-saved-model", [], {
        allowSavedModelWithoutChoices: true,
      }),
    ).toBe("some-saved-model");
  });
});

describe("getDefaultSttSelection", () => {
  test("keeps the active configured provider and repairs its missing model", () => {
    expect(
      getDefaultSttSelection(
        ["fmtr"],
        {
          fmtr: {
            configured: true,
            models: [{ id: "soniqo-parakeet-batch" }],
          },
        },
        "fmtr",
      ),
    ).toEqual({ provider: "fmtr", model: "soniqo-parakeet-batch" });
  });

  test("skips configured providers that have no available model", () => {
    expect(
      getDefaultSttSelection(["fmtr", "other"], {
        fmtr: {
          configured: true,
          models: [{ id: "soniqo-omnilingual", isDownloaded: false }],
        },
        other: {
          configured: true,
          models: [{ id: "some-model" }],
        },
      }),
    ).toEqual({ provider: "other", model: "some-model" });
  });

  test("returns no selection when nothing is available", () => {
    expect(
      getDefaultSttSelection(["fmtr"], {
        fmtr: {
          configured: true,
          models: [{ id: "soniqo-omnilingual", isDownloaded: false }],
        },
      }),
    ).toBeNull();
  });
});

describe("getLanguageSupportIssue", () => {
  test("returns the languages the model cannot transcribe", async () => {
    const issue = await getLanguageSupportIssue(
      ["en", "ko", "ja"],
      async (languages) => !languages.includes("ko"),
    );

    expect(issue).toEqual({ unsupportedLanguages: ["ko"] });
  });

  test("distinguishes an unsupported combination from unsupported languages", async () => {
    const issue = await getLanguageSupportIssue(
      ["en", "ko"],
      async (languages) => languages.length === 1,
    );

    expect(issue).toEqual({ unsupportedLanguages: [] });
  });

  test("returns no issue when the full selection is supported", async () => {
    const issue = await getLanguageSupportIssue(["en", "ko"], async () => true);

    expect(issue).toBeNull();
  });
});

describe("resolveLiveLanguageSupportMode", () => {
  test("uses provider live support for hosted models", () => {
    expect(
      resolveLiveLanguageSupportMode({
        isOnDeviceModel: false,
        useLiveOnDeviceModel: false,
        liveSupported: true,
      }),
    ).toBe(true);
  });

  test("keeps batch-only on-device models in batch mode", () => {
    expect(
      resolveLiveLanguageSupportMode({
        isOnDeviceModel: true,
        useLiveOnDeviceModel: false,
        liveSupported: true,
      }),
    ).toBe(false);
  });

  test("requires provider live support for realtime on-device models", () => {
    expect(
      resolveLiveLanguageSupportMode({
        isOnDeviceModel: true,
        useLiveOnDeviceModel: true,
        liveSupported: false,
      }),
    ).toBe(false);
  });
});

test("resolves a saved Parakeet choice to the matching platform model", () => {
  expect(
    getPreferredProviderModel("soniqo-parakeet-batch", [
      { id: "onnx-parakeet-streaming", isDownloaded: true },
      { id: "onnx-parakeet-batch", isDownloaded: true },
    ]),
  ).toBe("onnx-parakeet-batch");
  expect(
    getPreferredProviderModel(
      "onnx-parakeet-streaming",
      [
        { id: "soniqo-parakeet-batch", isDownloaded: true },
        { id: "soniqo-parakeet-streaming", isDownloaded: false },
      ],
      { keepUnavailableSavedModel: true },
    ),
  ).toBe("soniqo-parakeet-streaming");
});
