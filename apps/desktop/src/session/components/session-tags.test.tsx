import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { SessionTags } from "./session-tags";

const mocks = vi.hoisted(() => ({
  accept: vi.fn(),
  dismiss: vi.fn(),
  update: vi.fn(),
}));

vi.mock("~/session/queries", () => ({
  useSession: () => ({
    tags: ["existing"],
    tag_suggestions: { items: ["existing", "new/topic"], dismissed: [] },
  }),
  useUpdateSession: () => mocks.update,
}));
vi.mock("~/tags/queries", () => ({
  useTags: () => [],
  useInUseTags: () => [],
  ensureTag: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    sessionAcceptTagSuggestion: mocks.accept,
    sessionDismissTagSuggestion: mocks.dismiss,
  },
}));

beforeEach(() => {
  vi.clearAllMocks();
  mocks.accept.mockResolvedValue({ status: "ok", data: true });
  mocks.dismiss.mockResolvedValue({ status: "ok", data: true });
});
afterEach(cleanup);

function renderTags() {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <SessionTags sessionId="session-1" />
    </QueryClientProvider>,
  );
}

it("shows new suggestions without applying them and hides already attached names", async () => {
  renderTags();
  expect(
    screen.queryByRole("button", { name: "Accept suggested tag existing" }),
  ).toBeNull();
  expect(mocks.accept).not.toHaveBeenCalled();
  expect(mocks.update).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "Accept suggested tag new/topic" }),
  );
  await waitFor(() =>
    expect(mocks.accept).toHaveBeenCalledWith("session-1", "new/topic"),
  );
  expect(mocks.update).not.toHaveBeenCalled();
});

it("dismisses a suggestion through the persisted store operation", async () => {
  renderTags();
  fireEvent.click(
    screen.getByRole("button", { name: "Dismiss suggested tag new/topic" }),
  );
  await waitFor(() =>
    expect(mocks.dismiss).toHaveBeenCalledWith("session-1", "new/topic"),
  );
  expect(mocks.accept).not.toHaveBeenCalled();
  expect(mocks.update).not.toHaveBeenCalled();
});
