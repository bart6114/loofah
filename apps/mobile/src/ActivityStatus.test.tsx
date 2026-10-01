import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ActivityStatus } from "./ActivityStatus";
import type { Snapshot } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const snapshot: Snapshot = {
  vault: {
    local_path: "/app/Library/Application Support/io.loofah.notes/vault",
    icloud_path: null,
  },
  sessions: ["Running", "Queued", "Paused", "Failed"].map((title) => ({
    id: title.toLowerCase(),
    title: `${title} note`,
    created_at: "2026-09-27",
    has_transcript_words: false,
  })),
  recording: {
    session_id: null,
    elapsed_seconds: 0,
    interrupted: false,
    error: null,
  },
  jobs: [],
  model: {
    ready: true,
    downloading: false,
    phase: "ready",
    downloaded_bytes: 0,
    total_bytes: 0,
    model_id: "test-model",
    revision: "main",
  },
  summary: { available: true, reason: null },
  sync: {
    connected: false,
    state: "idle",
    pending: 0,
    error: null,
    conflicts: [],
  },
  settings: {
    transcription_model: "parakeet-v3",
    summary_provider: "none",
    summary_language: "nl",
    summary_model: "",
    summary_base_url: "",
    providers: {},
    has_api_key: false,
  },
};
const jobs: Snapshot["jobs"] = [
  {
    session_id: "running",
    kind: "transcribe",
    state: "running",
    progress: 0.4,
    error: null,
  },
  {
    session_id: "queued",
    kind: "summary",
    state: "queued",
    progress: 0,
    error: null,
  },
  {
    session_id: "paused",
    kind: "transcribe",
    state: "paused",
    progress: 0.6,
    error: null,
  },
  {
    session_id: "failed",
    kind: "summary",
    state: "failed",
    progress: 0.5,
    error: "The source changed while summarizing.",
  },
];

