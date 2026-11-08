import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  records: [] as import("~/types/tauri.gen").DeviceRecordingInfo[],
  monitors: [] as {
    dispose: ReturnType<typeof vi.fn>;
    stop: ReturnType<typeof vi.fn>;
  }[],
  start: vi.fn(),
  append: vi.fn(),
  pendingAppend: vi.fn(),
  resume: vi.fn(),
  retire: vi.fn(),
  finish: vi.fn(),
  exportAudio: vi.fn(),
  cancelProcessing: vi.fn(),
  updates: new Set<() => void>(),
  destinations: [] as unknown[],
  authorizations: [] as (() => boolean)[],
}));
vi.mock("./device-recordings", () => ({
  createDeviceRecordingMonitor: () => {
    const monitor = {
      recoveryInventory: vi.fn(async () => native.records),
      subscribe: vi.fn(async (changed: () => void) => {
        native.updates.add(changed);
        return () => {
          native.updates.delete(changed);
        };
      }),
      stop: vi.fn(async () => true),
      finishRecovery: native.finish,
      exportAudio: native.exportAudio,
      cancelProcessing: native.cancelProcessing,
      dispose: vi.fn(),
    };
    native.monitors.push(monitor);
    return monitor;
  },
  createDeviceRecordingClient: (destination: unknown) => {
    native.destinations.push(destination);
    return {
      start: native.start,
      append: native.append,
      pendingAppendRequests: native.pendingAppend,
      recoveryInventory: vi.fn(async () => native.records),
      resumeUpload: native.resume,
      retire: (identity: unknown, request: string, current: () => boolean) => {
        native.authorizations.push(current);
        if (!current()) throw new Error("Original scope closed");
        return native.retire(identity, request);
      },
      dispose: vi.fn(),
    };
  },
}));

import {
  DeviceRecordButton,
  DeviceRecordingAccess,
  DeviceRecordingHost,
} from "./device-recording-host";
import { ApplicationRequestError } from "./errors";
import type { VaultClient } from "./vault-client";
import { VaultHostContext } from "./vault-provider";
import { VaultScope } from "./vault-scope";

const destination = {
  scopeKey: "scope",
  scope: {
    server: "https://company.example",
    member_id: "member",
    vault_id: "vault",
  },
  connectionId: "original-connection",
};
const record = (): import("~/types/tauri.gen").DeviceRecordingInfo => ({
  profile_id: "profile",
  identity: {
    scope: destination.scope,
    capture_id: "capture",
    request_id: "original-request",
    target: {
      kind: "existing_session",
      session_id: "note",
      expected_audio_sha256: null,
    },
  },
  phase: "capturing",
  first_audio_ms: 1_790_000_000_000,
  observation_durable: true,
  frames: 16000,
  pcm_bytes: 128000,
  sealed: false,
  publication: null,
  retirement: null,
  upload: null,
  processing: null,
  failure: null,
});
const scopes: VaultScope[] = [];
const caches: QueryClient[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  native.start.mockReset().mockResolvedValue(record());
  native.append.mockReset();
  native.pendingAppend.mockReset().mockResolvedValue([]);
  native.finish.mockReset();
  native.retire.mockReset();
  native.exportAudio.mockReset();
  native.cancelProcessing.mockReset().mockResolvedValue(true);
  native.records = [];
  native.monitors.length = 0;
  native.updates.clear();
  native.destinations.length = 0;
  native.authorizations.length = 0;
});
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  for (const cache of caches.splice(0)) cache.clear();
});

