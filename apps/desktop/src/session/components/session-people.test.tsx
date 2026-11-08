import { cleanup, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SessionPeople } from "./session-people";

const mocks = vi.hoisted(() => ({
  useTranscriptOverview: vi.fn(),
  usePeople: vi.fn(),
}));

vi.mock("~/stt/transcript-overview", () => ({
  useTranscriptOverview: () => ({
    ...mocks.useTranscriptOverview(),
    readable: true,
    scope: { peopleOptions: () => ({}), queryClient: {} },
  }),
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: mocks.usePeople() }),
}));

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mocks.usePeople.mockReturnValue([]);
  mocks.useTranscriptOverview.mockReturnValue({
    data: { speaker_labels: [] },
  });
});

describe("SessionPeople", () => {
  it("renders nothing when no speakers are named", () => {
    mocks.useTranscriptOverview.mockReturnValue({
      data: { speaker_labels: [] },
    });

    const { container } = render(<SessionPeople sessionId="session-1" />);

    expect(container.firstChild).toBeNull();
  });

  it("shows registry names for assigned person ids, deduped across transcripts", () => {
    mocks.usePeople.mockReturnValue([{ id: "bob_peters", name: "Bob Peters" }]);
    mocks.useTranscriptOverview.mockReturnValue({
      data: { speaker_labels: ["bob_peters", "bob_peters", "kim"] },
    });

    render(<SessionPeople sessionId="session-1" />);

    expect(screen.getByText("Bob Peters")).toBeTruthy();
    // Legacy raw-name hint with no registry entry renders as itself.
    expect(screen.getByText("kim")).toBeTruthy();
    expect(screen.getAllByText(/Bob Peters|kim/)).toHaveLength(2);
  });
});
