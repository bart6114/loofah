import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  copy: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("~/settings/queries", () => ({
  useStoredSettingValuesQuery: () => ({
    data: {
      values: {
        current_stt_provider: "fmtr",
        current_stt_model: "QuantizedBase",
      },
      hasValues: new Set(["current_stt_provider", "current_stt_model"]),
    },
  }),
}));
vi.mock("~/shared/config", () => ({
  useConfigValues: () => ({
    current_stt_provider: "fmtr",
    current_stt_model: "QuantizedBase",
    transcription_timing: "live",
    meeting_languages: ["en"],
  }),
}));
vi.mock("~/stt/contexts", () => ({
  useListener: (selector: (state: unknown) => unknown) =>
    selector({
      live: {
        status: "inactive",
        loading: false,
        liveTranscriptionActive: null,
        requestedLiveTranscription: null,
        transcriptionStalled: false,
        degraded: null,
      },
      batch: {},
    }),
}));
vi.mock("~/stt/useLocalSttModel", () => ({
  localSttQueries: {
    supportedModels: () => ({
      queryKey: ["supported-models"],
      queryFn: async () => [
        { key: "QuantizedBase", display_name: "Whisper Base (Multilingual)" },
      ],
    }),
    isDownloaded: () => ({
      queryKey: ["downloaded"],
      queryFn: async () => true,
    }),
    isDownloading: () => ({
      queryKey: ["downloading"],
      queryFn: async () => false,
    }),
    server: () => ({
      queryKey: ["server"],
      queryFn: async () => ({ status: "ready", model: "QuantizedBase" }),
    }),
  },
}));
vi.mock("~/stt/capabilities", () => ({
  getLiveTranscriptionConfig: async () => ({ transcriptionMode: "live" }),
  getTranscriptionLanguages: (languages: string[]) => languages,
  isFmtrLocalSttModel: () => true,
  isSupportedLanguagesBatch: async () => true,
}));
vi.mock("~/stt/useRunBatch", () => ({ getBatchProvider: () => "whispercpp" }));
vi.mock("@lingui/react", () => {
  const translate = ({
    message,
    id,
    values = {},
  }: {
    message?: string;
    id?: string;
    values?: Record<string, unknown>;
  }) =>
    (message ?? id ?? "").replace(/\{(\w+)\}/g, (_, key: string) =>
      String(values[key] ?? ""),
    );
  return {
    Trans: (props: {
      message?: string;
      id?: string;
      values?: Record<string, unknown>;
      children?: ReactNode;
    }) => <>{props.children ?? translate(props)}</>,
    useLingui: () => ({
      _: translate,
      i18n: {
        _: translate,
        locale: "en",
        number: (value: number) => value.toLocaleString(),
        date: (value: Date) => value.toISOString(),
      },
    }),
  };
});

import { DebugSection } from "./about-debug";

const device = {
  appVersion: "0.41.4",
  buildHash: "abc123",
  identifier: "io.loofah.dev",
  platform: "macos",
  arch: "aarch64",
  osVersion: "macOS 15.6",
  locale: "en",
};

function setup(info: typeof device | undefined, deviceError = false) {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: mocks.copy },
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <DebugSection device={info} deviceError={deviceError} />
    </QueryClientProvider>,
  );
  return client;
}

afterEach(() => {
  cleanup();
  mocks.copy.mockReset().mockResolvedValue(undefined);
});

describe("About diagnostics", () => {
  it("copies exactly the preview and confirms success", async () => {
    const client = setup(device);
    await waitFor(() =>
      expect(screen.getByLabelText("Debug information").textContent).toContain(
        "Batch target: whispercpp / QuantizedBase",
      ),
    );
    await waitFor(() =>
      expect(screen.getByLabelText("Debug information").textContent).toContain(
        "Whisper Base (Multilingual) [QuantizedBase]",
      ),
    );
    const preview = screen.getByLabelText("Debug information").textContent;
    fireEvent.click(
      screen.getByRole("button", { name: "Copy debug information" }),
    );
    await waitFor(() => expect(mocks.copy).toHaveBeenCalledWith(preview));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeTruthy();
    client.clear();
  });

  it("keeps the preview available when copying fails", async () => {
    mocks.copy.mockRejectedValueOnce(new Error("Clipboard unavailable"));
    const client = setup(device);
    fireEvent.click(
      screen.getByRole("button", { name: "Copy debug information" }),
    );
    expect((await screen.findByRole("alert")).textContent).toContain(
      "Couldn’t copy debug information",
    );
    expect(screen.getByLabelText("Debug information")).toBeTruthy();
    client.clear();
  });

  it("disables copying and reports a device information failure", () => {
    const client = setup(undefined, true);
    expect(
      (
        screen.getByRole("button", {
          name: "Copy debug information",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    expect(screen.getByRole("status").textContent).toContain(
      "Couldn’t load debug information",
    );
    client.clear();
  });
});
