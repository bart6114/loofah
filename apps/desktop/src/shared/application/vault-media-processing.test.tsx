import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const clipboard = vi.hoisted(() => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => clipboard);

import type { LocalMediaStatus } from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";
import { VaultMediaProcessing } from "./vault-media-processing";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
beforeEach(() => vi.resetAllMocks());
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
});

async function fixture({ state = "review", canEdit = true } = {}) {
  let emit!: (event: VaultClientEvent) => void;
  const job = {
    request_id: "original-request",
    session_id: "note",
    kind: "transcribe",
    state,
    transcript_id: null,
    word_count: null,
    error: null,
  } as LocalMediaStatus;
  const media = {
    list: vi.fn(async () => [
      job,
      { ...job, request_id: "other", session_id: "other" },
    ]),
    recover: vi.fn(async () => job),
    cancel: vi.fn(async () => job),
    result: vi.fn(async () => ({
      words: [{ text: "Saved" }, { text: "speech" }],
    })),
    summaryStatus: vi.fn(async () => null),
  };
  const client = {
    scope: "original-scope",
    canEdit,
    networkMode: "always",
    media,
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
  const view = render(<VaultMediaProcessing scope={scope} sessionId="note" />);
  await screen.findByRole("status");
  return { media, scope, emit: (event: VaultClientEvent) => emit(event), view };
}

it("inspects the original session without dispatching recovery or inference", async () => {
  const f = await fixture();
  expect(
    screen.getAllByRole("button", { name: "Recover saved request" }),
  ).toHaveLength(1);
  expect(f.media.recover).not.toHaveBeenCalled();
  expect(f.media.cancel).not.toHaveBeenCalled();
  expect(f.media.list).toHaveBeenCalledWith("note", expect.anything());
  fireEvent.click(
    screen.getByRole("button", { name: "Recover saved request" }),
  );
  await waitFor(() => expect(f.media.recover).toHaveBeenCalledTimes(1));
  expect(f.media.recover).toHaveBeenCalledWith(
    "original-request",
    expect.anything(),
  );
});

it("cancels the original running request", async () => {
  const f = await fixture({ state: "running" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel processing" }));
  await waitFor(() =>
    expect(f.media.cancel).toHaveBeenCalledWith(
      "original-request",
      expect.anything(),
    ),
  );
  expect(f.media.recover).not.toHaveBeenCalled();
});

it("allows read-only retained output copying without enabling publication", async () => {
  const f = await fixture({ canEdit: false });
  expect(
    (
      screen.getByRole("button", {
        name: "Recover saved request",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  fireEvent.click(
    screen.getByRole("button", { name: "Copy retained transcript" }),
  );
  await waitFor(() =>
    expect(clipboard.writeText).toHaveBeenCalledWith("Saved speech"),
  );
  expect(f.media.recover).not.toHaveBeenCalled();
});

it("does not copy retained output after the scope loses access", async () => {
  const f = await fixture();
  let finish!: (result: unknown) => void;
  f.media.result.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }) as never,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Copy retained transcript" }),
  );
  await waitFor(() => expect(f.media.result).toHaveBeenCalled());
  act(() =>
    f.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await act(async () => finish({ words: [{ text: "Private" }] }));
  expect(clipboard.writeText).not.toHaveBeenCalled();
  expect(screen.queryByRole("region", { name: "Media processing" })).toBeNull();
});

it("does not copy a late reply after navigation", async () => {
  const f = await fixture();
  let finish!: (result: unknown) => void;
  f.media.result.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }) as never,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Copy retained transcript" }),
  );
  await waitFor(() => expect(f.media.result).toHaveBeenCalled());
  f.view.unmount();
  await act(async () => finish({ words: [{ text: "Old" }] }));
  expect(clipboard.writeText).not.toHaveBeenCalled();
});

it("reports missing retained output without retrying the model", async () => {
  const f = await fixture();
  f.media.result.mockResolvedValue(null as never);
  fireEvent.click(
    screen.getByRole("button", { name: "Copy retained transcript" }),
  );
  expect((await screen.findByRole("alert")).textContent).toContain(
    "No retained transcript",
  );
  expect(f.media.result).toHaveBeenCalledTimes(1);
  expect(f.media.recover).not.toHaveBeenCalled();
  expect(clipboard.writeText).not.toHaveBeenCalled();
});
