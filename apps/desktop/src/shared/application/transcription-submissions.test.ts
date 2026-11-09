import { describe, expect, it, vi } from "vitest";

vi.mock("~/types/tauri.gen", () => ({ commands: {} }));

import type {
  TranscriptionJobRequest,
  TranscriptionJobSnapshot,
} from "./protocol.gen";
import { TranscriptionSubmissions } from "./transcription-submissions";
import type { VaultScope } from "./vault-scope";

import type { TranscriptionSubmissionReceipt } from "~/types/tauri.gen";

function fixture() {
  let state = "connected";
  const listeners = new Set<() => void>();
  const order: string[] = [];
  const disk: TranscriptionSubmissionReceipt[] = [];
  const job: TranscriptionJobSnapshot = {
    id: "job",
    session_id: "note",
    state: "queued",
    phase: null,
    created_at: "time",
    updated_at: "time",
    failure: null,
    publication: null,
    result_available: false,
  };
  const api = {
    policy: vi.fn(async () => ({
      batch_enabled: true,
      availability: "ready",
      model: "original",
      live_enabled: false,
    })),
    submit: vi.fn(async () => {
      order.push("dispatch");
      return job;
    }),
    get: vi.fn(async () => job),
  };
  const client = {
    scope: "original-scope",
    canEdit: true,
    features: ["batch_transcription_jobs", "transcription_policy"],
    transcriptionJobs: api,
    audioMetadata: vi.fn(async () => ({ sha256: "a".repeat(64) })),
  };
  const store = {
    list: vi.fn(async () => structuredClone(disk)),
    load: vi.fn(async (requestId: string) =>
      structuredClone(disk.find((r) => r.request_id === requestId) ?? null),
    ),
    prepare: vi.fn(async (input: TranscriptionJobRequest) => {
      order.push("persist");
      const value = {
        scope: client.scope,
        request_id: crypto.randomUUID(),
        input,
        job: null,
        created_at_ms: 1,
      };
      disk.push(value);
      return structuredClone(value);
    }),
    observe: vi.fn(
      async (requestId: string, snapshot: TranscriptionJobSnapshot) => {
        order.push("observe");
        const value = disk.find((r) => r.request_id === requestId)!;
        value.job = snapshot;
        return structuredClone(value);
      },
    ),
  };
  const scope = {
    client,
    run: async <T>(operation: () => Promise<T>) => operation(),
    getSnapshot: () => ({ state }),
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  } as unknown as VaultScope;
  return {
    controller: new TranscriptionSubmissions(scope, store),
    client,
    api,
    store,
    disk,
    order,
    job,
    setState(value: string) {
      state = value;
      for (const listener of listeners) listener();
    },
  };
}

describe("saved transcription submissions", () => {
  it("persists exact absent-only source and model before dispatch", async () => {
    const f = fixture();
    await f.controller.transcribe("note");
    expect(f.order).toEqual(["persist", "dispatch", "observe"]);
    expect(f.disk[0].input).toEqual({
      session_id: "note",
      expected_recording_sha256: "a".repeat(64),
      expected_transcript: "absent",
      model: "original",
    });
    expect(f.api.submit).toHaveBeenCalledWith(f.disk[0].input, {
      requestId: f.disk[0].request_id,
    });
  });
  it("retains unknown requests and recovers the same UUID and frozen input explicitly", async () => {
    const f = fixture();
    f.api.submit.mockRejectedValueOnce(new Error("response lost"));
    await expect(f.controller.transcribe("note")).rejects.toThrow(
      "response lost",
    );
    const original = structuredClone(f.disk[0]);
    f.api.policy.mockResolvedValue({
      batch_enabled: true,
      availability: "ready",
      model: "changed",
      live_enabled: false,
    });
    await expect(f.controller.transcribe("note")).rejects.toThrow(
      "Recover or finish",
    );
    expect(f.api.submit).toHaveBeenCalledTimes(1);
    await f.controller.resume(original.request_id);
    expect(f.api.submit).toHaveBeenLastCalledWith(original.input, {
      requestId: original.request_id,
    });
    expect(f.api.policy).toHaveBeenCalledTimes(1);
    expect(f.client.audioMetadata).toHaveBeenCalledTimes(1);
  });
  it("refreshes acknowledged requests without dispatching inference again", async () => {
    const f = fixture();
    await f.controller.transcribe("note");
    await f.controller.resume(f.disk[0].request_id);
    expect(f.api.submit).toHaveBeenCalledTimes(1);
    expect(f.api.get).toHaveBeenCalledWith("job", undefined);
  });
  it("refuses foreign receipts", async () => {
    const f = fixture();
    await f.controller.transcribe("note");
    f.disk[0].scope = "other-account";
    await expect(f.controller.resume(f.disk[0].request_id)).rejects.toThrow(
      "not found",
    );
    expect(f.api.get).not.toHaveBeenCalled();
  });
  it("retains preparation without dispatch after a disconnect/reconnect ABA", async () => {
    const f = fixture();
    const prepare = f.store.prepare.getMockImplementation()!;
    f.store.prepare.mockImplementation(async (input) => {
      const value = await prepare(input);
      f.setState("reconnecting");
      f.setState("connected");
      return value;
    });
    await expect(f.controller.transcribe("note")).rejects.toThrow(
      "no longer available",
    );
    expect(f.disk).toHaveLength(1);
    expect(f.api.submit).not.toHaveBeenCalled();
  });
  it("checks aborted work after persisting the request", async () => {
    const f = fixture();
    const abort = new AbortController();
    const prepare = f.store.prepare.getMockImplementation()!;
    f.store.prepare.mockImplementation(async (input) => {
      const value = await prepare(input);
      abort.abort();
      return value;
    });
    await expect(
      f.controller.transcribe("note", { signal: abort.signal }),
    ).rejects.toThrow("no longer available");
    expect(f.disk).toHaveLength(1);
    expect(f.api.submit).not.toHaveBeenCalled();
  });
  it("requires the complete policy/job capabilities before reading any source", async () => {
    const f = fixture();
    f.client.features = ["batch_transcription_jobs"];
    await expect(f.controller.transcribe("note")).rejects.toThrow(
      "does not support",
    );
    expect(f.api.policy).not.toHaveBeenCalled();
    expect(f.client.audioMetadata).not.toHaveBeenCalled();
    expect(f.store.prepare).not.toHaveBeenCalled();
  });
  it("never prepares while the engine is unavailable", async () => {
    const f = fixture();
    f.api.policy.mockResolvedValue({
      batch_enabled: false,
      availability: "unavailable",
      model: "original",
      live_enabled: false,
    });
    await expect(f.controller.transcribe("note")).rejects.toThrow("not ready");
    expect(f.store.prepare).not.toHaveBeenCalled();
  });
});
