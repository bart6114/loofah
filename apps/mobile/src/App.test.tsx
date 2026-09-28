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

import { listModels } from "@hypr/ai-providers";

import type { Session, Snapshot } from "./api";
import { App, Detail, Editor, Setup, SummarySettings } from "./App";
vi.stubGlobal("scrollTo", vi.fn());
vi.mock("@tauri-apps/plugin-http", () => ({ fetch: vi.fn() }));
vi.mock("@hypr/ai-providers", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@hypr/ai-providers")>()),
  listModels: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn(),
}));
const session: Session = {
  id: "one",
  title: "Original",
  notes: "Original notes",
  summary: null,
  transcript: [],
  tasks: [],
  audio_url: null,
};
const snapshot: Snapshot = {
  vault: {
    local_path: "/app/Library/Application Support/io.loofah.mobile/vault",
    icloud_path: null,
  },
  sessions: [],
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
  summary: { available: false, reason: "Local model unavailable" },
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
function wrapper({ children }: { children: React.ReactNode }) {
  return (
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { mutations: { retry: false } } })
      }
    >
      {children}
    </QueryClientProvider>
  );
}
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listModels).mockReset();
  vi.mocked(listModels).mockResolvedValue({
    models: [],
    ignored: [],
    metadata: {},
  });
  vi.mocked(invoke).mockResolvedValue(undefined);
});
afterEach(cleanup);
describe("iPhone navigation", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_snapshot"
        ? {
            ...snapshot,
            sessions: [
              {
                id: "one",
                title: "Original",
                created_at: "2026-09-26",
                has_transcript_words: false,
              },
            ],
          }
        : session,
    );
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
      configurable: true,
      value: function (this: HTMLDialogElement) {
        this.setAttribute("open", "");
      },
    });
  });
  afterEach(() => vi.restoreAllMocks());
  it("keeps a draft until the user explicitly discards it", async () => {
    render(<App />, { wrapper });
    fireEvent.click(await screen.findByRole("button", { name: /Original/ }));
    fireEvent.change(await screen.findByLabelText("Notes"), {
      target: { value: "Keep my draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Back to notes" }));
    expect(screen.getByRole("dialog").textContent).toContain(
      "haven’t been saved",
    );
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect((screen.getByLabelText("Notes") as HTMLTextAreaElement).value).toBe(
      "Keep my draft",
    );
    fireEvent.click(screen.getByRole("button", { name: "Back to notes" }));
    fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(await screen.findByRole("heading", { name: "Notes" })).toBeTruthy();
    expect(screen.queryByLabelText("Notes")).toBeNull();
  });
  it("restores a deleted note without interrupting another draft", async () => {
    let deleted = false;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "mobile_snapshot")
        return {
          ...snapshot,
          sessions: [
            ...(!deleted
              ? [
                  {
                    id: "one",
                    title: "Original",
                    created_at: "2026-09-26",
                    has_transcript_words: false,
                  },
                ]
              : []),
            {
              id: "two",
              title: "Second note",
              created_at: "2026-09-26",
              has_transcript_words: false,
            },
          ],
        };
      if (command === "mobile_delete_session") {
        deleted = true;
        return;
      }
      if (command === "mobile_restore_session") {
        deleted = false;
        return;
      }
      return (args as { sessionId?: string })?.sessionId === "two"
        ? { ...session, id: "two", title: "Second note" }
        : session;
    });
    render(<App />, { wrapper });
    fireEvent.click(await screen.findByRole("button", { name: /Original/ }));
    fireEvent.click(
      await screen.findByRole("button", { name: "Note actions" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Delete note" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete note" }));
    await screen.findByText("Note moved to trash");
    fireEvent.click(await screen.findByRole("button", { name: /Second note/ }));
    fireEvent.change(await screen.findByLabelText("Notes"), {
      target: { value: "Keep this draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() =>
      expect(screen.queryByText("Note moved to trash")).toBeNull(),
    );
    expect(screen.getByLabelText("Notes")).toHaveProperty(
      "value",
      "Keep this draft",
    );
    expect(screen.getByLabelText("Note title")).toHaveProperty(
      "value",
      "Second note",
    );
  });
  it("returns from settings to the same selected note", async () => {
    render(<App />, { wrapper });
    fireEvent.click(await screen.findByRole("button", { name: /Original/ }));
    await screen.findByLabelText("Notes");
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByLabelText("Transcription model");
    fireEvent.click(screen.getByRole("button", { name: "Back to note" }));
    expect(await screen.findByLabelText("Notes")).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Notes" })).toBeNull();
  });
  it("explains an empty search and allows clearing it", async () => {
    render(<App />, { wrapper });
    fireEvent.change(await screen.findByRole("searchbox"), {
      target: { value: "missing" },
    });
    expect(screen.getByRole("status").textContent).toContain(
      "No matching notes",
    );
    fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
    expect(screen.getByRole("button", { name: /Original/ })).toBeTruthy();
  });
  it("does not offer iCloud audio or transcription for an empty local note", async () => {
    render(<Detail id="one" snapshot={snapshot} onDirty={() => {}} />, {
      wrapper,
    });
    await screen.findByLabelText("Notes");
    expect(screen.queryByRole("button", { name: "Download audio" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Transcribe" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Transcript" })).toBeNull();
  });
});
describe("desktop-aligned mobile note views", () => {
  it("opens an existing summary, shows inline tasks and toggles attachments without leaving the view", async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...session,
      summary: "# Decisions\n- [ ] Ship Friday",
      has_audio: true,
      audio_url: "asset://audio",
      tasks: [{ id: "task", title: "Ship Friday", completed: false }],
      attachments: [
        {
          name: "Agenda.pdf",
          relative_path: "attachments/agenda.pdf",
          url: "asset://agenda",
        },
      ],
    });
    render(<Detail id="one" snapshot={snapshot} onDirty={() => {}} />, {
      wrapper,
    });
    await screen.findByRole("heading", { name: "Decisions" });
    expect(
      screen
        .getByRole("tab", { name: "Summary" })
        .getAttribute("aria-selected"),
    ).toBe("true");
    expect(screen.queryByRole("tab", { name: "Tasks" })).toBeNull();
    expect(screen.getByRole("checkbox")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Record" })).toBeNull();
    expect(screen.getByLabelText("Note recording")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Attachments" }));
    fireEvent.click(screen.getByRole("button", { name: "Agenda.pdf" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_open_attachment", {
        sessionId: "one",
        relativePath: "attachments/agenda.pdf",
      }),
    );
    expect(
      screen
        .getByRole("tab", { name: "Summary" })
        .getAttribute("aria-selected"),
    ).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Attachments" }));
    expect(screen.queryByRole("button", { name: "Agenda.pdf" })).toBeNull();
  });
  it("preserves a note draft across view changes and uses arrow-key navigation", async () => {
    vi.mocked(invoke).mockResolvedValue(session);
    render(<Detail id="one" snapshot={snapshot} onDirty={() => {}} />, {
      wrapper,
    });
    fireEvent.change(await screen.findByLabelText("Notes"), {
      target: { value: "Keep this draft" },
    });
    fireEvent.keyDown(screen.getByRole("tab", { name: "Note" }), {
      key: "ArrowLeft",
    });
    expect(
      screen
        .getByRole("tab", { name: "Summary" })
        .getAttribute("aria-selected"),
    ).toBe("true");
    fireEvent.click(screen.getByRole("tab", { name: "Note" }));
    expect((screen.getByLabelText("Notes") as HTMLTextAreaElement).value).toBe(
      "Keep this draft",
    );
    fireEvent.click(screen.getByRole("button", { name: "Save notes" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "mobile_update_session",
        expect.objectContaining({
          notes: "Keep this draft",
          expectedNotes: "Original notes",
        }),
      ),
    );
  });
  it("does not save a dirty note when using recording, attachment or view controls", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session" ? session : undefined,
    );
    render(<Detail id="one" snapshot={snapshot} onDirty={() => {}} />, {
      wrapper,
    });
    fireEvent.change(await screen.findByLabelText("Notes"), {
      target: { value: "Unsaved draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_start_recording", {
        sessionId: "one",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Attachments" }));
    fireEvent.click(screen.getByRole("tab", { name: "Summary" }));
    expect(
      vi
        .mocked(invoke)
        .mock.calls.some(([command]) => command === "mobile_update_session"),
    ).toBe(false);
    expect((screen.getByLabelText("Notes") as HTMLTextAreaElement).value).toBe(
      "Unsaved draft",
    );
  });
  it("renders summary task checkboxes once without duplicating the canonical task list", async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...session,
      summary: "## Actions\n- [ ] Ship Friday",
      tasks: [{ id: "task", title: "Ship Friday", completed: false }],
    });
    render(<Detail id="one" snapshot={snapshot} onDirty={() => {}} />, {
      wrapper,
    });
    await screen.findByRole("heading", { name: "Actions" });
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    expect(screen.getAllByText("Ship Friday")).toHaveLength(1);
  });
  it("keeps title editing and save available from an existing summary", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session"
        ? { ...session, summary: "Saved summary" }
        : undefined,
    );
    render(<Detail id="one" snapshot={snapshot} onDirty={() => {}} />, {
      wrapper,
    });
    await screen.findByText("Saved summary");
    fireEvent.change(screen.getByLabelText("Note title"), {
      target: { value: "A long wrapped meeting title" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save notes" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "mobile_update_session",
        expect.objectContaining({
          title: "A long wrapped meeting title",
          notes: "Original notes",
          expectedTitle: "Original",
        }),
      ),
    );
  });
  it("hides idle capture in settings but retains active recording controls", async () => {
    let state = snapshot;
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_snapshot" ? state : session,
    );
    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}>
        <App />
      </QueryClientProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Settings" }));
    await screen.findByLabelText("Transcription model");
    expect(screen.queryByRole("button", { name: "Record" })).toBeNull();
    state = {
      ...snapshot,
      recording: {
        ...snapshot.recording,
        session_id: "one",
        elapsed_seconds: 42,
      },
    };
    await client.invalidateQueries({ queryKey: ["mobile", "snapshot"] });
    expect(await screen.findByRole("button", { name: "Stop" })).toBeTruthy();
    expect(screen.queryByText(/beta/i)).toBeNull();
  });
});
describe("mobile summary content", () => {
  const enabled = { ...snapshot, summary: { available: true, reason: null } };
  it.each([
    ["a note", "Saved meeting notes", [], false],
    ["a transcript", "", [{ text: "Hello", start: 0, end: 1 }], false],
    ["empty content", "   ", [], true],
  ] as const)("handles %s", async (_, notes, transcript, disabled) => {
    vi.mocked(invoke).mockResolvedValue({ ...session, notes, transcript });
    render(<Detail id="one" snapshot={enabled} onDirty={() => {}} />, {
      wrapper,
    });
    await screen.findByLabelText("Notes");
    fireEvent.click(screen.getByRole("tab", { name: "Summary" }));
    const button = screen.getByRole("button", {
      name: "Create summary",
    }) as HTMLButtonElement;
    expect(button.disabled).toBe(disabled);
    if (disabled)
      expect(
        screen.getByText("Add a note or transcript to create a summary."),
      ).toBeTruthy();
    else {
      fireEvent.click(button);
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith("mobile_summarize", {
          sessionId: "one",
        }),
      );
    }
  });
  it("requires saving a draft before summarizing the stored note", async () => {
    vi.mocked(invoke).mockResolvedValue(session);
    render(<Detail id="one" snapshot={enabled} onDirty={() => {}} />, {
      wrapper,
    });
    fireEvent.change(await screen.findByLabelText("Notes"), {
      target: { value: "Unsaved note" },
    });
    fireEvent.click(screen.getByRole("tab", { name: "Summary" }));
    expect(
      (
        screen.getByRole("button", {
          name: "Create summary",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    expect(
      screen.getByText("Save your notes before creating a summary."),
    ).toBeTruthy();
  });
});
describe("mobile processing views", () => {
  const processingJob = (
    kind: Snapshot["jobs"][number]["kind"],
    state: Snapshot["jobs"][number]["state"],
    session_id = "one",
  ): Snapshot["jobs"][number] => ({
    session_id,
    kind,
    state,
    progress: 0,
    error: state === "failed" ? "Provider failed" : null,
  });
  const selectedTab = (name: string) =>
    screen.getByRole("tab", { name }).getAttribute("aria-selected");

  it("keeps the record button disabled while the stopped recording is finalizing", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_snapshot"
        ? {
            ...snapshot,
            recording: {
              ...snapshot.recording,
              session_id: "one",
              stopping: true,
            },
          }
        : session,
    );
    render(<App />, { wrapper });
    const button = await screen.findByRole("button", { name: "Saving…" });
    expect((button as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "Record" })).toBeNull();
  });

  it.each([
    ["transcribe", "Transcript", "Transcription queued…"],
    ["summary", "Summary", "Summary queued…"],
    ["title", "Summary", "Summary queued…"],
  ] as const)(
    "opens an in-flight %s job in %s",
    async (kind, tab, placeholder) => {
      vi.mocked(invoke).mockImplementation(async (command) =>
        command === "mobile_session" ? session : undefined,
      );
      render(
        <Detail
          id="one"
          snapshot={{ ...snapshot, jobs: [processingJob(kind, "queued")] }}
          onDirty={() => {}}
        />,
        { wrapper },
      );
      await screen.findByLabelText("Note title");
      expect(selectedTab(tab)).toBe("true");
      expect(screen.getByText(placeholder)).toBeTruthy();
      expect(screen.queryByRole("button", { name: "Transcribe" })).toBeNull();
      expect(
        screen.queryByRole("button", { name: "Create summary" }),
      ).toBeNull();
    },
  );

  it("follows finishing, transcription and summary without overriding a manual tab in either phase", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session" ? session : undefined,
    );
    const onDirty = vi.fn();
    const view = render(
      <Detail
        id="one"
        snapshot={{
          ...snapshot,
          recording: {
            ...snapshot.recording,
            session_id: "one",
            stopping: true,
          },
        }}
        onDirty={onDirty}
      />,
      { wrapper },
    );
    await screen.findByText("Finishing recording…");
    expect(selectedTab("Transcript")).toBe("true");
    const update = (jobs: Snapshot["jobs"]) =>
      view.rerender(
        <Detail id="one" snapshot={{ ...snapshot, jobs }} onDirty={onDirty} />,
      );
    update([processingJob("transcribe", "queued")]);
    expect(await screen.findByText("Transcription queued…")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Note" }));
    update([processingJob("transcribe", "running")]);
    expect(selectedTab("Note")).toBe("true");
    update([processingJob("summary", "queued")]);
    await waitFor(() => expect(selectedTab("Summary")).toBe("true"));
    expect(screen.getByText("Summary queued…")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Note" }));
    update([processingJob("summary", "running")]);
    expect(selectedTab("Note")).toBe("true");
    update([processingJob("title", "running")]);
    expect(selectedTab("Note")).toBe("true");
  });

  it("switches to a completed summary when polling skips the summary job", async () => {
    let currentSession = session;
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session" ? currentSession : undefined,
    );
    const client = new QueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Detail
          id="one"
          snapshot={{
            ...snapshot,
            jobs: [processingJob("transcribe", "queued")],
          }}
          onDirty={() => {}}
        />
      </QueryClientProvider>,
    );
    await screen.findByText("Transcription queued…");
    currentSession = { ...session, summary: "Ready summary" };
    view.rerender(
      <QueryClientProvider client={client}>
        <Detail id="one" snapshot={snapshot} onDirty={() => {}} />
      </QueryClientProvider>,
    );
    await client.invalidateQueries({ queryKey: ["mobile", "session", "one"] });
    await screen.findByText("Ready summary");
    expect(selectedTab("Summary")).toBe("true");
  });

  it("preserves a manual tab after an observed summary job completes", async () => {
    let currentSession = session;
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session" ? currentSession : undefined,
    );
    const client = new QueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <Detail
          id="one"
          snapshot={{
            ...snapshot,
            jobs: [processingJob("summary", "running")],
          }}
          onDirty={() => {}}
        />
      </QueryClientProvider>,
    );
    await screen.findByText("Generating summary…");
    fireEvent.click(screen.getByRole("tab", { name: "Note" }));
    view.rerender(
      <QueryClientProvider client={client}>
        <Detail id="one" snapshot={snapshot} onDirty={() => {}} />
      </QueryClientProvider>,
    );
    currentSession = { ...session, summary: "Ready summary" };
    await client.invalidateQueries({ queryKey: ["mobile", "session", "one"] });
    await waitFor(() =>
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(([command]) => command === "mobile_session")
          .length,
      ).toBeGreaterThan(1),
    );
    expect(selectedTab("Note")).toBe("true");
  });

  it("leaves dirty notes and title drafts visible when processing advances", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session" ? session : undefined,
    );
    const view = render(
      <Detail id="one" snapshot={snapshot} onDirty={() => {}} />,
      { wrapper },
    );
    fireEvent.change(await screen.findByLabelText("Note title"), {
      target: { value: "My draft title" },
    });
    fireEvent.change(screen.getByLabelText("Notes"), {
      target: { value: "My draft notes" },
    });
    view.rerender(
      <Detail
        id="one"
        snapshot={{
          ...snapshot,
          jobs: [processingJob("transcribe", "queued")],
        }}
        onDirty={() => {}}
      />,
    );
    expect(selectedTab("Note")).toBe("true");
    view.rerender(
      <Detail
        id="one"
        snapshot={{ ...snapshot, jobs: [processingJob("summary", "running")] }}
        onDirty={() => {}}
      />,
    );
    expect(selectedTab("Note")).toBe("true");
    expect(
      (screen.getByLabelText("Note title") as HTMLTextAreaElement).value,
    ).toBe("My draft title");
    expect((screen.getByLabelText("Notes") as HTMLTextAreaElement).value).toBe(
      "My draft notes",
    );
  });

  it("keeps a manual tab through pause and resume, then follows a new processing cycle", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session" ? session : undefined,
    );
    const onDirty = vi.fn();
    const update = (jobs: Snapshot["jobs"]) =>
      view.rerender(
        <Detail id="one" snapshot={{ ...snapshot, jobs }} onDirty={onDirty} />,
      );
    const view = render(
      <Detail
        id="one"
        snapshot={{
          ...snapshot,
          jobs: [processingJob("transcribe", "queued")],
        }}
        onDirty={onDirty}
      />,
      { wrapper },
    );
    await screen.findByText("Transcription queued…");
    fireEvent.click(screen.getByRole("tab", { name: "Note" }));
    update([processingJob("transcribe", "paused")]);
    update([processingJob("transcribe", "running")]);
    expect(selectedTab("Note")).toBe("true");
    update([]);
    update([processingJob("transcribe", "queued")]);
    await waitFor(() => expect(selectedTab("Transcript")).toBe("true"));
  });

  it("ignores jobs for another note", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_session" ? session : undefined,
    );
    render(
      <Detail
        id="one"
        snapshot={{
          ...snapshot,
          jobs: [processingJob("transcribe", "running", "two")],
        }}
        onDirty={() => {}}
      />,
      { wrapper },
    );
    await screen.findByLabelText("Note title");
    expect(selectedTab("Note")).toBe("true");
    expect(screen.queryByRole("tab", { name: "Transcript" })).toBeNull();
  });

  it.each(["paused", "failed"] as const)(
    "does not claim a %s job is still processing or duplicate its controls",
    async (state) => {
      vi.mocked(invoke).mockImplementation(async (command) =>
        command === "mobile_session" ? session : undefined,
      );
      render(
        <Detail
          id="one"
          snapshot={{ ...snapshot, jobs: [processingJob("summary", state)] }}
          onDirty={() => {}}
        />,
        { wrapper },
      );
      await screen.findByLabelText("Note title");
      fireEvent.click(screen.getByRole("tab", { name: "Summary" }));
      expect(screen.getByText("No summary yet.")).toBeTruthy();
      expect(screen.queryByText("Generating summary…")).toBeNull();
      expect(screen.queryByText("Provider failed")).toBeNull();
      expect(
        screen.queryByRole("button", { name: /Resume summary|Retry summary/ }),
      ).toBeNull();
    },
  );
});
describe("mobile notes", () => {
  it("keeps the draft and original concurrency baseline when iCloud changes the note", async () => {
    const onDirty = vi.fn();
    const view = render(<Editor session={session} onDirty={onDirty} />, {
      wrapper,
    });
    fireEvent.change(screen.getByLabelText("Notes"), {
      target: { value: "My draft" },
    });
    view.rerender(
      <Editor
        session={{ ...session, title: "From Mac", notes: "From Mac notes" }}
        onDirty={onDirty}
      />,
    );
    expect((screen.getByLabelText("Notes") as HTMLTextAreaElement).value).toBe(
      "My draft",
    );
    vi.mocked(invoke).mockRejectedValueOnce(
      "Notes changed in iCloud. Reload before saving.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Save notes" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_update_session", {
        sessionId: "one",
        title: "Original",
        notes: "My draft",
        expectedTitle: "Original",
        expectedNotes: "Original notes",
      }),
    );
    await screen.findByRole("alert");
    expect((screen.getByLabelText("Notes") as HTMLTextAreaElement).value).toBe(
      "My draft",
    );
  });
  it("refreshes a pristine editor from iCloud", async () => {
    const onDirty = vi.fn();
    const view = render(<Editor session={session} onDirty={onDirty} />, {
      wrapper,
    });
    view.rerender(
      <Editor
        session={{ ...session, notes: "Updated on Mac" }}
        onDirty={onDirty}
      />,
    );
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Notes") as HTMLTextAreaElement).value,
      ).toBe("Updated on Mac"),
    );
  });
});
describe("shared summary languages", () => {
  it.each(["fr", "fr-CA"])(
    "preserves synced %s when saving summary settings",
    async (language) => {
      render(
        <SummarySettings
          snapshot={{
            ...snapshot,
            settings: {
              ...snapshot.settings,
              summary_language: language,
            },
          }}
        />,
        { wrapper },
      );
      expect(
        (screen.getByLabelText("Summary language") as HTMLSelectElement).value,
      ).toBe(language);
      expect(screen.getByRole("option", { name: "French" })).toBeTruthy();
      expect(screen.getByRole("option", { name: "Japanese" })).toBeTruthy();
      fireEvent.click(
        screen.getByRole("button", { name: "Save summary settings" }),
      );
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith("mobile_save_settings", {
          summaryProvider: "none",
          summaryLanguage: language,
          summaryModel: "",
          summaryBaseUrl: "",
          apiKey: null,
        }),
      );
    },
  );
});
describe("synced summary settings", () => {
  it("refreshes pristine settings and preserves a local settings draft", async () => {
    const view = render(<SummarySettings snapshot={snapshot} />, { wrapper });
    view.rerender(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: { ...snapshot.settings, summary_language: "fr" },
        }}
      />,
    );
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Summary language") as HTMLSelectElement).value,
      ).toBe("fr"),
    );
    fireEvent.change(screen.getByLabelText("Summary language"), {
      target: { value: "en" },
    });
    view.rerender(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: { ...snapshot.settings, summary_language: "de" },
        }}
      />,
    );
    expect(
      (screen.getByLabelText("Summary language") as HTMLSelectElement).value,
    ).toBe("en");
  });
});
describe("API summary consent", () => {
  it("saves OpenRouter only after the user explicitly selects and submits it", async () => {
    render(<SummarySettings snapshot={snapshot} />, { wrapper });
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "openrouter" },
    });
    expect(
      screen.getByText(/allow notes and transcripts to be sent/),
    ).toBeTruthy();
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Model ID"), {
      target: { value: "test/model" },
    });
    fireEvent.change(screen.getByLabelText("OpenRouter API key"), {
      target: { value: "test-key" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Save summary settings" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_save_settings", {
        summaryProvider: "openrouter",
        summaryLanguage: "nl",
        summaryModel: "test/model",
        summaryBaseUrl: "https://openrouter.ai/api/v1",
        apiKey: "test-key",
      }),
    );
    await waitFor(() =>
      expect(
        (screen.getByLabelText("OpenRouter API key") as HTMLInputElement).value,
      ).toBe(""),
    );
  });
});
describe("mobile provider connections", () => {
  it("offers desktop API providers and reachable servers without an on-device summary option", () => {
    render(<SummarySettings snapshot={snapshot} />, { wrapper });
    for (const name of [
      "OpenRouter",
      "OpenAI",
      "Anthropic",
      "Google Gemini",
      "Mistral",
      "Azure OpenAI",
      "Azure AI Foundry",
      "Cloudflare Workers AI",
      "Custom",
      "LM Studio",
      "Ollama",
    ])
      expect(screen.getByRole("option", { name })).toBeTruthy();
    expect(screen.queryByRole("option", { name: "On-device" })).toBeNull();
    expect(
      screen.queryByRole("option", { name: "ChatGPT subscription" }),
    ).toBeNull();
    expect(screen.queryByText(/beta/i)).toBeNull();
  });
  it("explains unsupported synced ChatGPT sign-in and requires another selection", () => {
    render(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: {
            ...snapshot.settings,
            summary_provider: "chatgpt_subscription",
            summary_model: "gpt-5",
          },
        }}
      />,
      { wrapper },
    );
    expect(
      screen.getByText(/ChatGPT subscription sign-in is supported on Mac/),
    ).toBeTruthy();
    expect(
      (
        screen.getByRole("button", {
          name: "Save summary settings",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "none" },
    });
    expect(
      (
        screen.getByRole("button", {
          name: "Save summary settings",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false);
  });
  it("loads models explicitly with the selected provider's device credential and saves the selected ID", async () => {
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_provider_api_key" ? "device-key" : undefined,
    );
    vi.mocked(listModels).mockResolvedValue({
      models: ["claude-sonnet-4-6"],
      ignored: [],
      metadata: {},
    });
    render(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: {
            ...snapshot.settings,
            providers: {
              anthropic: {
                base_url: "https://api.anthropic.com/v1",
                has_api_key: true,
              },
            },
          },
        }}
      />,
      { wrapper },
    );
    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "anthropic" },
    });
    expect(invoke).not.toHaveBeenCalled();
    expect(listModels).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    await screen.findByLabelText("Available models");
    expect(invoke).toHaveBeenCalledWith("mobile_provider_api_key", {
      providerId: "anthropic",
    });
    expect(listModels).toHaveBeenCalledWith(
      expect.objectContaining({
        providerId: "anthropic",
        baseUrl: "https://api.anthropic.com/v1",
        apiKey: "device-key",
      }),
    );
    fireEvent.change(screen.getByLabelText("Available models"), {
      target: { value: "claude-sonnet-4-6" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Save summary settings" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_save_settings", {
        summaryProvider: "anthropic",
        summaryModel: "claude-sonnet-4-6",
        summaryBaseUrl: "https://api.anthropic.com/v1",
        summaryLanguage: "nl",
        apiKey: null,
      }),
    );
  });
  it("uses entered keys for discovery without saving and clears them when switching providers", async () => {
    render(<SummarySettings snapshot={snapshot} />, { wrapper });
    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "openai" },
    });
    fireEvent.change(screen.getByLabelText("OpenAI API key"), {
      target: { value: "draft-secret" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    await screen.findByText(
      "This endpoint did not list any models. Enter a model or deployment ID.",
    );
    expect(listModels).toHaveBeenCalledWith(
      expect.objectContaining({ providerId: "openai", apiKey: "draft-secret" }),
    );
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "openrouter" },
    });
    expect(
      (screen.getByLabelText("OpenRouter API key") as HTMLInputElement).value,
    ).toBe("");
  });
  it("refreshes pristine connection fields but preserves drafts and dirty state across synced provider changes", async () => {
    const onDirty = vi.fn();
    const settings = {
      ...snapshot.settings,
      summary_provider: "openai",
      summary_model: "original-model",
      summary_base_url: "https://api.openai.com/v1",
    };
    const view = render(
      <SummarySettings
        snapshot={{ ...snapshot, settings }}
        onDirty={onDirty}
      />,
      { wrapper },
    );
    view.rerender(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: { ...settings, summary_model: "synced-model" },
        }}
        onDirty={onDirty}
      />,
    );
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Model ID") as HTMLInputElement).value,
      ).toBe("synced-model"),
    );
    fireEvent.change(screen.getByLabelText("Model ID"), {
      target: { value: "draft-model" },
    });
    fireEvent.change(screen.getByLabelText("OpenAI API key"), {
      target: { value: "draft-key" },
    });
    view.rerender(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: {
            ...settings,
            summary_provider: "anthropic",
            summary_model: "from-mac",
            summary_base_url: "https://api.anthropic.com/v1",
          },
        }}
        onDirty={onDirty}
      />,
    );
    expect((screen.getByLabelText("Provider") as HTMLSelectElement).value).toBe(
      "openai",
    );
    expect((screen.getByLabelText("Model ID") as HTMLInputElement).value).toBe(
      "draft-model",
    );
    expect(onDirty).toHaveBeenLastCalledWith(true);
    vi.mocked(invoke).mockRejectedValueOnce(new Error("Cannot save settings"));
    fireEvent.click(
      screen.getByRole("button", { name: "Save summary settings" }),
    );
    await screen.findByRole("alert");
    expect(
      (screen.getByLabelText("OpenAI API key") as HTMLInputElement).value,
    ).toBe("draft-key");
    expect(onDirty).toHaveBeenLastCalledWith(true);
  });
  it("treats a cleared device key as empty when the snapshot still reports a saved key", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    render(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: {
            ...snapshot.settings,
            summary_provider: "lmstudio",
            summary_base_url: "http://192.168.1.10:1234/v1",
            has_api_key: true,
          },
        }}
      />,
      { wrapper },
    );
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    await waitFor(() =>
      expect(listModels).toHaveBeenCalledWith(
        expect.objectContaining({ apiKey: "" }),
      ),
    );
  });
  it("redacts entered and Keychain keys from model discovery failures", async () => {
    const key = "secret/key+suffix";
    vi.mocked(invoke).mockResolvedValue(key);
    vi.mocked(listModels).mockRejectedValue(
      new Error(`Failure ${key} and ${encodeURIComponent(key)}`),
    );
    render(
      <SummarySettings
        snapshot={{
          ...snapshot,
          settings: {
            ...snapshot.settings,
            summary_provider: "openai",
            summary_base_url: "https://api.openai.com/v1",
            has_api_key: true,
          },
        }}
      />,
      { wrapper },
    );
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "Failure [redacted] and [redacted]",
    );
    fireEvent.change(screen.getByLabelText("OpenAI API key"), {
      target: { value: key },
    });
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "Failure [redacted] and [redacted]",
    );
  });
  it("retains manual model entry when listing fails and describes local server connectivity", async () => {
    vi.mocked(listModels).mockRejectedValue(new Error("Server unavailable"));
    render(<SummarySettings snapshot={snapshot} />, { wrapper });
    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "lmstudio" },
    });
    expect(
      screen.getByText(/summaries do not run offline on iPhone/),
    ).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Server URL"), {
      target: { value: "http://192.168.1.10:1234/v1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Load models" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "You can enter a model ID below.",
    );
    fireEvent.change(screen.getByLabelText("Model ID"), {
      target: { value: "my-model" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Save summary settings" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "mobile_save_settings",
        expect.objectContaining({
          summaryProvider: "lmstudio",
          summaryModel: "my-model",
          summaryBaseUrl: "http://192.168.1.10:1234/v1",
          apiKey: null,
        }),
      ),
    );
  });
});
describe("model settings", () => {
  it("shows byte progress with a known total and stays indeterminate without one", () => {
    const view = render(
      <Setup
        snapshot={{
          ...snapshot,
          model: {
            ...snapshot.model,
            ready: false,
            downloading: true,
            downloaded_bytes: 500_000_000,
            total_bytes: 1_000_000_000,
          },
        }}
      />,
      { wrapper },
    );
    const progress = screen.getByRole("progressbar", {
      name: "Model download",
    }) as HTMLProgressElement;
    expect(progress.value).toBe(500_000_000);
    expect(progress.max).toBe(1_000_000_000);
    expect(progress.getAttribute("aria-valuetext")).toBe(
      "50% · 500 / 1,000 MB",
    );
    view.rerender(
      <Setup
        snapshot={{
          ...snapshot,
          model: {
            ...snapshot.model,
            ready: false,
            downloading: true,
            downloaded_bytes: 500_000_000,
            total_bytes: 0,
          },
        }}
      />,
    );
    expect(progress.hasAttribute("value")).toBe(false);
    expect(progress.getAttribute("aria-valuetext")).toBe("500 MB downloaded");
  });
  it("offers multilingual and English-only models and selects through the backend", async () => {
    render(<Setup snapshot={snapshot} />, { wrapper });
    const choice = screen.getByLabelText(
      "Transcription model",
    ) as HTMLSelectElement;
    expect(choice.value).toBe("parakeet-v3");
    expect(
      screen.getByRole("option", { name: "Parakeet v2 — English only" }),
    ).toBeTruthy();
    fireEvent.change(choice, { target: { value: "parakeet-v2" } });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_select_model", {
        modelId: "parakeet-v2",
      }),
    );
  });
  it("shows progress immediately from idle, polls while the command is pending, and verifies", async () => {
    const idle: Snapshot = {
      ...snapshot,
      model: {
        ...snapshot.model,
        ready: false,
        phase: "idle",
        total_bytes: 1_000_000_000,
      },
    };
    let latest = idle;
    let finish!: () => void;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "mobile_snapshot") return latest;
      if (command === "mobile_download_model")
        return new Promise<void>((resolve) => {
          finish = resolve;
        });
    });
    render(<Setup snapshot={idle} />, { wrapper });
    fireEvent.click(screen.getByRole("button", { name: "Download model" }));
    await screen.findByRole("progressbar", { name: "Model download" });
    expect(screen.queryByText("Working…")).toBeNull();
    expect(
      (screen.getByLabelText("Transcription model") as HTMLSelectElement)
        .disabled,
    ).toBe(true);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("mobile_snapshot"));
    latest = {
      ...idle,
      model: {
        ...idle.model,
        downloading: true,
        phase: "downloading",
        downloaded_bytes: 250_000_000,
      },
    };
    await screen.findByText("25% · 250 / 1,000 MB");
    latest = { ...latest, model: { ...latest.model, phase: "verifying" } };
    await screen.findByText("Verifying downloaded model…");
    expect(screen.getByRole("progressbar").hasAttribute("value")).toBe(false);
    finish();
  });
  it("shows a download failure and allows retry", async () => {
    const idle: Snapshot = {
      ...snapshot,
      model: { ...snapshot.model, ready: false, phase: "idle" },
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "mobile_snapshot") return idle;
      if (command === "mobile_download_model")
        throw new Error("Network unavailable");
    });
    render(<Setup snapshot={idle} />, { wrapper });
    fireEvent.click(screen.getByRole("button", { name: "Download model" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Network unavailable",
    );
    const retry = await screen.findByRole("button", { name: "Retry download" });
    expect((retry as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(retry);
    await waitFor(() =>
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(
            ([command]) => command === "mobile_download_model",
          ),
      ).toHaveLength(2),
    );
  });
  it("deletes a downloaded model through the command", async () => {
    render(<Setup snapshot={snapshot} />, { wrapper });
    fireEvent.click(
      screen.getByRole("button", { name: "Delete downloaded model" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("mobile_delete_model", {}),
    );
  });
  it("allows model deletion after processing is paused", () => {
    render(
      <Setup
        snapshot={{
          ...snapshot,
          jobs: [
            {
              session_id: "one",
              kind: "transcribe",
              state: "paused",
              progress: 0.5,
              error: null,
            },
          ],
        }}
      />,
      { wrapper },
    );
    expect(
      (
        screen.getByRole("button", {
          name: "Delete downloaded model",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false);
  });
  it.each(["recording", "queued", "running", "downloading"] as const)(
    "prevents model deletion during %s",
    (state) => {
      render(
        <Setup
          snapshot={{
            ...snapshot,
            recording: {
              ...snapshot.recording,
              session_id: state === "recording" ? "one" : null,
            },
            model: { ...snapshot.model, downloading: state === "downloading" },
            jobs:
              state === "queued" || state === "running"
                ? [
                    {
                      session_id: "one",
                      kind: "transcribe",
                      state,
                      progress: 0,
                      error: null,
                    },
                  ]
                : [],
          }}
        />,
        { wrapper },
      );
      const button = screen.getByRole("button", {
        name: "Delete downloaded model",
      }) as HTMLButtonElement;
      expect(button.disabled).toBe(true);
      expect(
        (screen.getByLabelText("Transcription model") as HTMLSelectElement)
          .disabled,
      ).toBe(true);
      fireEvent.click(button);
      expect(invoke).not.toHaveBeenCalled();
    },
  );
});

describe("vault location", () => {
  it("shows the active local folder before iCloud is configured", () => {
    render(<Setup snapshot={snapshot} />, { wrapper });
    expect(
      screen.getByRole("heading", { name: "Notes & recordings folder" }),
    ).toBeTruthy();
    expect(screen.getByText("On this iPhone")).toBeTruthy();
    expect(screen.getByText(snapshot.vault.local_path)).toBeTruthy();
    expect(screen.getByText("Off · saved only on this iPhone")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Connect iCloud folder" }),
    ).toBeTruthy();
    expect(screen.queryByText("Not connected")).toBeNull();
  });

  it("keeps both folder locations visible when a linked iCloud folder is unavailable", () => {
    render(
      <Setup
        snapshot={{
          ...snapshot,
          vault: {
            ...snapshot.vault,
            icloud_path: "/iCloud Drive/Meeting notes",
          },
          sync: {
            ...snapshot.sync,
            state: "error",
            error: "Folder unavailable",
            pending: 2,
          },
        }}
      />,
      { wrapper },
    );
    expect(screen.getByText("On this iPhone")).toBeTruthy();
    expect(screen.getByText("Meeting notes")).toBeTruthy();
    expect(
      screen.getByText(/Sync unavailable · changes are saved on this iPhone/),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Reconnect iCloud folder" }),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Connect iCloud folder" }),
    ).toBeNull();
  });
});
