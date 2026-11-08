import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  bootstrap: vi.fn(),
  remote: vi.fn(),
  recordings: vi.fn(),
}));
vi.mock("~/types/tauri.gen", () => ({
  commands: { applicationDesktopBootstrap: mocks.bootstrap },
}));
vi.mock("./server-preview", () => ({
  ServerConnection: (props: unknown) => {
    mocks.remote(props);
    return <div>Remote workspace</div>;
  },
}));
vi.mock("./desktop-startup-preferences", () => ({
  DesktopStartupPreferences: () => <button>Startup options</button>,
}));
vi.mock("./device-recovery", () => ({
  DeviceRecovery: () => <div>Device recovery</div>,
}));

vi.mock("./device-recording-host", () => ({
  DeviceRecordingHost: ({
    children,
    ...props
  }: {
    children: import("react").ReactNode;
  }) => {
    mocks.recordings(props);
    return <>{children}</>;
  },
}));

import {
  DesktopBootstrapRoot,
  loadDesktopBootstrap,
} from "./desktop-bootstrap";

beforeEach(() => vi.resetAllMocks());
afterEach(() => cleanup());

function mount(state: Awaited<ReturnType<typeof loadDesktopBootstrap>>) {
  const local = vi.fn(() => <div>Existing local app</div>);
  function Local() {
    return local();
  }
  render(
    <QueryClientProvider client={new QueryClient()}>
      <DesktopBootstrapRoot state={state}>
        <Local />
      </DesktopBootstrapRoot>
    </QueryClientProvider>,
  );
  return local;
}

it("opens the existing standalone app without connecting or mounting sign-in", async () => {
  mocks.bootstrap.mockResolvedValue({
    status: "ok",
    data: {
      startup: { selection: { kind: "local" }, revision: "missing" },
      connection: null,
    },
  });
  const local = mount(await loadDesktopBootstrap());
  expect(local).toHaveBeenCalledOnce();
  expect(screen.getByText("Existing local app")).toBeTruthy();
  expect(mocks.remote).not.toHaveBeenCalled();
  expect(screen.queryByText("Startup options")).toBeNull();
});

it("opens only the saved remote workspace and pins its destination", async () => {
  mocks.bootstrap.mockResolvedValue({
    status: "ok",
    data: {
      startup: {
        selection: {
          kind: "remote",
          profile_name: "company",
          profile_id: "profile",
        },
        revision: "saved",
      },
      connection: {
        id: "profile",
        name: "company",
        server: "https://company.example/",
        member_id: "alice",
        vault_id: "private",
      },
    },
  });
  const local = mount(await loadDesktopBootstrap());
  expect(local).not.toHaveBeenCalled();
  expect(screen.getByText("Remote workspace")).toBeTruthy();
  expect(mocks.remote.mock.lastCall?.[0]).toMatchObject({
    initialServer: "https://company.example/",
    autoConnect: true,
    preferredMemberId: "alice",
    preferredVaultId: "private",
    connectToServer: expect.any(Function),
  });
  expect(mocks.recordings.mock.lastCall?.[0]).toEqual({
    profileId: "profile",
    destination: {
      scopeKey: expect.any(String),
      scope: {
        server: "https://company.example/",
        member_id: "alice",
        vault_id: "private",
      },
    },
  });
});

it.each([
  "A saved profile was removed",
  "Unsupported startup preference version",
])("keeps local data closed on startup failure: %s", async (message) => {
  mocks.bootstrap.mockRejectedValue(new Error(message));
  const local = mount(await loadDesktopBootstrap());
  expect(local).not.toHaveBeenCalled();
  expect(mocks.remote).not.toHaveBeenCalled();
  expect(screen.getByRole("alert").textContent).toBe(message);
  expect(screen.getByText("Device recovery")).toBeTruthy();
  expect(screen.getByText("Startup options")).toBeTruthy();
});
