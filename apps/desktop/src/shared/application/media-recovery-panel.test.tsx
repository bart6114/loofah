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
  reveal: vi.fn(),
  selectFile: vi.fn(),
  prepareSource: vi.fn(),
}));
vi.mock("@hypr/plugin-opener2", () => ({
  commands: { revealItemInDir: native.reveal },
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: native.selectFile }));
vi.mock("./device-attachment-source", async (original) => ({
  ...(await original<typeof import("./device-attachment-source")>()),
  prepareDeviceAttachmentSource: native.prepareSource,
}));

import { ApplicationRequestError } from "./errors";
import { MediaRecoveryPanel } from "./media-recovery-panel";
import type {
  MediaStatus,
  VaultClient,
  VaultClientEvent,
} from "./vault-client";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
beforeEach(() => vi.resetAllMocks());
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});
const missing = () =>
  new ApplicationRequestError({
    code: "not_found",
    message: "Original request not found on server",
    retryable: false,
    request_id: null,
    current_revision: null,
    mutation_outcome: null,
  });
async function fixture({ state = "missing", canEdit = true, count = 1 } = {}) {
  let emit!: (event: VaultClientEvent) => void;
  const job: MediaStatus = {
    request_id: "original-request",
    session_id: "",
    kind: "import_audio",
    state: state === "missing" ? "review" : (state as MediaStatus["state"]),
    transcript_id: null,
    word_count: null,
    error: null,
  };
  const receipts = Array.from({ length: count }, (_, i) => ({
    request_id: i ? `request-${i}` : "original-request",
    filename: i ? `recording-${i}.wav` : "Original.wav",
    session_id: null,
  }));
  const prepared = {
    input: {
      filename: "Original.wav",
      size_bytes: 128000,
      sha256: "a".repeat(64),
    },
    source: { kind: "path" as const, path: "/private/Original.wav" },
    close: vi.fn(async () => {}),
  };
  const media = {
    exportSource: vi.fn(async (_requestId: string, _options?: unknown) => ({
      path: "/downloads/Loofah/Original.wav",
    })),
    pendingSubmissions: vi.fn(async () => receipts),
    status: vi.fn(async (requestId: string) => {
      if (state === "missing") throw missing();
      return { ...job, request_id: requestId };
    }),
    recover: vi.fn(async () => ({ ...job, state: "receiving" as const })),
    resumeSource: vi.fn(async (_requestId: string) => ({
      ...job,
      state: "running" as const,
    })),
    cancel: vi.fn(async () => ({ ...job, state: "cancelled" as const })),
    importAudio: vi.fn(),
  };
  const prepare = native.prepareSource.mockResolvedValue(prepared);
  const client = {
    scope: "original-scope",
    canEdit,
    networkMode: "always",
    media,
    prepareAttachmentSource: prepare,
    subscribe: async (listener: typeof emit) => {
      emit = listener;
      emit({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient;
  const scope = new VaultScope(client);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, networkMode: "always" },
  });
  scopes.push(scope);
  await scope.start();
  const view = render(
    <StrictMode>
      <MediaRecoveryPanel scope={scope} />
    </StrictMode>,
  );
  const expand = async () => {
    fireEvent.click(screen.getByRole("button", { name: "Media recovery" }));
    await screen.findByText("Original.wav");
  };
  return {
    scope,
    media,
    prepare,
    prepared,
    emit: (event: VaultClientEvent) => emit(event),
    view,
    expand,
  };
}

it("explicitly saves a retained original without recovering or repeating inference", async () => {
  native.reveal.mockResolvedValue({ status: "ok", data: null });
  const f = await fixture({ state: "review" });
  await f.expand();
  expect(f.media.exportSource).not.toHaveBeenCalled();
  fireEvent.click(
    await screen.findByRole("button", { name: "Save original file" }),
  );
  await screen.findByText("Original file saved to Downloads/Loofah.");
  expect(f.media.exportSource).toHaveBeenCalledWith(
    "original-request",
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(native.reveal).toHaveBeenCalledWith("/downloads/Loofah/Original.wav");
  expect(f.media.recover).not.toHaveBeenCalled();
  expect(f.media.importAudio).not.toHaveBeenCalled();
});

it("keeps a failed source save for explicit retry with the same original request", async () => {
  const f = await fixture({ state: "review" });
  f.media.exportSource.mockRejectedValue(new Error("Source is unavailable"));
  await f.expand();
  fireEvent.click(
    await screen.findByRole("button", { name: "Save original file" }),
  );
  await screen.findByText("Source is unavailable");
  expect(f.media.exportSource).toHaveBeenCalledTimes(1);
  expect(native.reveal).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Save original file" }));
  await waitFor(() => expect(f.media.exportSource).toHaveBeenCalledTimes(2));
  expect(
    f.media.exportSource.mock.calls.every(([id]) => id === "original-request"),
  ).toBe(true);
  expect(f.media.recover).not.toHaveBeenCalled();
});

it("keeps lost POST receipts visible without creating a note or dispatching on load", async () => {
  const f = await fixture();
  expect(f.media.pendingSubmissions).not.toHaveBeenCalled();
  expect(f.media.status).not.toHaveBeenCalled();
  await f.expand();
  const recover = await screen.findByRole("button", {
    name: "Recover saved request",
  });
  expect(screen.getByText(/server outcome is not confirmed/i)).toBeTruthy();
  expect(f.media.recover).not.toHaveBeenCalled();
  expect(f.media.resumeSource).not.toHaveBeenCalled();
  expect(f.media.importAudio).not.toHaveBeenCalled();
  fireEvent.click(recover);
  await waitFor(() =>
    expect(f.media.recover).toHaveBeenCalledWith(
      "original-request",
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    ),
  );
  expect(f.media.importAudio).not.toHaveBeenCalled();
});

it("checks only one bounded page of saved requests while expanded", async () => {
  const f = await fixture({ state: "review", count: 25 });
  await f.expand();
  await waitFor(() =>
    expect(new Set(f.media.status.mock.calls.map((call) => call[0])).size).toBe(
      10,
    ),
  );
  expect(screen.queryByText("recording-10.wav")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Next" }));
  await waitFor(() =>
    expect(new Set(f.media.status.mock.calls.map((call) => call[0])).size).toBe(
      20,
    ),
  );
  expect(screen.queryByText("Original.wav")).toBeNull();
  expect(screen.getByText("recording-10.wav")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Media recovery" }));
  expect(screen.queryByText("recording-10.wav")).toBeNull();
  expect(f.media.recover).not.toHaveBeenCalled();
});

it("resumes a receiving upload with the original UUID and Native prepared source", async () => {
  const f = await fixture({ state: "receiving" });
  native.selectFile.mockResolvedValue("/private/Original.wav");
  await f.expand();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "Choose original file to resume upload",
    }),
  );
  await waitFor(() =>
    expect(f.media.resumeSource).toHaveBeenCalledWith(
      "original-request",
      expect.objectContaining({
        input: f.prepared.input,
        source: f.prepared.source,
      }),
      expect.anything(),
    ),
  );
  expect(f.prepare).toHaveBeenCalledWith(
    { kind: "path", path: "/private/Original.wav", name: "Original.wav" },
    expect.anything(),
  );
  await waitFor(() => expect(f.prepared.close).toHaveBeenCalledTimes(1));
  expect(f.media.importAudio).not.toHaveBeenCalled();
});

it("retains the same request and closes its source when original-byte verification refuses the file", async () => {
  const f = await fixture({ state: "receiving" });
  native.selectFile.mockResolvedValue("/private/Wrong.wav");
  f.media.resumeSource.mockRejectedValue(
    new Error("Selected file does not match the original source"),
  );
  await f.expand();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "Choose original file to resume upload",
    }),
  );
  expect((await screen.findByRole("alert")).textContent).toContain(
    "does not match",
  );
  expect(f.media.resumeSource.mock.calls[0][0]).toBe("original-request");
  expect(f.prepared.close).toHaveBeenCalledTimes(1);
  expect(f.media.importAudio).not.toHaveBeenCalled();
});

