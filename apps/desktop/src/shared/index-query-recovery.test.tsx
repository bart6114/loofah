import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { expect, it, vi } from "vitest";

import { useIndexQuery } from "./index-query";

const mocks = vi.hoisted(() => ({
  listen: vi
    .fn()
    .mockRejectedValueOnce(new Error("temporary listener failure"))
    .mockResolvedValue(() => {}),
}));
vi.mock("~/types/tauri.gen", () => ({
  events: { indexChanged: { listen: mocks.listen } },
}));

it("retries a failed listener and refreshes cached data after recovery", async () => {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  client.setQueryData(["recovery"], 1);
  const read = vi.fn(async () => 2);
  const view = renderHook(
    () =>
      useIndexQuery({
        entity: "sessions",
        queryKey: ["recovery"],
        queryFn: read,
      }),
    {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      ),
    },
  );
  expect(view.result.current.data).toBe(1);
  await waitFor(() => expect(view.result.current.data).toBe(2), {
    timeout: 3000,
  });
  expect(mocks.listen).toHaveBeenCalledTimes(2);
  expect(read).toHaveBeenCalledOnce();
  view.unmount();
  client.clear();
});