async function mount({
  bound = true,
  canEdit = true,
  scopeKey = "scope",
  active = false,
  strict = false,
  nativeConnectionId = "original-connection",
  features = ["capture_publication", "capture_retirement"],
  hasAudio = false,
} = {}) {
  if (active) native.records = [record()];
  const backend = {
    scope: scopeKey,
    nativeConnectionId,
    networkMode: "always",
    canEdit,
    features,
    audioMetadata: vi.fn(async () => (hasAudio ? { duration_ms: 1000 } : null)),
    transcriptOverview: vi.fn(async () => ({
      has_words: false,
      speaker_labels: [],
      started_at: null,
      ended_at: null,
    })),
    subscribe: vi.fn(async (listener) => {
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    }),
  } as unknown as VaultClient;
  const scope = new VaultScope(backend);
  scopes.push(scope);
  await scope.start();
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  caches.push(cache);
  const content = (profileId = "profile", boundScope = scope) => (
    <QueryClientProvider client={cache}>
      <DeviceRecordingHost
        profileId={profileId}
        destination={bound ? destination : null}
      >
        <div>Server sign-in or workspace</div>
        <VaultHostContext.Provider
          value={{ scope: boundScope, error: null, retry: () => {} }}
        >
          <DeviceRecordingAccess />
          <DeviceRecordButton sessionId="note" />
        </VaultHostContext.Provider>
      </DeviceRecordingHost>
    </QueryClientProvider>
  );
  const renderContent = (profileId = "profile", boundScope = scope) =>
    strict ? (
      <StrictMode>{content(profileId, boundScope)}</StrictMode>
    ) : (
      content(profileId, boundScope)
    );
  const result = render(renderContent());
  return {
    ...result,
    scope,
    backend,
    rerenderHost: (profileId = "profile", boundScope = scope) =>
      result.rerender(renderContent(profileId, boundScope)),
  };
}

it("keeps Stop available outside server sign-in and never stops capture on StrictMode retirement", async () => {
  const view = await mount({ bound: false, active: true, strict: true });
  const stop = await screen.findByRole("button", { name: "Stop recording" });
  expect(native.start).not.toHaveBeenCalled();
  fireEvent.click(stop);
  await waitFor(() =>
    expect(
      native.monitors.reduce(
        (count, item) => count + item.stop.mock.calls.length,
        0,
      ),
    ).toBe(1),
  );
  const called = native.monitors.find((item) => item.stop.mock.calls.length)!;
  expect(called.stop).toHaveBeenCalledWith(native.records[0].identity);
  view.unmount();
  expect(
    native.monitors.every((item) => item.dispose.mock.calls.length === 1),
  ).toBe(true);
  expect(
    native.monitors.reduce(
      (count, item) => count + item.stop.mock.calls.length,
      0,
    ),
  ).toBe(1);
});

