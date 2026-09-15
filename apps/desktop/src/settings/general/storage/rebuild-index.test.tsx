import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  sessionRebuildIndex: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("~/types/tauri.gen", () => ({
  commands: { sessionRebuildIndex: mocks.sessionRebuildIndex },
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { success: mocks.toastSuccess, error: mocks.toastError },
}));

vi.mock("@lingui/react/macro", () => ({
  Trans: ({ children }: { children?: ReactNode }) => <>{children}</>,
  useLingui: () => ({
    t: (strings: TemplateStringsArray, ...values: unknown[]) =>
      strings.reduce(
        (message, part, index) =>
          `${message}${part}${index < values.length ? String(values[index]) : ""}`,
        "",
      ),
  }),
}));

import { RebuildIndexRow } from "./rebuild-index";

function renderRow() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });

  return render(
    <QueryClientProvider client={queryClient}>
      <RebuildIndexRow />
    </QueryClientProvider>,
  );
}

describe("RebuildIndexRow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
  });

  it("rebuilds the index from files and toasts success", async () => {
    mocks.sessionRebuildIndex.mockResolvedValue({
      status: "ok",
      data: {
        sessions: 1,
        notes: 1,
        transcripts: 0,
        errors: [],
        ghost_sessions: [],
      },
    });
    renderRow();

    fireEvent.click(
      screen.getByRole("button", { name: /^refresh notes from disk$/i }),
    );

    await waitFor(() =>
      expect(mocks.sessionRebuildIndex).toHaveBeenCalledTimes(1),
    );
    await waitFor(() => expect(mocks.toastSuccess).toHaveBeenCalledTimes(1));
    expect(mocks.toastError).not.toHaveBeenCalled();
  });

  it("surfaces a command failure as an error toast", async () => {
    mocks.sessionRebuildIndex.mockResolvedValue({
      status: "error",
      error: "vault base unavailable",
    });
    renderRow();

    fireEvent.click(
      screen.getByRole("button", { name: /^refresh notes from disk$/i }),
    );

    await waitFor(() =>
      expect(mocks.toastError).toHaveBeenCalledWith("vault base unavailable"),
    );
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
  });
});
