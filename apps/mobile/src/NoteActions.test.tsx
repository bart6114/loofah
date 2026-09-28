import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Session } from "./api";
import { NoteActions } from "./NoteActions";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const session: Session = {
  id: "one",
  title: "Meeting",
  notes: "Saved notes",
  summary: "# Summary\n\n- [ ] Task",
  transcript: [{ text: "Words", start: 0, end: 1 }],
  tasks: [],
  audio_url: null,
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
function setup(props: Partial<Parameters<typeof NoteActions>[0]> = {}) {
  const onDeleted = vi.fn();
  render(
    <NoteActions
      session={session}
      notes="Unsaved draft notes"
      recording={false}
      processing={false}
      onDeleted={onDeleted}
      {...props}
    />,
    { wrapper },
  );
  fireEvent.click(screen.getByRole("button", { name: "Note actions" }));
  return onDeleted;
}
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
    configurable: true,
    value() {
      this.setAttribute("open", "");
    },
  });
});
afterEach(cleanup);

describe("note actions", () => {
  it("copies the current note draft without saving it", async () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Copy notes" }));
    await screen.findByText("Notes copied");
    expect(invoke).toHaveBeenCalledWith("mobile_copy_text", {
      text: "Unsaved draft notes",
    });
    expect(invoke).not.toHaveBeenCalledWith(
      "mobile_update_session",
      expect.anything(),
    );
  });
  it("copies summary markdown intact", async () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Copy summary" }));
    await screen.findByText("Summary copied");
    expect(invoke).toHaveBeenCalledWith("mobile_copy_text", {
      text: session.summary,
    });
  });
  it("briefly confirms copying inside the action and resets its label", async () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Copy summary" }));
    const button = await screen.findByRole("button", {
      name: "Summary copied",
    });
    expect(button.querySelector("svg.copy-confirmed")).toBeTruthy();
    expect(screen.queryByRole("status")).toBeNull();
    await waitFor(() => expect(button.textContent).toBe("Copy summary"), {
      timeout: 2200,
    });
    expect(button.querySelector("svg.copy-confirmed")).toBeNull();
  });
  it("keeps pending copy feedback inside the action", async () => {
    let finish!: () => void;
    vi.mocked(invoke).mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Copy summary" }));
    expect(
      await screen.findByRole("button", { name: "Copying…" }),
    ).toHaveProperty("disabled", true);
    expect(screen.queryByRole("status")).toBeNull();
    await act(async () => finish());
    expect(
      await screen.findByRole("button", { name: "Summary copied" }),
    ).toBeTruthy();
  });
  it("copies all rendered transcript paragraphs and speaker labels", async () => {
    const segments = Array.from({ length: 65 }, (_, i) => ({
      speaker: i % 2 ? "Bart" : "Sam",
      text: `Paragraph ${i}`,
    }));
    vi.mocked(invoke).mockImplementation(async (command) =>
      command === "mobile_transcript_export" ? segments : undefined,
    );
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Copy transcript" }));
    await screen.findByText("Transcript copied");
    expect(invoke).toHaveBeenCalledWith("mobile_transcript_export", {
      sessionId: "one",
    });
    expect(invoke).toHaveBeenCalledWith("mobile_copy_text", {
      text: segments.map((s) => `${s.speaker}: ${s.text}`).join("\n\n"),
    });
  });
  it("shows clipboard failures without a success message", async () => {
    vi.mocked(invoke).mockRejectedValue("Clipboard unavailable");
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Copy notes" }));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "Clipboard unavailable",
    );
    expect(screen.queryByText("Notes copied")).toBeNull();
  });
  it("requires confirmation and navigates only after successful deletion", async () => {
    const onDeleted = setup();
    fireEvent.click(screen.getByRole("button", { name: "Delete note" }));
    expect(invoke).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Keep note" }));
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Note actions" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete note" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete note" }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledOnce());
    expect(invoke).toHaveBeenCalledWith("mobile_delete_session", {
      sessionId: "one",
    });
  });
  it("explains that deleting discards unsaved edits", () => {
    setup({ dirty: true });
    fireEvent.click(screen.getByRole("button", { name: "Delete note" }));
    expect(screen.getByRole("dialog").textContent).toContain("Unsaved edits");
  });
  it.each([{ recording: true }, { processing: true }])(
    "prevents deleting an active note: %o",
    (props) => {
      setup(props);
      expect(
        screen.getByRole("button", { name: "Delete note" }),
      ).toHaveProperty("disabled", true);
    },
  );
});
