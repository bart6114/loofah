import { onlineManager } from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { useSessionAttachments } from "./useAttachmentResolver";

import { createSessionSourceHost } from "~/session/session-source.test-support";
import type { VaultClient } from "~/shared/application/vault-client";

const hosts: ReturnType<typeof createSessionSourceHost>[] = [];
function fixture(mode: "always" | "online" = "always") {
  let filename = "initial.png";
  const list = vi.fn<VaultClient["attachments"]>(async (session) => ({
    files: [
      { filename: session + filename, contentType: "image/png", sizeBytes: 7 },
    ],
    next_cursor: null,
  }));
  const host = createSessionSourceHost(mode, {
    features: ["attachment_list"],
    attachments: list,
  });
  hosts.push(host);
  return {
    ...host,
    list,
    rename: (value: string) => {
      filename = value;
    },
  };
}
afterEach(() => {
  cleanup();
  hosts.splice(0).forEach((host) => host.dispose());
  onlineManager.setOnline(true);
});
it("subscribes to captured attachment artifacts and resync without invalidating another vault's identical IDs", async () => {
  const own = fixture(),
    other = fixture();
  const a = renderHook(() => useSessionAttachments("same"), {
    wrapper: own.Host,
  });
  const b = renderHook(() => useSessionAttachments("same"), {
    wrapper: other.Host,
  });
  await waitFor(() =>
    expect(a.result.current.data?.[0].filename).toBe("sameinitial.png"),
  );
  await waitFor(() =>
    expect(b.result.current.data?.[0].filename).toBe("sameinitial.png"),
  );
  own.rename("updated.png");
  act(() =>
    own.emit({
      kind: "change",
      change: {
        kind: "index_changed",
        entity: "artifacts",
        ids: ["same"],
        artifacts: [{ session_id: "same", target: { kind: "attachments" } }],
      },
    }),
  );
  await waitFor(() =>
    expect(a.result.current.data?.[0].filename).toBe("sameupdated.png"),
  );
  expect(other.list).toHaveBeenCalledOnce();
  own.rename("resynced.png");
  act(() => own.emit({ kind: "change", change: { kind: "resync_required" } }));
  await waitFor(() =>
    expect(a.result.current.data?.[0].filename).toBe("sameresynced.png"),
  );
  expect(other.list).toHaveBeenCalledOnce();
});
it("refuses an old retry closure after session change and hides terminal data permanently", async () => {
  const own = fixture();
  const view = renderHook(({ id }) => useSessionAttachments(id), {
    wrapper: own.Host,
    initialProps: { id: "first" },
  });
  await waitFor(() =>
    expect(view.result.current.data?.[0].filename).toBe("firstinitial.png"),
  );
  const old = view.result.current.refetch;
  view.rerender({ id: "next" });
  await waitFor(() =>
    expect(view.result.current.data?.[0].filename).toBe("nextinitial.png"),
  );
  await old();
  expect(own.list).toHaveBeenCalledTimes(2);
  act(() =>
    own.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  expect(view.result.current.data).toBeUndefined();
  act(() => own.emit({ kind: "connection", state: "connected", error: null }));
  expect(view.result.current.data).toBeUndefined();
});
it("keeps local offline listing and prevents retired paused remote reads from dispatching on reconnect", async () => {
  onlineManager.setOnline(false);
  const local = fixture();
  const ready = renderHook(() => useSessionAttachments("same"), {
    wrapper: local.Host,
  });
  await waitFor(() =>
    expect(ready.result.current.data?.[0].filename).toBe("sameinitial.png"),
  );
  const remote = fixture("online");
  renderHook(() => useSessionAttachments("same"), { wrapper: remote.Host });
  expect(remote.list).not.toHaveBeenCalled();
  remote.scope.dispose();
  act(() => onlineManager.setOnline(true));
  await Promise.resolve();
  expect(remote.list).not.toHaveBeenCalled();
});