it("preserves the start UUID after an unknown reply instead of starting a second recording", async () => {
  native.start.mockRejectedValue(
    new ApplicationRequestError({
      code: "unavailable",
      message: "Check device status",
      request_id: null,
      retryable: false,
      current_revision: null,
      mutation_outcome: "unknown",
    }),
  );
  await mount();
  const button = await screen.findByRole("button", { name: "Record audio" });
  await waitFor(() =>
    expect((button as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(button);
  await screen.findByText("Check device status");
  fireEvent.click(button);
  await waitFor(() => expect(native.start).toHaveBeenCalledTimes(2));
  expect(native.start.mock.calls[0][0]).toEqual(native.start.mock.calls[1][0]);
  expect(native.start.mock.calls[0][0]).toMatchObject({
    target: { session_id: "note" },
    format: { channels: ["microphone", "system_audio"] },
  });
  expect(native.monitors.every((item) => !item.stop.mock.calls.length)).toBe(
    true,
  );
});

it("does not read local models or admit capture for a different scoped workspace", async () => {
  const { backend } = await mount({ scopeKey: "other-vault" });
  await act(async () => {});
  expect(screen.queryByRole("button", { name: "Record audio" })).toBeNull();
  expect(backend.audioMetadata).not.toHaveBeenCalled();
  expect(backend.transcriptOverview).not.toHaveBeenCalled();
  expect(native.start).not.toHaveBeenCalled();
});

it("keeps reader capture unavailable without dispatching native work", async () => {
  const { backend } = await mount({ canEdit: false });
  const button = await screen.findByRole("button", { name: "Record audio" });
  expect((button as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(button);
  expect(native.start).not.toHaveBeenCalled();
  expect(backend.audioMetadata).not.toHaveBeenCalled();
});

it("updates retained audio status after Stop without requiring a server connection", async () => {
  await mount({ bound: false, active: true });
  await screen.findByRole("button", { name: "Stop recording" });
  native.records = [{ ...native.records[0], phase: "finished", sealed: true }];
  act(() => {
    for (const changed of native.updates) changed();
  });
  const resume = await screen.findByRole("button", { name: "Resume upload" });
  expect((resume as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole("button", { name: "Stop recording" })).toBeNull();
  expect(native.resume).not.toHaveBeenCalled();
});

it("starts a pending capture before server sign-in using only the saved destination", async () => {
  const cache = new QueryClient();
  caches.push(cache);
  render(
    <QueryClientProvider client={cache}>
      <DeviceRecordingHost
        profileId="profile"
        destination={{
          scopeKey: destination.scopeKey,
          scope: destination.scope,
        }}
      >
        <div>Server sign-in</div>
      </DeviceRecordingHost>
    </QueryClientProvider>,
  );
  const button = await screen.findByRole("button", {
    name: "Record a new note",
  });
  await waitFor(() =>
    expect((button as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(button);
  await waitFor(() => expect(native.start).toHaveBeenCalledOnce());
  const request = native.start.mock.calls[0][0];
  expect(request.target).toEqual({
    kind: "pending_session",
    create_request_id: expect.any(String),
  });
  expect(request.request_id).not.toBe(request.target.create_request_id);
  expect(native.destinations).toEqual([
    { scopeKey: destination.scopeKey, scope: destination.scope },
  ]);
  expect(native.resume).not.toHaveBeenCalled();
});

it("retains both pending request identities when the start reply is unknown", async () => {
  native.start.mockRejectedValue(new Error("Unknown recording outcome"));
  await mount();
  const button = await screen.findByRole("button", {
    name: "Record a new note",
  });
  await waitFor(() =>
    expect((button as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(button);
  await screen.findByText("Unknown recording outcome");
  fireEvent.click(button);
  await waitFor(() => expect(native.start).toHaveBeenCalledTimes(2));
  expect(native.start.mock.calls[1][0]).toEqual(native.start.mock.calls[0][0]);
  expect(native.start.mock.calls[0][0].target.kind).toBe("pending_session");
});

it("captures explicit processing options before an offline pending start and preserves them after an unknown reply", async () => {
  native.start.mockRejectedValue(new Error("Unknown recording outcome"));
  await mount();
  const button = await screen.findByRole("button", {
    name: "Record a new note",
  });
  await waitFor(() =>
    expect((button as HTMLButtonElement).disabled).toBe(false),
  );
  const transcribe = screen.getAllByRole("checkbox", {
    name: "Transcribe on the server",
  })[0];
  const summarize = screen.getAllByRole("checkbox", {
    name: "Summarize when transcription finishes",
  })[0];
  expect(summarize.matches(":disabled")).toBe(true);
  fireEvent.click(transcribe);
  fireEvent.click(summarize);
  fireEvent.click(button);
  await screen.findByText("Unknown recording outcome");
  expect(transcribe.matches(":disabled")).toBe(true);
  expect(summarize.matches(":disabled")).toBe(true);
  fireEvent.click(button);
  await waitFor(() => expect(native.start).toHaveBeenCalledTimes(2));
  const first = native.start.mock.calls[0][0];
  expect(first.processing).toEqual({
    model: null,
    languages: [],
    keywords: [],
    summary: {
      target: { kind: "summary" },
      title_policy: "generate_if_empty",
    },
  });
  expect(native.start.mock.calls[1][0]).toEqual(first);
  expect(first.target.kind).toBe("pending_session");
});

it("starts raw audio without declaring optional processing when it was not selected", async () => {
  await mount();
  const button = await screen.findByRole("button", { name: "Record audio" });
  await waitFor(() =>
    expect((button as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(button);
  await waitFor(() => expect(native.start).toHaveBeenCalledOnce());
  expect(native.start.mock.calls[0][0].processing).toBeNull();
});

function processingRecord(): import("~/types/tauri.gen").DeviceRecordingInfo {
  const value = record();
  const format = {
    encoding: "pcm_f32_le" as const,
    sample_rate: 16000,
    channels: ["microphone" as const, "system_audio" as const],
  };
  return {
    ...value,
    processing: {
      format,
      declaration: {
        request: {
          capture_fingerprint: "a".repeat(64),
          model: null,
          languages: [],
          keywords: [],
          summary: null,
        },
        cancel_requested: false,
      },
      status: null,
      error: null,
      captions: {
        binding: {
          capture_id: value.identity.capture_id,
          generation_id: "generation",
          capture_fingerprint: "a".repeat(64),
          session_id: "note",
          format,
        },
        connection: "reconnecting",
        error: null,
        next_sequence: 1,
        words: [
          {
            id: "word",
            text: "Saved caption",
            start_ms: 0,
            end_ms: 100,
            channel: 0,
            role: "microphone",
          },
        ],
        partials: [],
      },
    },
  };
}

it("shows interrupted caption delivery and cancels processing without stopping or retargeting raw audio", async () => {
  native.records = [processingRecord()];
  await mount();
  await screen.findByText("Saved caption");
  await screen.findByText(
    "Live captions are interrupted. Recording continues on this device.",
  );
  fireEvent.click(screen.getByRole("button", { name: "Cancel processing" }));
  await waitFor(() =>
    expect(native.cancelProcessing).toHaveBeenCalledExactlyOnceWith(
      native.records[0].identity,
    ),
  );
  expect(
    native.monitors.every((monitor) => !monitor.stop.mock.calls.length),
  ).toBe(true);
  expect(native.resume).not.toHaveBeenCalled();
  expect(native.start).not.toHaveBeenCalled();
});

it("keeps another member's caption text private while retaining device audio recovery", async () => {
  const value = processingRecord();
  value.identity = {
    ...value.identity,
    scope: { ...value.identity.scope, member_id: "previous-member" },
  };
  native.records = [value];
  await mount();
  await screen.findByRole("button", { name: "Stop recording" });
  expect(screen.queryByText("Saved caption")).toBeNull();
  expect(
    screen.queryByRole("button", { name: "Cancel processing" }),
  ).toBeNull();
  expect(native.cancelProcessing).not.toHaveBeenCalled();
});

it("keeps pending capture disabled while this profile still owns an active recording", async () => {
  await mount({ active: true });
  const button = await screen.findByRole("button", {
    name: "Record a new note",
  });
  expect((button as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(button);
  expect(native.start).not.toHaveBeenCalled();
});

it("prepares only the original stopped prefix while signed out and leaves upload explicit", async () => {
  native.records = [{ ...record(), phase: "finished", sealed: false }];
  native.finish.mockImplementation(async () => {
    native.records = [{ ...native.records[0], sealed: true }];
    return native.records[0];
  });
  const original = native.records[0].identity;
  await mount({ bound: false });
  const finish = await screen.findByRole("button", {
    name: "Prepare saved audio for upload",
  });
  fireEvent.click(finish);
  await waitFor(() =>
    expect(native.finish).toHaveBeenCalledExactlyOnceWith(original),
  );
  const resume = await screen.findByRole("button", { name: "Resume upload" });
  expect((resume as HTMLButtonElement).disabled).toBe(true);
  expect(native.resume).not.toHaveBeenCalled();
  expect(native.start).not.toHaveBeenCalled();
});

it("exports untimed audio without login, a new recording, or an invented seal", async () => {
  native.records = [
    {
      ...record(),
      phase: null,
      first_audio_ms: null,
      observation_durable: false,
    },
  ];
  native.exportAudio.mockResolvedValue({
    frames: 16000,
    pcm_bytes: 128000,
    file_bytes: 128068,
    sha256: "a".repeat(64),
    sealed: false,
  });
  await mount({ bound: false });
  const exportButton = await screen.findByRole("button", {
    name: "Export saved audio",
  });
  expect(
    screen.queryByRole("button", { name: "Prepare saved audio for upload" }),
  ).toBeNull();
  fireEvent.click(exportButton);
  await screen.findByText(
    "Audio exported. The original recording is retained on this device.",
  );
  expect(native.exportAudio).toHaveBeenCalledExactlyOnceWith(
    native.records[0].identity,
  );
  expect(native.finish).not.toHaveBeenCalled();
  expect(native.resume).not.toHaveBeenCalled();
  expect(native.start).not.toHaveBeenCalled();
  expect(native.records[0].sealed).toBe(false);
});

it("treats a closed export picker as cancellation and keeps the original item recoverable", async () => {
  native.records = [{ ...record(), phase: "finished", sealed: true }];
  native.exportAudio.mockResolvedValue(null);
  await mount({ bound: false });
  const exportButton = await screen.findByRole("button", {
    name: "Export saved audio",
  });
  fireEvent.click(exportButton);
  await waitFor(() => expect(native.exportAudio).toHaveBeenCalledOnce());
  expect(
    screen.queryByText(
      "Audio exported. The original recording is retained on this device.",
    ),
  ).toBeNull();
  await waitFor(() =>
    expect((exportButton as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(exportButton);
  await waitFor(() => expect(native.exportAudio).toHaveBeenCalledTimes(2));
  expect(native.exportAudio.mock.calls[1][0]).toEqual(
    native.exportAudio.mock.calls[0][0],
  );
});

it("keeps recovery and export unavailable until active capture has finished saving", async () => {
  await mount({ bound: false, active: true });
  await screen.findByRole("button", { name: "Stop recording" });
  expect(
    screen.queryByRole("button", { name: "Prepare saved audio for upload" }),
  ).toBeNull();
  expect(
    screen.queryByRole("button", { name: "Export saved audio" }),
  ).toBeNull();
  expect(native.finish).not.toHaveBeenCalled();
  expect(native.exportAudio).not.toHaveBeenCalled();
});

function publishedRecord(): import("~/types/tauri.gen").DeviceRecordingInfo {
  const value = record();
  return {
    ...value,
    phase: "finished",
    sealed: true,
    publication: {
      session_id: "note",
      canonical_audio_sha256: "c".repeat(64),
      canonical_audio_bytes: 128044,
      source: {
        capture_fingerprint: "a".repeat(64),
        chunk_count: 1,
        frames: value.frames,
        pcm_bytes: value.pcm_bytes,
        chain_digest: "b".repeat(64),
        source_sha256: "d".repeat(64),
      },
    },
  };
}
function cleanupReceipt(
  value: import("~/types/tauri.gen").DeviceRecordingInfo,
  request: string,
  state: "declared" | "pending" | "retired",
): import("~/types/tauri.gen").CaptureRetirementSnapshot {
  return {
    state,
    remaining_chunks: state === "retired" ? 0 : 1,
    request: {
      cleanup_request_id: request,
      identity: value.identity,
      format: {
        encoding: "pcm_f32_le",
        sample_rate: 16000,
        channels: ["microphone", "system_audio"],
      },
      source: value.publication!.source,
      publication: value.publication!,
      processing: null,
    },
  };
}
it("cleanup requires exact editable connected owner and never runs from startup, signed-out or foreign connection", async () => {
  for (const options of [
    { bound: false },
    { canEdit: false },
    { nativeConnectionId: "replacement" },
    { features: ["capture_publication"] },
    { scopeKey: "other" },
  ]) {
    native.records = [publishedRecord()];
    const view = await mount(options);
    await screen.findByText(
      "Verified on the server. The device copy is retained.",
    );
    expect(
      screen.queryByRole("button", { name: "Free device audio" }),
    ).toBeNull();
    expect(native.retire).not.toHaveBeenCalled();
    view.unmount();
  }
  native.records = [publishedRecord()];
  const { backend } = await mount();
  await screen.findByRole("button", { name: "Free device audio" });
  expect(native.retire).not.toHaveBeenCalled();
  const receive = vi.mocked(backend.subscribe).mock.calls[0][0];
  act(() =>
    receive({ kind: "connection", state: "reconnecting", error: null }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Free device audio" }),
    ).toBeNull(),
  );
  act(() => receive({ kind: "connection", state: "connected", error: null }));
  await screen.findByRole("button", { name: "Free device audio" });
  act(() =>
    receive({ kind: "connection", state: "login_required", error: null }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Free device audio" }),
    ).toBeNull(),
  );
  expect(native.retire).not.toHaveBeenCalled();
});
it("explicit cleanup retries the durable UUID after remount and fences retired upload and export controls", async () => {
  const saved = publishedRecord();
  saved.retirement = cleanupReceipt(saved, "original-cleanup", "declared");
  native.records = [saved];
  native.retire.mockRejectedValueOnce(
    new Error("Verify the original cleanup again"),
  );
  const first = await mount();
  fireEvent.click(await screen.findByRole("button", { name: "Retry cleanup" }));
  await screen.findByText("Verify the original cleanup again");
  expect(native.retire).toHaveBeenCalledWith(
    saved.identity,
    "original-cleanup",
  );
  first.unmount();
  native.retire.mockImplementation(async () => {
    const retired = {
      ...saved,
      retirement: cleanupReceipt(saved, "original-cleanup", "retired"),
    };
    native.records = [retired];
    return retired;
  });
  await mount();
  fireEvent.click(await screen.findByRole("button", { name: "Retry cleanup" }));
  await screen.findByText(
    "Server copy verified. Saved audio has been removed from this device.",
  );
  expect(native.retire.mock.calls).toEqual([
    [saved.identity, "original-cleanup"],
    [saved.identity, "original-cleanup"],
  ]);
  expect(
    screen.queryByRole("button", { name: "Export saved audio" }),
  ).toBeNull();
  expect(screen.queryByRole("button", { name: "Resume upload" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Retry cleanup" })).toBeNull();
  expect(native.exportAudio).not.toHaveBeenCalled();
  expect(native.resume).not.toHaveBeenCalled();
});
it("unfinished source removal remains fenced and uncertain processing never enables cleanup", async () => {
  const saved = publishedRecord();
  saved.retirement = cleanupReceipt(saved, "original-cleanup", "pending");
  native.records = [saved];
  const view = await mount();
  await screen.findByRole("button", { name: "Retry cleanup" });
  expect(
    screen.queryByRole("button", { name: "Export saved audio" }),
  ).toBeNull();
  expect(native.retire).not.toHaveBeenCalled();
  view.unmount();
  const unknown = processingRecord();
  unknown.phase = "finished";
  unknown.sealed = true;
  unknown.publication = saved.publication;
  unknown.processing!.status = {
    capture_id: unknown.identity.capture_id,
    capture_fingerprint: "a".repeat(64),
    intent_id: unknown.identity.request_id,
    session_id: "note",
    model: "server-model",
    request: unknown.processing!.declaration.request,
    state: "uncertain",
    live_generation_id: null,
    repair_job_id: null,
    summary_job_id: null,
    transcript_publication: null,
    failure: "storage",
    created_at: "2026-11-08T00:00:00Z",
    updated_at: "2026-11-08T00:00:00Z",
  };

  native.records = [unknown];
  await mount();
  const button = await screen.findByRole("button", {
    name: "Free device audio",
  });
  expect((button as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(button);
  expect(native.retire).not.toHaveBeenCalled();
});

it("retires cleanup authorization when only the profile changes", async () => {
  native.records = [publishedRecord()];
  native.retire.mockImplementation(() => new Promise(() => {}));
  const view = await mount();
  fireEvent.click(
    await screen.findByRole("button", { name: "Free device audio" }),
  );
  await waitFor(() => expect(native.authorizations).toHaveLength(1));
  const original = native.authorizations[0];
  expect(original()).toBe(true);
  view.rerenderHost("other-profile");
  expect(original()).toBe(false);
});

it("does not reuse cleanup authorization after the access scope is replaced", async () => {
  native.records = [publishedRecord()];
  native.retire.mockImplementation(() => new Promise(() => {}));
  const view = await mount();
  fireEvent.click(
    await screen.findByRole("button", { name: "Free device audio" }),
  );
  await waitFor(() => expect(native.authorizations).toHaveLength(1));
  const original = native.authorizations[0];
  const replacement = new VaultScope(view.backend);
  scopes.push(replacement);
  await replacement.start();
  view.rerenderHost("profile", replacement);
  await waitFor(() => expect(original()).toBe(false));
  expect(view.scope.getSnapshot().state).toBe("connected");
});

it("offers append only for the same live client with the append capability", async () => {
  for (const options of [
    { features: ["capture_publication"] },
    {
      features: ["capture_publication", "capture_append"],
      nativeConnectionId: "replacement",
    },
    { features: ["capture_publication", "capture_append"], canEdit: false },
  ]) {
    const view = await mount({ ...options, hasAudio: true });
    const button = await screen.findByRole("button", {
      name: options.canEdit === false ? "Record audio" : "Append audio",
    });
    expect((button as HTMLButtonElement).disabled).toBe(true);
    view.unmount();
  }
  expect(native.append).not.toHaveBeenCalled();
  expect(native.pendingAppend).not.toHaveBeenCalled();
});

it("reuses the saved append declaration after restart and never guesses a format", async () => {
  const original = {
    scope: destination.scope,
    session_id: "note",
    request_id: "original-append-request",
    capture_id: "original-append-capture",
    admission_id: "original-admission",
    processing: null,
  };
  native.pendingAppend.mockResolvedValue([original]);
  native.append.mockRejectedValue(new Error("Lost append response"));
  const view = await mount({
    hasAudio: true,
    features: ["capture_publication", "capture_append"],
  });
  const button = await screen.findByRole("button", { name: "Append audio" });
  await waitFor(() =>
    expect((button as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(button);
  await screen.findByText("Lost append response");
  fireEvent.click(button);
  await waitFor(() => expect(native.append).toHaveBeenCalledTimes(2));
  expect(native.append.mock.calls[0][0]).toEqual(original);
  expect(native.append.mock.calls[1][0]).toEqual(original);
  expect(native.append.mock.calls[0][0]).not.toHaveProperty("format");
  expect(native.start).not.toHaveBeenCalled();
  view.unmount();
});

it("refuses ambiguous saved append requests instead of obtaining a fresh baseline", async () => {
  native.pendingAppend.mockResolvedValue([
    { session_id: "note", request_id: "first" },
    { session_id: "note", request_id: "second" },
  ]);
  await mount({
    hasAudio: true,
    features: ["capture_publication", "capture_append"],
  });
  await screen.findByText(
    "Multiple saved append requests need recovery before recording again.",
  );
  expect(
    (screen.getByRole("button", { name: "Append audio" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(native.append).not.toHaveBeenCalled();
  expect(native.start).not.toHaveBeenCalled();
});
