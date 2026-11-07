import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

vi.mock("@tanstack/react-router", () => ({
  Outlet: () => <output>device route</output>,
}));
vi.mock("@tauri-apps/api/core", async (original) => ({
  ...(await original<typeof import("@tauri-apps/api/core")>()),
  isTauri: () => false,
}));

import MainAppLayout from "./main-app-layout";

afterEach(cleanup);

it("mounts the device/router shell before any vault or tab provider exists", () => {
  render(<MainAppLayout />);
  expect(screen.getByRole("status").textContent).toBe("device route");
});
