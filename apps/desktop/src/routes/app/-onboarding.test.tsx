import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  navigate: vi.fn(),
  rememberWelcome: vi.fn(),
}));
vi.mock("@tanstack/react-router", async (original) => ({
  ...(await original<typeof import("@tanstack/react-router")>()),
  useNavigate: () => mocks.navigate,
}));
vi.mock("~/onboarding/welcome-note", () => ({
  setPendingWelcomeSession: mocks.rememberWelcome,
}));
vi.mock("./-resolve-entry-path", () => ({
  resolveShellEntryPath: async () => "/app/main",
}));
vi.mock("~/shared/deferred-view", () => ({
  deferredView: () => ({
    View: ({ onFinish }: { onFinish: (id: string) => void }) => (
      <button onClick={() => onFinish("welcome-local")}>Finish</button>
    ),
  }),
  DeferredView: ({ children }: { children: React.ReactNode }) => children,
}));

import { Route } from "./onboarding";

afterEach(cleanup);

it("hands the welcome note to the resolved local main host without accessing an unscoped store", async () => {
  const Component = Route.options.component!;
  render(<Component />);
  fireEvent.click(screen.getByRole("button", { name: "Finish" }));
  expect(mocks.rememberWelcome).toHaveBeenCalledWith("welcome-local");
  await waitFor(() =>
    expect(mocks.navigate).toHaveBeenCalledWith({ to: "/app/main" }),
  );
});
