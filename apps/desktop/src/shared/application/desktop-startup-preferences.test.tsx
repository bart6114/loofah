import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  startup: vi.fn(),
  connections: vi.fn(),
  save: vi.fn(),
  create: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: {
    applicationDesktopStartup: mocks.startup,
    applicationDesktopConnections: mocks.connections,
    applicationSaveDesktopStartup: mocks.save,
    applicationCreateDesktopConnection: mocks.create,
  },
}));

import { DesktopStartupPreferences } from "./desktop-startup-preferences";

const profile = {
  id: "profile",
  name: "company",
  server: "https://company.example/",
  member_id: null,
  vault_id: null,
};
const local = { selection: { kind: "local" }, revision: "missing" };
beforeEach(() => {
  vi.resetAllMocks();
  mocks.startup.mockResolvedValue({ status: "ok", data: local });
  mocks.connections.mockResolvedValue({ status: "ok", data: [profile] });
});
afterEach(() => cleanup());
function mount() {
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <DesktopStartupPreferences />
    </QueryClientProvider>,
  );
}
async function open() {
  fireEvent.click(screen.getByRole("button", { name: "Startup options" }));
  return screen.findByRole("combobox", { name: "Open Loofah with" });
}

it("does no profile or authentication work until opened", () => {
  mount();
  expect(mocks.startup).not.toHaveBeenCalled();
  expect(mocks.connections).not.toHaveBeenCalled();
});

it("saves a profile by immutable ID and revision for the next app start", async () => {
  mocks.save.mockResolvedValue({
    status: "ok",
    data: {
      kind: "saved",
      snapshot: {
        selection: {
          kind: "remote",
          profile_name: "company",
          profile_id: "profile",
        },
        revision: "new",
      },
    },
  });
  mount();
  fireEvent.change(await open(), { target: { value: "profile" } });
  fireEvent.click(screen.getByRole("button", { name: "Save startup choice" }));
  await screen.findByText("Saved for the next app start.");
  expect(mocks.save).toHaveBeenCalledExactlyOnceWith({
    selection: {
      kind: "remote",
      profile_name: "company",
      profile_id: "profile",
    },
    expected_revision: "missing",
  });
});

it("shows a concurrent preference change without retrying the write", async () => {
  mocks.save.mockResolvedValue({
    status: "ok",
    data: { kind: "conflict", snapshot: local },
  });
  mount();
  fireEvent.change(await open(), { target: { value: "profile" } });
  fireEvent.click(screen.getByRole("button", { name: "Save startup choice" }));
  await screen.findByRole("alert");
  expect(mocks.save).toHaveBeenCalledOnce();
  expect(screen.queryByText("Saved for the next app start.")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Reload preferences" }));
  await waitFor(() => expect(mocks.startup).toHaveBeenCalledTimes(2));
  expect(mocks.save).toHaveBeenCalledOnce();
});

it("keeps an unavailable saved profile visible and allows explicit standalone selection", async () => {
  mocks.startup.mockResolvedValue({
    status: "ok",
    data: {
      selection: { kind: "remote", profile_name: "old", profile_id: "removed" },
      revision: "old",
    },
  });
  mocks.save.mockResolvedValue({
    status: "ok",
    data: { kind: "saved", snapshot: local },
  });
  mount();
  const select = await open();
  expect((select as HTMLSelectElement).value).toBe("removed");
  fireEvent.change(select, { target: { value: "local" } });
  fireEvent.click(screen.getByRole("button", { name: "Save startup choice" }));
  await waitFor(() =>
    expect(mocks.save).toHaveBeenCalledExactlyOnceWith({
      selection: { kind: "local" },
      expected_revision: "old",
    }),
  );
});