function showActivity(
  data: Snapshot,
  onSelectSession = vi.fn(),
  onOpenTranscriptionSettings?: () => void,
) {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { mutations: { retry: false } } })
      }
    >
      <ActivityStatus
        snapshot={data}
        onSelectSession={onSelectSession}
        onOpenTranscriptionSettings={onOpenTranscriptionSettings}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("global background activity", () => {
  it("stays hidden with no jobs and does not duplicate recording controls", () => {
    showActivity({
      ...snapshot,
      recording: { ...snapshot.recording, session_id: "running" },
    });
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("shows all jobs, progress and errors and opens their note", () => {
    const select = vi.fn();
    showActivity({ ...snapshot, jobs }, select);
    const trigger = screen.getByRole("button", {
      name: "Transcribing…, 4 background activities, 1 needs attention",
    });
    expect(trigger.textContent).toContain("Transcribing…");
    expect(screen.getByRole("status").textContent).toBe("Transcribing…");
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(trigger);
    expect(
      screen.getByRole("dialog", { name: "Background activity" }),
    ).toBeTruthy();
    expect(screen.getByText("Transcribing")).toBeTruthy();
    expect(screen.getByText("Summary queued")).toBeTruthy();
    expect(screen.getByText("Transcription paused")).toBeTruthy();
    expect(screen.getByText("Summary failed")).toBeTruthy();
    expect(
      screen.getByText("The source changed while summarizing."),
    ).toBeTruthy();
    expect(
      (
        screen.getByRole("progressbar", {
          name: "transcription progress for Running note",
        }) as HTMLProgressElement
      ).value,
    ).toBe(0.4);
    fireEvent.click(screen.getByRole("button", { name: "Failed note" }));
    expect(select).toHaveBeenCalledWith("failed");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("uses existing pause, resume and retry commands for the right note", async () => {
    showActivity({ ...snapshot, jobs });
    fireEvent.click(
      screen.getByRole("button", { name: /4 background activities/ }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Pause transcription for Running note",
      }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_cancel_job", {
        sessionId: "running",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Pause summary for Queued note" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_cancel_job", {
        sessionId: "queued",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Resume transcription for Paused note",
      }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_transcribe", {
        sessionId: "paused",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Retry summary for Failed note" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_summarize", {
        sessionId: "failed",
      }),
    );
  });

  it("retries a failed title independently from its saved summary", async () => {
    showActivity({
      ...snapshot,
      jobs: [{ ...jobs[3], kind: "title", error: "Title provider failed" }],
    });
    fireEvent.click(
      screen.getByRole("button", {
        name: "Needs attention, 1 background activity, 1 needs attention",
      }),
    );
    expect(screen.getByText("Title failed")).toBeTruthy();
    expect(screen.getByText("Retry a failed job to continue.")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Retry title for Failed note" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_generate_title", {
        sessionId: "failed",
      }),
    );
    expect(invoke).not.toHaveBeenCalledWith(
      "mobile_summarize",
      expect.anything(),
    );
  });

  it("opens model setup instead of retrying transcription without a model", () => {
    const openSettings = vi.fn();
    showActivity(
      {
        ...snapshot,
        model: { ...snapshot.model, ready: false },
        jobs: [
          {
            ...jobs[3],
            kind: "transcribe",
            error:
              "Download the transcription model, then retry this recording",
          },
        ],
      },
      vi.fn(),
      openSettings,
    );
    fireEvent.click(
      screen.getByRole("button", { name: /Needs attention, 1 background/ }),
    );
    expect(
      screen.getByText("Download the transcription model to continue."),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: /Retry transcription/ }),
    ).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Set up transcription" }),
    );
    expect(openSettings).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("hides the old download error after the model is installed", () => {
    showActivity({
      ...snapshot,
      jobs: [
        {
          ...jobs[3],
          kind: "transcribe",
          error: "Download the transcription model, then retry this recording",
        },
      ],
    });
    fireEvent.click(
      screen.getByRole("button", { name: /Needs attention, 1 background/ }),
    );
    expect(
      screen.queryByText(/Download the transcription model, then retry/),
    ).toBeNull();
    expect(
      screen.getByRole("button", { name: /Retry transcription/ }),
    ).toBeTruthy();
  });

  it("offers model setup for paused transcription after the model is removed", () => {
    const openSettings = vi.fn();
    showActivity(
      {
        ...snapshot,
        model: { ...snapshot.model, ready: false },
        jobs: [jobs[2]],
      },
      vi.fn(),
      openSettings,
    );
    fireEvent.click(
      screen.getByRole("button", { name: /Transcription paused/ }),
    );
    expect(
      screen.getByText("Download the transcription model to continue."),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: /Resume transcription/ }),
    ).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Set up transcription" }),
    );
    expect(openSettings).toHaveBeenCalledOnce();
  });

  it("shows only the attention icon and dismisses all failed jobs", async () => {
    showActivity({
      ...snapshot,
      jobs: [jobs[3], { ...jobs[3], session_id: "paused", kind: "title" }],
    });
    const trigger = screen.getByRole("button", {
      name: "Needs attention, 2 background activities, 2 need attention",
    });
    expect(trigger.textContent).toBe("");
    expect(trigger.querySelector("svg")).toBeTruthy();
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss all" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_dismiss_failed_jobs", {}),
    );
  });

  it("keeps mutation failures visible and actions disabled while pending", async () => {
    let reject!: (reason: unknown) => void;
    vi.mocked(invoke).mockReturnValueOnce(
      new Promise((_, rejectRequest) => {
        reject = rejectRequest;
      }),
    );
    showActivity({ ...snapshot, jobs: [jobs[2]] });
    fireEvent.click(
      screen.getByRole("button", {
        name: "Transcription paused, 1 background activity",
      }),
    );
    const resume = screen.getByRole("button", {
      name: "Resume transcription for Paused note",
    }) as HTMLButtonElement;
    fireEvent.click(resume);
    await waitFor(() => expect(resume.disabled).toBe(true));
    reject("Recording changed; existing transcript preserved");
    expect((await screen.findByRole("alert")).textContent).toContain(
      "existing transcript preserved",
    );
    await waitFor(() => expect(resume.disabled).toBe(false));
  });

  it("shows model download progress and verification globally", () => {
    const view = showActivity({
      ...snapshot,
      model: {
        ...snapshot.model,
        ready: false,
        downloading: true,
        phase: "downloading",
        downloaded_bytes: 25,
        total_bytes: 100,
      },
    });
    fireEvent.click(
      screen.getByRole("button", {
        name: "Downloading model…, 1 background activity",
      }),
    );
    expect(screen.getByText("Downloading · 25%")).toBeTruthy();
    expect((screen.getByRole("progressbar") as HTMLProgressElement).value).toBe(
      0.25,
    );
    view.unmount();
    showActivity({
      ...snapshot,
      model: { ...snapshot.model, phase: "verifying" },
    });
    fireEvent.click(
      screen.getByRole("button", {
        name: "Verifying model…, 1 background activity",
      }),
    );
    expect(screen.getByText("Verifying download…")).toBeTruthy();
    expect(screen.getByRole("progressbar").hasAttribute("value")).toBe(false);
  });

  it("dismisses on Escape and outside taps, restoring focus on Escape", () => {
    showActivity({ ...snapshot, jobs: [jobs[0]] });
    const trigger = screen.getByRole("button", {
      name: /Transcribing…, 1 background activity/,
    });
    fireEvent.click(trigger);
    expect(document.activeElement).toBe(screen.getByRole("dialog"));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    fireEvent.click(trigger);
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("follows transcription with summary and title phases without announcing progress changes", () => {
    const view = showActivity({ ...snapshot, jobs: [jobs[0]] });
    expect(screen.getByRole("status").textContent).toBe("Transcribing…");
    view.rerender(
      <QueryClientProvider client={new QueryClient()}>
        <ActivityStatus
          snapshot={{ ...snapshot, jobs: [{ ...jobs[0], progress: 0.7 }] }}
          onSelectSession={vi.fn()}
        />
      </QueryClientProvider>,
    );
    expect(screen.getByRole("status").textContent).toBe("Transcribing…");
    fireEvent.click(screen.getByRole("button", { name: /Transcribing…/ }));
    expect(screen.getByText("70%")).toBeTruthy();
    view.rerender(
      <QueryClientProvider client={new QueryClient()}>
        <ActivityStatus
          snapshot={{
            ...snapshot,
            jobs: [{ ...jobs[1], state: "running", progress: 0.95 }],
          }}
          onSelectSession={vi.fn()}
        />
      </QueryClientProvider>,
    );
    expect(screen.getByRole("status").textContent).toBe("Generating summary…");
    expect(screen.queryByText("95%")).toBeNull();
    expect(screen.getByRole("progressbar").hasAttribute("value")).toBe(false);
    view.rerender(
      <QueryClientProvider client={new QueryClient()}>
        <ActivityStatus
          snapshot={{
            ...snapshot,
            jobs: [
              { ...jobs[1], kind: "title", state: "running", progress: 0.95 },
            ],
          }}
          onSelectSession={vi.fn()}
        />
      </QueryClientProvider>,
    );
    expect(screen.getByRole("status").textContent).toBe("Generating title…");
    expect(screen.queryByText("95%")).toBeNull();
  });

  it("prioritizes active work over old failures while keeping attention visible", () => {
    showActivity({
      ...snapshot,
      jobs: [jobs[3], { ...jobs[1], state: "running" }],
    });
    const trigger = screen.getByRole("button", {
      name: "Generating summary…, 2 background activities, 1 needs attention",
    });
    expect(trigger.textContent).toContain("Generating summary…");
    expect(trigger.querySelector(".activity-warning")).toBeTruthy();
    fireEvent.click(trigger);
    expect(screen.getByText("Summary failed")).toBeTruthy();
  });

  it("reports queued and paused work without showing a made-up percentage", () => {
    const view = showActivity({ ...snapshot, jobs: [jobs[1], jobs[3]] });
    expect(screen.getByRole("status").textContent).toBe("Summary queued");
    fireEvent.click(screen.getByRole("button", { name: /Summary queued/ }));
    expect(screen.queryByText("0%")).toBeNull();
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(
      screen.getByText(/Queued work starts while Loofah is open/),
    ).toBeTruthy();
    view.rerender(
      <QueryClientProvider client={new QueryClient()}>
        <ActivityStatus
          snapshot={{ ...snapshot, jobs: [jobs[2], jobs[3]] }}
          onSelectSession={vi.fn()}
        />
      </QueryClientProvider>,
    );
    expect(screen.getByRole("status").textContent).toBe("Transcription paused");
    expect(screen.getByText("60%")).toBeTruthy();
    expect(screen.getByText("Resume a paused job to continue.")).toBeTruthy();
  });

  it("explains recording pauses and foreground processing", () => {
    showActivity({
      ...snapshot,
      recording: { ...snapshot.recording, session_id: "running" },
      jobs: [jobs[1]],
    });
    expect(screen.getByRole("status").textContent).toBe("Processing paused");
    fireEvent.click(screen.getByRole("button", { name: /Processing paused/ }));
    expect(screen.getByText(/Processing waits while you record/)).toBeTruthy();
  });

  it("keeps saving the recording visible before a transcription job appears", () => {
    showActivity({
      ...snapshot,
      recording: { ...snapshot.recording, stopping: true },
    });
    expect(screen.getByRole("status").textContent).toBe("Saving recording…");
    fireEvent.click(screen.getByRole("button", { name: /Saving recording…/ }));
    expect(screen.getByText("Saving audio…")).toBeTruthy();
    expect(screen.getByRole("progressbar").hasAttribute("value")).toBe(false);
  });
});
