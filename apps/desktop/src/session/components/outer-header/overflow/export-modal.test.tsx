import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { createRenderCacheHost } from "../../note-input/transcript/render-cache.test-support";
import { ExportModal } from "./export-modal";

const mocks = vi.hoisted(() => ({
  export: vi.fn(),
  reveal: vi.fn(),
  target: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({ commands: {}, events: {} }));
vi.mock("~/session/queries", () => ({
  useEnhancedDocumentTarget: mocks.target,
}));
vi.mock("@hypr/plugin-opener2", () => ({
  commands: { revealItemInDir: mocks.reveal },
}));

let host: ReturnType<typeof createRenderCacheHost>;
beforeEach(() => {
  vi.resetAllMocks();
  host = createRenderCacheHost("scope:export", "always", {
    features: ["agent_read"],
    exportSession: mocks.export,
  });
  mocks.target.mockReturnValue({ kind: "summary" });
  mocks.export.mockResolvedValue({ path: "/downloads/Loofah/owned.pdf" });
  mocks.reveal.mockResolvedValue({ status: "ok", data: null });
});
afterEach(() => {
  cleanup();
  host.dispose();
});
function setup(
  view: { type: "raw" } | { type: "enhanced"; id: string } = { type: "raw" },
) {
  const changed = vi.fn();
  const content = (open: boolean) => (
    <host.Host>
      <ExportModal
        sessionId="s1"
        currentView={view}
        open={open}
        onOpenChange={changed}
      />
    </host.Host>
  );
  const rendered = render(content(true));
  return {
    ...rendered,
    changed,
    setOpen: (open: boolean) => rendered.rerender(content(open)),
  };
}

it("delegates selected saved sections and localized labels to the original scoped exporter", async () => {
  const value = setup();
  fireEvent.click(screen.getByLabelText("MD"));
  fireEvent.click(screen.getByLabelText("Transcript"));
  fireEvent.click(screen.getByRole("button", { name: "Export" }));
  await waitFor(() => expect(mocks.export).toHaveBeenCalledTimes(1));
  expect(mocks.export).toHaveBeenCalledWith(
    {
      session_id: "s1",
      target: { kind: "summary" },
      format: "md",
      include_note: true,
      include_summary: true,
      include_transcript: true,
      labels: {
        untitled: "Untitled",
        created: "Created",
        participants: "Participants",
        duration: "Duration",
        metadata: "Metadata",
        note: "Note",
        summary: "Summary",
        transcript: "Transcript",
      },
    },
    expect.objectContaining({
      requestId: expect.any(String),
      signal: expect.any(AbortSignal),
    }),
  );
  await waitFor(() => expect(value.changed).toHaveBeenCalledWith(false));
  expect(mocks.reveal).toHaveBeenCalledWith("/downloads/Loofah/owned.pdf");
});

it("preserves an explicitly selected template output", async () => {
  mocks.target.mockReturnValue({
    kind: "template_output",
    document_id: "template-id",
  });
  setup({ type: "enhanced", id: "template-id" });
  fireEvent.click(screen.getByRole("button", { name: "Export" }));
  await waitFor(() => expect(mocks.export).toHaveBeenCalled());
  expect(mocks.export.mock.calls[0][0].target).toEqual({
    kind: "template_output",
    document_id: "template-id",
  });
});

it("refuses an unidentifiable selected output without choosing another document", () => {
  mocks.target.mockReturnValue(null);
  setup({ type: "enhanced", id: "missing" });
  expect(
    (screen.getByRole("button", { name: "Export" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(screen.getByRole("alert").textContent).toContain(
    "cannot be identified safely",
  );
  expect(mocks.export).not.toHaveBeenCalled();
});

it("does not repeat a failed export and retains the selected options for explicit retry", async () => {
  mocks.export.mockRejectedValueOnce(
    new Error("Export could not be confirmed"),
  );
  setup();
  fireEvent.click(screen.getByLabelText("ORG"));
  fireEvent.click(screen.getByRole("button", { name: "Export" }));
  await screen.findByText("Export could not be confirmed");
  expect(mocks.export).toHaveBeenCalledTimes(1);
  expect((screen.getByLabelText("ORG") as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Export" }));
  await waitFor(() => expect(mocks.export).toHaveBeenCalledTimes(2));
});

it("cancels the captured request on close and never reveals a late result", async () => {
  let release!: (value: { path: string }) => void;
  mocks.export.mockReturnValue(
    new Promise((resolve) => {
      release = resolve;
    }),
  );
  const value = setup();
  fireEvent.click(screen.getByRole("button", { name: "Export" }));
  await waitFor(() => expect(mocks.export).toHaveBeenCalled());
  const signal = mocks.export.mock.calls[0][1].signal as AbortSignal;
  value.setOpen(false);
  expect(signal.aborted).toBe(true);
  await act(async () => release({ path: "/downloads/old.pdf" }));
  expect(mocks.reveal).not.toHaveBeenCalled();
  expect(value.changed).not.toHaveBeenCalled();
});
