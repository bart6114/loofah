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
  invoke: vi.fn(),
  flush: vi.fn(),
  flushEditors: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
  isTauri: () => false,
}));
vi.mock("~/shared/write-queue", () => ({ flushDatabaseWrites: mocks.flush }));
vi.mock("@hypr/editor/note", () => ({ flushNoteEditors: mocks.flushEditors }));

import { CompanyStorageSettings } from "./company";

beforeEach(() => {
  vi.clearAllMocks();
  mocks.flush.mockResolvedValue(undefined);
  mocks.invoke.mockImplementation(
    async (
      _command: string,
      { operation, args }: { operation: string; args: Record<string, unknown> },
    ) => {
      if (operation === "status")
        return {
          active: true,
          recording: false,
          profile: {
            base_url: "https://company.example",
            server_id: "server",
            user_id: "me",
            vault_id: "team",
          },
          pending: [],
          transfers: [],
          error: null,
        };
      if (operation === "request" && args.path === "/v1/vaults")
        return [
          {
            id: "team",
            name: "Team notes",
            personal: false,
            members: { me: "owner", other: "viewer" },
          },
        ];
      if (operation === "request" && args.path === "/v1/users")
        return [
          { id: "me", name: "Me", email: "me@example.com" },
          { id: "other", name: "Other person", email: "other@example.com" },
          { id: "new", name: "New person", email: "new@example.com" },
        ];
      return {};
    },
  );
});
afterEach(cleanup);

const clients: QueryClient[] = [];
afterEach(() => {
  for (const client of clients.splice(0)) client.clear();
});

function show() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  clients.push(client);
  return render(
    <QueryClientProvider client={client}>
      <CompanyStorageSettings />
    </QueryClientProvider>,
  );
}

it("switches destinations only after pending frontend writes are flushed", async () => {
  show();
  await screen.findByText("Manage shared vault");
  fireEvent.change(screen.getByLabelText("Vault location"), {
    target: { value: "" },
  });
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "select",
      args: { vault_id: null },
    }),
  );
  expect(mocks.flush.mock.invocationCallOrder[0]).toBeLessThan(
    mocks.invoke.mock.invocationCallOrder[
      mocks.invoke.mock.calls.findIndex(([, arg]) => arg.operation === "select")
    ],
  );
});

function overrideStatus(fields: Record<string, unknown>) {
  const fallback = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation(async (command, args) => {
    const value = await fallback(command, args);
    return args.operation === "status" ? { ...value, ...fields } : value;
  });
}

function draft(
  error: string,
  args: Record<string, unknown> = { session_id: "s1", markdown: "My draft" },
) {
  overrideStatus({
    pending: [
      {
        call: { mutation_id: "draft-id", operation: "session_save_note", args },
        error: { code: error, message: "Review unsaved work" },
      },
    ],
  });
}

it("signs in and signs out only after editor and native writes are flushed", async () => {
  show();
  await screen.findByText("Manage shared vault");
  fireEvent.change(screen.getByLabelText("Server address"), {
    target: { value: "https://company.example" },
  });
  fireEvent.submit(screen.getByLabelText("Server address").closest("form")!);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "login",
      args: { url: "https://company.example" },
    }),
  );
  fireEvent.click(screen.getByText("Sign out"));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "logout",
      args: {},
    }),
  );
  expect(mocks.flushEditors).toHaveBeenCalledTimes(2);
  expect(mocks.flush).toHaveBeenCalledTimes(2);
  for (const operation of ["login", "logout"]) {
    const index = mocks.invoke.mock.calls.findIndex(
      ([, arg]) => arg.operation === operation,
    );
    expect(
      mocks.flush.mock.invocationCallOrder[operation === "login" ? 0 : 1],
    ).toBeLessThan(mocks.invoke.mock.invocationCallOrder[index]);
  }
});

it("keeps the current destination when flushing writes fails", async () => {
  mocks.flush.mockRejectedValueOnce(new Error("Draft could not be saved"));
  show();
  await screen.findByText("Manage shared vault");
  fireEvent.change(screen.getByLabelText("Vault location"), {
    target: { value: "" },
  });
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Draft could not be saved",
  );
  expect(
    mocks.invoke.mock.calls.some(([, args]) => args.operation === "select"),
  ).toBe(false);
});

it.each(["recording", "processing"])(
  "prevents sign-in, sign-out and switching while %s",
  async (activity) => {
    overrideStatus({ [activity]: true });
    show();
    await screen.findByText("Manage shared vault");
    expect(
      (screen.getByLabelText("Vault location") as HTMLSelectElement).disabled,
    ).toBe(true);
    expect(
      (screen.getByText("Sign in again") as HTMLButtonElement).disabled,
    ).toBe(true);
    expect((screen.getByText("Sign out") as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(
      screen.getByText(
        "Finish recording, transcription, and summaries before switching vaults.",
      ),
    ).toBeTruthy();
  },
);

it("creates and renames a shared vault and adds and removes members", async () => {
  show();
  await screen.findByText("New person");
  fireEvent.change(screen.getByLabelText("New shared vault"), {
    target: { value: "New team" },
  });
  fireEvent.click(screen.getByText("Create vault"));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "request",
      args: { method: "POST", path: "/v1/vaults", body: { name: "New team" } },
    }),
  );
  fireEvent.change(screen.getByLabelText("Vault name"), {
    target: { value: "Renamed team" },
  });
  fireEvent.click(screen.getByText("Rename"));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "request",
      args: {
        method: "PUT",
        path: "/v1/vaults/team",
        body: { name: "Renamed team" },
      },
    }),
  );
  fireEvent.change(screen.getByLabelText("Company user"), {
    target: { value: "new" },
  });
  fireEvent.change(screen.getByLabelText("New member role"), {
    target: { value: "viewer" },
  });
  fireEvent.submit(screen.getByLabelText("Company user").closest("form")!);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "request",
      args: {
        method: "PUT",
        path: "/v1/vaults/team/members/new",
        body: { role: "viewer" },
      },
    }),
  );
  await waitFor(() =>
    expect(
      (screen.getAllByText("Remove")[1] as HTMLButtonElement).disabled,
    ).toBe(false),
  );
  fireEvent.click(screen.getAllByText("Remove")[1]);
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation: "request",
      args: { method: "DELETE", path: "/v1/vaults/team/members/other" },
    }),
  );
});

