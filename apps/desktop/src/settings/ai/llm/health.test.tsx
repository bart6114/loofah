import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useConnectionHealth } from "./health";

const mocks = vi.hoisted(() => ({
  generate: vi.fn(),
  model: { modelId: "test-model" },
}));
vi.mock("ai", () => ({ generateText: mocks.generate }));
vi.mock("~/ai/hooks", () => ({
  useLanguageModel: () => mocks.model,
  useLLMConnectionStatus: () => ({ status: "success", providerId: "openai" }),
}));
vi.mock("~/shared/config", () => ({
  useConfigValues: () => ({ current_llm_provider: "openai" }),
}));

function HealthProbe() {
  const health = useConnectionHealth();
  return <output>{health.status}</output>;
}
afterEach(cleanup);

describe("connection readiness", () => {
  it("returns to checking during a recheck instead of retaining stale success", async () => {
    mocks.generate.mockResolvedValue({ text: "hello" });
    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}>
        <HealthProbe />
      </QueryClientProvider>,
    );
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("success"),
    );
    let finish!: (value: { text: string }) => void;
    mocks.generate.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    act(() => {
      void client.invalidateQueries({ queryKey: ["llm-health-check"] });
    });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("pending"),
    );
    await act(async () => finish({ text: "hello" }));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("success"),
    );
    client.clear();
  });
});