it("does not bind a file picker reply to a retired account or workspace", async () => {
  const f = await fixture({ state: "receiving" });
  let finish!: (path: string) => void;
  native.selectFile.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await f.expand();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "Choose original file to resume upload",
    }),
  );
  await waitFor(() => expect(native.selectFile).toHaveBeenCalled());
  act(() =>
    f.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await act(async () => finish("/private/Original.wav"));
  expect(screen.queryByText("Original.wav")).toBeNull();
  expect(f.prepare).not.toHaveBeenCalled();
  expect(f.media.resumeSource).not.toHaveBeenCalled();
});

it("drains a late prepared source after navigation without uploading it", async () => {
  const f = await fixture({ state: "receiving" });
  let finish!: (prepared: typeof f.prepared) => void;
  native.selectFile.mockResolvedValue("/private/Original.wav");
  f.prepare.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await f.expand();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "Choose original file to resume upload",
    }),
  );
  await waitFor(() => expect(f.prepare).toHaveBeenCalled());
  f.view.unmount();
  await act(async () => finish(f.prepared));
  await waitFor(() => expect(f.prepared.close).toHaveBeenCalledTimes(1));
  expect(f.media.resumeSource).not.toHaveBeenCalled();
});

it("keeps recovery disabled for read-only or disconnected scopes and never auto-recovers on reconnect", async () => {
  const f = await fixture({ canEdit: false });
  await f.expand();
  expect(
    (
      (await screen.findByRole("button", {
        name: "Recover saved request",
      })) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  act(() => f.emit({ kind: "connection", state: "reconnecting", error: null }));
  expect(
    (
      screen.getByRole("button", {
        name: "Refresh requests",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  act(() => f.emit({ kind: "connection", state: "connected", error: null }));
  await waitFor(() =>
    expect(f.media.pendingSubmissions.mock.calls.length).toBeGreaterThan(0),
  );
  expect(f.media.recover).not.toHaveBeenCalled();
  expect(f.media.resumeSource).not.toHaveBeenCalled();
});

it("shows uncertain status errors for explicit original-request recovery without inventing a fresh import", async () => {
  const f = await fixture({ state: "review" });
  f.media.status.mockRejectedValue(new Error("The server reply was lost"));
  await f.expand();
  expect((await screen.findByRole("alert")).textContent).toContain(
    "reply was lost",
  );
  expect(screen.getByText(/server outcome is not confirmed/i)).toBeTruthy();
  expect(f.media.recover).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "Recover saved request" }),
  );
  await waitFor(() =>
    expect(f.media.recover).toHaveBeenCalledWith(
      "original-request",
      expect.anything(),
    ),
  );
  expect(f.media.importAudio).not.toHaveBeenCalled();
});
