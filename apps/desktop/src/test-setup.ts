import { i18n } from "@lingui/core";
import { randomUUID } from "node:crypto";
import * as React from "react";
import { vi } from "vitest";

// Compiled @lingui/core/macro calls hit the real global i18n; give it a
// locale so untranslated messages fall back to their English source.
i18n.load("en", {});
i18n.activate("en");

Object.defineProperty(globalThis.crypto, "randomUUID", { value: randomUUID });

Object.defineProperty(globalThis.window, "__TAURI_OS_PLUGIN_INTERNALS__", {
  value: { platform: "macos", arch: "aarch64", eol: "\n" },
  writable: true,
  configurable: true,
});

Object.defineProperty(globalThis.window, "__TAURI_INTERNALS__", {
  value: {
    metadata: {
      currentWindow: {
        label: "main",
      },
      currentWebview: {
        label: "main",
      },
    },
    transformCallback: vi.fn((callback: unknown) => {
      const callbackId = Math.trunc(Math.random() * Number.MAX_SAFE_INTEGER);
      Object.assign(globalThis.window, {
        [`_${callbackId}`]: callback,
      });

      return callbackId;
    }),
    unregisterCallback: vi.fn((callbackId: number) => {
      delete (globalThis.window as unknown as Record<string, unknown>)[
        `_${callbackId}`
      ];
    }),
    invoke: vi.fn((command: string) =>
      Promise.resolve(command === "plugin:event|listen" ? 0 : null),
    ),
  },
  writable: true,
  configurable: true,
});

Object.defineProperty(globalThis.window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
  value: {
    unregisterListener: vi.fn(),
  },
  writable: true,
  configurable: true,
});

vi.mock("@tauri-apps/api/path", () => ({
  resolveResource: vi.fn((path: string) =>
    Promise.resolve(`/resources/${path}`),
  ),
  sep: vi.fn().mockReturnValue("/"),
}));

function translate(
  input:
    | TemplateStringsArray
    | string
    | { message?: string; values?: Record<string, unknown> },
  ...values: unknown[]
) {
  if (typeof input === "string") {
    return input;
  }

  if (typeof input === "object" && !("raw" in input)) {
    let message = input.message ?? "";
    for (const [key, value] of Object.entries(input.values ?? {})) {
      message = message.split(`{${key}}`).join(String(value));
    }
    return message;
  }

  return Array.from(input).reduce(
    (text, part, index) => `${text}${part}${values[index] ?? ""}`,
    "",
  );
}

vi.mock("@lingui/react/macro", () => ({
  Trans: ({
    children,
    id,
    message,
    values,
  }: {
    children?: React.ReactNode;
    id?: string;
    message?: string;
    values?: Record<string, unknown>;
  }) =>
    React.createElement(
      React.Fragment,
      null,
      children ?? translate({ message: message ?? id, values }),
    ),
  useLingui: () => ({
    _: translate,
    t: translate,
  }),
}));

vi.mock("@lingui/react", () => ({
  I18nProvider: ({ children }: { children?: React.ReactNode }) =>
    React.createElement(React.Fragment, null, children),
  Trans: ({
    children,
    id,
    message,
    values,
  }: {
    children?: React.ReactNode;
    id?: string;
    message?: string;
    values?: Record<string, unknown>;
  }) =>
    React.createElement(
      React.Fragment,
      null,
      children ?? translate({ message: message ?? id, values }),
    ),
  useLingui: () => ({
    _: translate,
    t: translate,
    i18n: { _: translate, locale: "en" },
  }),
}));

vi.mock("./types/tauri.gen", () => ({
  commands: {
    getOnboardingNeeded: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: false }),
    showDevtool: vi.fn().mockResolvedValue(true),
    getPinnedTabs: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    setPinnedTabs: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    getRecentlyOpenedSessions: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: null }),
    setRecentlyOpenedSessions: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: null }),
    // Vault index read commands (Phase E): empty-index defaults.
    sessionGet: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    sessionListHeaders: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
    sessionIds: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
    sessionIsEmpty: vi.fn().mockResolvedValue({ status: "ok", data: true }),
    sessionHasTranscript: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: false }),
    sessionEnhancedDocs: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
    enhancedDocGet: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    sessionTranscripts: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
    transcriptGet: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    sessionFindByTrackingId: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: null }),
    sessionPrepareRecording: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: "/tmp/fmtr/sessions/session" }),
    sessionReleaseRecordingPrepare: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: null }),
    sessionQueueTagSuggestions: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: null }),
    sessionAcceptTagSuggestion: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: true }),
    sessionDismissTagSuggestion: vi
      .fn()
      .mockResolvedValue({ status: "ok", data: true }),
  },
  events: {
    indexChanged: {
      listen: vi.fn().mockResolvedValue(() => {}),
    },
    recordingMetaSettled: {
      listen: vi.fn().mockResolvedValue(() => {}),
    },
  },
}));
