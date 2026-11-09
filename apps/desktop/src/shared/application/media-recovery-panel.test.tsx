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
import type { MediaRetirementRequest } from "./protocol.gen";
import type {
  MediaStatus,
  VaultClient,
  VaultClientEvent,
} from "./vault-client";
import { VaultScope } from "./vault-scope";

import type { MediaRetirementView } from "~/types/tauri.gen";

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
async function fixture({
  state = "missing",
  canEdit = true,
  count = 1,
  retirement: initialRetirement,
}: {
  state?: string;
  canEdit?: boolean;
  count?: number;
  retirement?: MediaRetirementView;
} = {}) {
  let retirement = initialRetirement;
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
    cleanup: retirement?.pending ? ("pending" as const) : undefined,
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
    retirementStatus: retirement
      ? vi.fn(async (_requestId: string, _options?: unknown) => retirement!)
      : undefined,
    retire: retirement
      ? vi.fn(
          async (
            _id: string,
            input: MediaRetirementRequest,
          ): Promise<MediaRetirementView> => {
            retirement = {
              ...retirement!,
              pending: input,
              status: {
                ...retirement!.status,
                state: "retired",
                retirement: input,
                allowed_dispositions: [],
                quota_released: true,
              },
            };
            return retirement;
          },
        )
      : undefined,
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

function retirementView(): MediaRetirementView {
  return {
    status: {
      request_id: "original-request",
      revision: "6b6769c3-cc92-4383-b7be-d3472457b746",
      declaration_sha256: "b".repeat(64),
      source: { sha256: "a".repeat(64), size_bytes: 128000 },
      state: "eligible",
      retirement: null,
      reason: null,
      quota_released: false,
      allowed_dispositions: ["exported", "discard"],
    },
    pending: null,
    verified_export: false,
  };
}

it("reviews explicit source loss before retirement and never treats an unchecked export as proof", async () => {
  const f = await fixture({ state: "succeeded", retirement: retirementView() });
  await f.expand();
  expect(f.media.retirementStatus).not.toHaveBeenCalled();
  fireEvent.click(
    await screen.findByRole("button", { name: "Review retained files" }),
  );
  expect(f.media.retire).not.toHaveBeenCalled();
  expect(
    await screen.findByRole("option", {
      name: "I have kept the verified exported copy",
    }),
  ).toHaveProperty("disabled", true);
  fireEvent.change(
    await screen.findByRole("combobox", { name: "Original source" }),
    { target: { value: "discard" } },
  );
  expect(
    screen.getByRole("button", { name: "Remove retained upload" }),
  ).toHaveProperty("disabled", true);
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(
    screen.getByRole("button", { name: "Remove retained upload" }),
  );
  await waitFor(() => expect(f.media.retire).toHaveBeenCalledTimes(1));
  expect(f.media.retire!.mock.calls[0][0]).toBe("original-request");
  expect(f.media.retire!.mock.calls[0][1]).toMatchObject({
    expected_revision: retirementView().status.revision,
    expected_declaration_sha256: "b".repeat(64),
    expected_source: retirementView().status.source,
    disposition: "discard",
  });
  await screen.findByText(
    "Retained upload retired; request quota released. Canonical content and results remain available.",
  );
  expect(
    screen.queryByRole("button", { name: "Save original file" }),
  ).toBeNull();
  expect(f.media.recover).not.toHaveBeenCalled();
  expect(f.media.resumeSource).not.toHaveBeenCalled();
});

it("reuses the original reviewed cleanup UUID after an unknown reply without automatic retry", async () => {
  const f = await fixture({ state: "succeeded", retirement: retirementView() });
  f.media.retire!.mockRejectedValueOnce(new Error("Cleanup outcome unknown"));
  await f.expand();
  expect(f.media.retirementStatus).not.toHaveBeenCalled();
  fireEvent.click(
    await screen.findByRole("button", { name: "Review retained files" }),
  );
  fireEvent.change(
    await screen.findByRole("combobox", { name: "Original source" }),
    { target: { value: "discard" } },
  );
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(
    screen.getByRole("button", { name: "Remove retained upload" }),
  );
  await screen.findByText("Cleanup outcome unknown");
  expect(f.media.retire).toHaveBeenCalledTimes(1);
  expect(
    screen.getByRole("combobox", { name: "Original source" }),
  ).toHaveProperty("disabled", true);
  fireEvent.click(
    screen.getByRole("button", { name: "Resume original cleanup" }),
  );
  await waitFor(() => expect(f.media.retire).toHaveBeenCalledTimes(2));
  expect(f.media.retire!.mock.calls[1][1]).toEqual(
    f.media.retire!.mock.calls[0][1],
  );
  expect(f.media.recover).not.toHaveBeenCalled();
});

it("loads a saved cleanup decision read-only and fences original-source recovery", async () => {
  const view = retirementView();
  view.pending = {
    retirement_id: "9b6769c3-cc92-4383-b7be-d3472457b746",
    expected_revision: view.status.revision,
    expected_declaration_sha256: view.status.declaration_sha256,
    expected_source: view.status.source,
    disposition: "discard",
  };
  const f = await fixture({ state: "receiving", retirement: view });
  await f.expand();
  const review = await screen.findByRole("button", {
    name: "Review original cleanup",
  });
  expect(f.media.retire).not.toHaveBeenCalled();
  expect(
    screen.queryByRole("button", {
      name: "Choose original file to resume upload",
    }),
  ).toBeNull();
  expect(
    screen.queryByRole("button", { name: "Save original file" }),
  ).toBeNull();
  fireEvent.click(review);
  fireEvent.click(await screen.findByRole("checkbox"));
  fireEvent.click(
    screen.getByRole("button", { name: "Resume original cleanup" }),
  );
  await waitFor(() => expect(f.media.retire).toHaveBeenCalledTimes(1));
  expect(f.media.retire!.mock.calls[0][1]).toEqual(view.pending);
});

it("keeps cleanup unavailable for Readers and after the original workspace disconnects", async () => {
  const f = await fixture({
    state: "succeeded",
    canEdit: false,
    retirement: retirementView(),
  });
  await f.expand();
  expect(
    await screen.findByRole("button", { name: "Review retained files" }),
  ).toHaveProperty("disabled", true);
  act(() => f.emit({ kind: "connection", state: "reconnecting", error: null }));
  expect(
    screen.getByRole("button", { name: "Review retained files" }),
  ).toHaveProperty("disabled", true);
  expect(f.media.retire).not.toHaveBeenCalled();
});

it("queries retirement eligibility only for the reviewed request on a bounded recovery page", async () => {
  const f = await fixture({
    state: "succeeded",
    count: 25,
    retirement: retirementView(),
  });
  await f.expand();
  const reviews = await screen.findAllByRole("button", {
    name: "Review retained files",
  });
  expect(reviews).toHaveLength(10);
  expect(f.media.retirementStatus).not.toHaveBeenCalled();
  fireEvent.click(reviews[0]);
  await screen.findByRole("combobox", { name: "Original source" });
  expect(f.media.retirementStatus).toHaveBeenCalledTimes(1);
  expect(f.media.retirementStatus!.mock.calls[0][0]).toBe("original-request");
  expect(f.media.retire).not.toHaveBeenCalled();
});

it("keeps an opened cleanup review inactive across disconnection until an explicit original-scope submit", async () => {
  const f = await fixture({ state: "succeeded", retirement: retirementView() });
  await f.expand();
  fireEvent.click(
    await screen.findByRole("button", { name: "Review retained files" }),
  );
  fireEvent.change(
    await screen.findByRole("combobox", { name: "Original source" }),
    { target: { value: "discard" } },
  );
  fireEvent.click(screen.getByRole("checkbox"));
  act(() => f.emit({ kind: "connection", state: "reconnecting", error: null }));
  expect(
    screen.getByRole("button", { name: "Remove retained upload" }),
  ).toHaveProperty("disabled", true);
  expect(f.media.retire).not.toHaveBeenCalled();
  act(() => f.emit({ kind: "connection", state: "connected", error: null }));
  expect(f.media.retire).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "Remove retained upload" }),
  );
  await waitFor(() => expect(f.media.retire).toHaveBeenCalledTimes(1));
});

it("reviews request-quota release without claiming uploaded files exist for a no-upload request", async () => {
  const view = retirementView();
  view.status.source = null;
  view.status.allowed_dispositions = ["no_upload"];
  const f = await fixture({ state: "succeeded", retirement: view });
  await f.expand();
  fireEvent.click(
    await screen.findByRole("button", { name: "Review retained files" }),
  );
  fireEvent.change(
    await screen.findByRole("combobox", { name: "Original source" }),
    { target: { value: "no_upload" } },
  );
  expect(
    screen.queryByRole("option", {
      name: "Discard the retained original upload",
    }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(
    screen.getByRole("button", { name: "Release request quota" }),
  );
  await waitFor(() => expect(f.media.retire).toHaveBeenCalledTimes(1));
  expect(f.media.retire!.mock.calls[0][1]).toMatchObject({
    disposition: "no_upload",
    expected_source: null,
  });
});