it.each(["viewer", "editor", "personal"])(
  "hides membership administration for %s vaults",
  async (kind) => {
    const fallback = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation(async (command, args) =>
      args.operation === "request" && args.args.path === "/v1/vaults"
        ? [
            {
              id: "team",
              name: "Team notes",
              personal: kind === "personal",
              members: { me: kind === "personal" ? "owner" : kind },
            },
          ]
        : fallback(command, args),
    );
    show();
    await screen.findByText(
      "Team notes" +
        (kind === "personal" ? " (personal)" : "") +
        " · " +
        (kind === "personal" ? "owner" : kind),
    );
    expect(screen.queryByText("Manage shared vault")).toBeNull();
  },
);

it.each([
  ["Save as a separate note", "save_separate", "save_separate"],
  ["Export draft", "export_draft", "export_draft"],
  ["Discard draft and reload", "resolve", "discard"],
  ["Replace the displayed server version", "resolve", "replace"],
])(
  "%s sends an explicit recovery action with the displayed revision",
  async (label, operation, action) => {
    draft("conflict");
    const fallback = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation(async (command, args) =>
      args.operation === "preview"
        ? {
            data: { note_markdown: "Server draft" },
            versions: { "sessions/s1/notes.md": "revision" },
          }
        : fallback(command, args),
    );
    show();
    await screen.findByDisplayValue("Server draft");
    fireEvent.click(screen.getByText(label));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
        operation,
        args: {
          id: "draft-id",
          action,
          expected: { "sessions/s1/notes.md": "revision" },
        },
      }),
    );
  },
);

it("shows summary conflict previews returned as plain Markdown", async () => {
  draft("conflict");
  const fallback = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation(async (command, args) =>
    args.operation === "preview"
      ? {
          data: "Server summary",
          versions: { "sessions/s1/summary.md": "revision" },
        }
      : fallback(command, args),
  );
  show();
  await screen.findByDisplayValue("Server summary");
});

it("keeps drafts visible and reports a failed explicit replacement", async () => {
  draft("conflict", { session_id: "s1", patch: { markdown: "Patched draft" } });
  const fallback = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation(async (command, args) => {
    if (args.operation === "preview")
      return { data: { markdown: "New server text" }, versions: {} };
    if (args.operation === "resolve") throw new Error("Server changed again");
    return fallback(command, args);
  });
  show();
  await screen.findByDisplayValue("New server text");
  fireEvent.click(screen.getByText("Replace the displayed server version"));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Server changed again",
  );
  expect(screen.getByDisplayValue("Patched draft")).toBeTruthy();
});

it.each([
  ["Export file", "export_transfer"],
  ["Discard upload", "discard_transfer"],
])("%s acts on the selected pending upload", async (label, operation) => {
  overrideStatus({
    transfers: [{ id: "transfer-id", filename: "audio.wav", error: null }],
  });
  show();
  fireEvent.click(await screen.findByText(label));
  await waitFor(() =>
    expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
      operation,
      args: { id: "transfer-id" },
    }),
  );
});

it("retries pending work and preserves it if retrying fails", async () => {
  draft("unavailable", {
    session_id: "s1",
    doc: { markdown: "Template draft" },
  });
  const fallback = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation(async (command, args) => {
    if (args.operation === "retry") throw new Error("Still offline");
    return fallback(command, args);
  });
  show();
  fireEvent.click(await screen.findByText("Retry saving"));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Still offline",
  );
  expect(screen.getByDisplayValue("Template draft")).toBeTruthy();
});

it("lets owners edit membership and displays server rejection", async () => {
  show();
  await screen.findByText("Other person");
  mocks.invoke.mockRejectedValueOnce(
    new Error("A shared vault must retain an owner"),
  );
  fireEvent.change(screen.getAllByLabelText("Member role")[0], {
    target: { value: "viewer" },
  });
  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain("retain an owner");
});

it("leaves unsaved work visible when saving is unavailable", async () => {
  const fallback = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation(async (command, args) =>
    args.operation === "status"
      ? {
          active: true,
          recording: false,
          profile: {
            base_url: "https://company.example",
            server_id: "server",
            user_id: "me",
            vault_id: "team",
          },
          pending: [
            {
              call: {
                mutation_id: "draft",
                operation: "session_save_note",
                args: { session_id: "s1", markdown: "Keep this draft" },
              },
              error: { code: "unavailable", message: "Server offline" },
            },
          ],
          transfers: [],
          error: { code: "unavailable", message: "Server offline" },
        }
      : fallback(command, args),
  );
  show();
  await screen.findByText("Work waiting to be saved");
  fireEvent.click(screen.getByText("Server offline", { selector: "summary" }));
  expect(screen.getByDisplayValue("Keep this draft")).toBeTruthy();
  expect(screen.queryByText("Replace the displayed server version")).toBeNull();
});
