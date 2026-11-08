import { onlineManager } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode, type ReactNode, useRef } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { Attachments, formatFileSize, formatFileType } from "./attachments";

import { createSessionSourceHost } from "~/session/session-source.test-support";
import type { FileInfo } from "~/shared/application/protocol.gen";
import type { VaultClient } from "~/shared/application/vault-client";

const native = vi.hoisted(() => ({
  focus: vi.fn(),
  picker: vi.fn(),
  list: vi.fn(),
  path: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
  callbacks: null as any,
  target: null as any,
  platform: "macos",
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onFocusChanged: native.focus }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: native.picker }));
vi.mock("@tauri-apps/plugin-os", () => ({ platform: () => native.platform }));
vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: { attachmentList: native.list, attachmentDir: native.path },
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { success: native.toastSuccess, error: native.toastError },
}));
vi.mock("~/shared/hooks/useNativeFileDrop", () => ({
  useNativeFileDrop: (target: unknown, callbacks: unknown) => {
    native.target = target;
    native.callbacks = callbacks;
    return { isHovering: false };
  },
}));

const hosts: ReturnType<typeof createSessionSourceHost>[] = [];
const info = (filename: string, sizeBytes = 1536): FileInfo => ({
  filename,
  sizeBytes,
  contentType: filename.endsWith(".pdf")
    ? "application/pdf"
    : "application/octet-stream",
});
function fixture(
  mode: "always" | "online" = "always",
  overrides: Partial<VaultClient> = {},
) {
  let files = [info("zeta", 5), info("Alpha.pdf")];
  const list = vi.fn<VaultClient["attachments"]>(async () => ({
    files: structuredClone(files),
    next_cursor: null,
  }));
  const saved = vi.fn().mockResolvedValue(undefined);
  const opened = vi.fn().mockResolvedValue(undefined),
    closed = vi.fn().mockResolvedValue(undefined);
  const open = vi.fn<VaultClient["openAttachment"]>(
    async (_session, filename) => ({
      id: crypto.randomUUID(),
      info: info(filename),
      labels: {
        open: { id: "Open file", message: "Open file" },
        reveal: { id: "Show file", message: "Show file" },
        ...(mode === "online"
          ? {
              save: { id: "Save original copy", message: "Save original copy" },
            }
          : {}),
      },
      open: opened,
      ...(mode === "online" ? { save: saved } : {}),
      reveal: vi.fn(),
      close: closed,
    }),
  );
  const directory = vi
    .fn<NonNullable<VaultClient["openAttachmentDirectory"]>>()
    .mockResolvedValue(undefined);
  const prepare = vi.fn<VaultClient["prepareAttachmentSource"]>(
    async (source) => ({
      input: {
        filename: source.name,
        size_bytes: source.kind === "file" ? source.file.size : 42,
        sha256: "0".repeat(64),
      },
      source: { kind: "path", path: "/device/source" },
      close: vi.fn().mockResolvedValue(undefined),
    }),
  );
  const upload = vi.fn<VaultClient["uploadAttachmentSource"]>(
    async (_session, source) => {
      const metadata = {
        filename: source.input.filename,
        contentType: "application/pdf",
        sizeBytes: source.input.size_bytes,
        sha256: source.input.sha256,
      };
      files.push(metadata);
      return metadata;
    },
  );
  const remove = vi.fn<VaultClient["removeAttachment"]>(
    async (_session, filename) => {
      files = files.filter((file) => file.filename !== filename);
      return true;
    },
  );
  const value = createSessionSourceHost(mode, {
    features: [
      "attachment_list",
      "attachment_upload",
      "attachment_metadata",
      "attachment_remove",
    ],
    attachments: list,
    openAttachment: open,
    openAttachmentDirectory: directory,
    prepareAttachmentSource: prepare,
    uploadAttachmentSource: upload,
    attachmentMetadata: async (_session, filename) => ({
      ...info(filename),
      sha256: "0".repeat(64),
    }),
    removeAttachment: remove,
    ...overrides,
  });
  hosts.push(value);
  return {
    ...value,
    list,
    open,
    opened,
    saved,
    closed,
    directory,
    prepare,
    upload,
    remove,
    setFiles: (next: FileInfo[]) => {
      files = next;
    },
  };
}
beforeEach(() => {
  vi.clearAllMocks();
  onlineManager.setOnline(true);
  native.focus.mockResolvedValue(() => {});
  native.picker.mockResolvedValue(["/device/new.pdf"]);
  native.platform = "macos";
});
afterEach(() => {
  cleanup();
  hosts.splice(0).forEach((host) => host.dispose());
  onlineManager.setOnline(true);
});
async function setup(value: ReturnType<typeof fixture>, children?: ReactNode) {
  const view = render(
    <value.Host>
      <Attachments sessionId="session-1">{children}</Attachments>
    </value.Host>,
  );
  await waitFor(() => expect(screen.getByText("Alpha.pdf")).toBeDefined());
  return view;
}
it.each(["always", "online"] as const)(
  "lists scoped metadata in %s mode without loading resources, paths or credentials from local commands",
  async (mode) => {
    const value = fixture(
      mode,
      mode === "online" ? { openAttachmentDirectory: undefined } : {},
    );
    await setup(value);
    expect(
      screen
        .getAllByRole("button")
        .findIndex((button) => button.textContent?.includes("Alpha.pdf")),
    ).toBeLessThan(
      screen
        .getAllByRole("button")
        .findIndex((button) => button.textContent?.includes("zeta")),
    );
    expect(screen.getByText("1.5 KB")).toBeDefined();
    expect(value.list).toHaveBeenCalledWith(
      "session-1",
      100,
      undefined,
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(value.open).not.toHaveBeenCalled();
    expect(native.list).not.toHaveBeenCalled();
    expect(native.path).not.toHaveBeenCalled();
    expect(value.rootClient.getQueryCache().getAll()).toEqual([]);
    if (mode === "online")
      expect(
        screen.queryByRole("button", { name: "Show in Finder" }),
      ).toBeNull();
  },
);
it.each(["macos", "windows"])(
  "preserves the local directory action on %s, including an empty directory",
  async (platform) => {
    native.platform = platform;
    const value = fixture();
    value.setFiles([]);
    render(
      <value.Host>
        <Attachments sessionId="session-1" />
      </value.Host>,
    );
    await screen.findByText("No attachments yet");
    fireEvent.click(
      screen.getByRole("button", {
        name:
          platform === "windows" ? "Show in File Explorer" : "Show in Finder",
      }),
    );
    await waitFor(() =>
      expect(value.directory).toHaveBeenCalledWith(
        "session-1",
        expect.objectContaining({ signal: expect.any(AbortSignal) }),
      ),
    );
  },
);
it("opens a remote file lazily through its client resource and closes the lease after the action", async () => {
  const value = fixture("online", { openAttachmentDirectory: undefined });
  await setup(value);
  fireEvent.click(screen.getByTitle("Open Alpha.pdf"));
  await waitFor(() => expect(value.opened).toHaveBeenCalledOnce());
  expect(value.open).toHaveBeenCalledWith(
    "session-1",
    "Alpha.pdf",
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(value.closed).toHaveBeenCalledOnce();
  expect(native.path).not.toHaveBeenCalled();
});
it("loads bounded additional pages explicitly instead of fetching the vault before showing results", async () => {
  const value = fixture();
  value.list.mockImplementation(async (_session, limit, after) => {
    expect(limit).toBe(100);
    return after
      ? { files: [info("Next page.pdf")], next_cursor: null }
      : { files: [info("Alpha.pdf")], next_cursor: "page2" };
  });
  await setup(value);
  expect(value.list).toHaveBeenCalledOnce();
  expect(value.open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Load more" }));
  await screen.findByText("Next page.pdf");
  expect(value.list).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
});
it("accepts a committed upload without a resource path and refreshes only its scoped list", async () => {
  const value = fixture("online", { openAttachmentDirectory: undefined });
  await setup(value);
  fireEvent.click(screen.getByRole("button", { name: "Add attachments" }));
  await screen.findByText("new.pdf");
  expect(value.upload).toHaveBeenCalledOnce();
  expect(value.open).not.toHaveBeenCalled();
  expect(native.toastError).not.toHaveBeenCalled();
  expect(value.rootClient.getQueryCache().getAll()).toEqual([]);
});
it("keeps the outer panel drop target and Windows filenames in submission order", async () => {
  const value = fixture();
  function Panel() {
    const target = useRef<HTMLDivElement>(null);
    return (
      <div ref={target} data-testid="panel">
        <Attachments sessionId="session-1" dropTargetRef={target} />
      </div>
    );
  }
  render(
    <value.Host>
      <Panel />
    </value.Host>,
  );
  await screen.findByText("Alpha.pdf");
  expect(native.target.current).toBe(screen.getByTestId("panel"));
  act(() => {
    native.callbacks.onDrop([
      "C:\\Users\\me\\report.pdf",
      "/device/second.pdf",
    ]);
  });
  await waitFor(() => expect(value.prepare).toHaveBeenCalledTimes(2));
  expect(value.prepare.mock.calls.map(([source]) => source.name)).toEqual([
    "report.pdf",
    "second.pdf",
  ]);
});
it("retains UUID and original staged source after an unknown upload reply, with explicit retry only", async () => {
  const value = fixture();
  value.upload.mockRejectedValueOnce(new Error("Reply lost"));
  await setup(value);
  fireEvent.click(screen.getByRole("button", { name: "Add attachments" }));
  await screen.findByText("Upload could not be confirmed");
  expect(value.upload).toHaveBeenCalledOnce();
  const id = value.upload.mock.calls[0][2]?.requestId;
  fireEvent.click(screen.getByRole("button", { name: "Retry new.pdf" }));
  await screen.findByText("new.pdf");
  expect(value.upload).toHaveBeenCalledTimes(2);
  expect(value.prepare).toHaveBeenCalledOnce();
  expect(value.upload.mock.calls[1][2]?.requestId).toBe(id);
});
it("confirms typed removal against current metadata before scoped invalidation", async () => {
  const value = fixture();
  await setup(value);
  fireEvent.click(screen.getByRole("button", { name: "Remove Alpha.pdf" }));
  expect(value.remove).not.toHaveBeenCalled();
  fireEvent.click(await screen.findByRole("button", { name: "Move to Trash" }));
  await waitFor(() =>
    expect(value.remove).toHaveBeenCalledWith(
      "session-1",
      "Alpha.pdf",
      { expected_sha256: "0".repeat(64) },
      { requestId: expect.any(String) },
    ),
  );
  await waitFor(() => expect(screen.queryByText("Alpha.pdf")).toBeNull());
});
it("rejects a late picker after host retirement without preparing sources or toasting another host", async () => {
  const value = fixture();
  let finish!: () => void;
  native.picker.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = () => done(["/private/late.pdf"]);
      }),
  );
  const view = await setup(value);
  fireEvent.click(screen.getByRole("button", { name: "Add attachments" }));
  await waitFor(() => expect(native.picker).toHaveBeenCalledOnce());
  view.unmount();
  await act(async () => finish());
  expect(value.prepare).not.toHaveBeenCalled();
  expect(value.upload).not.toHaveBeenCalled();
  expect(native.toastError).not.toHaveBeenCalled();
});
it.each([50, 150, 500])(
  "does not open or leak a resource that returns %ims after same-ID/same-string Scope replacement",
  async (delay) => {
    const first = fixture("online", { scope: "same" }),
      next = fixture("online", { scope: "same" });
    const close = vi.fn().mockResolvedValue(undefined),
      open = vi.fn();
    first.open.mockImplementationOnce(
      () =>
        new Promise((done) =>
          setTimeout(
            () =>
              done({
                id: "old-resource",
                info: info("Alpha.pdf"),
                labels: { open: { id: "Open" }, reveal: { id: "Show" } },
                open,
                reveal: vi.fn(),
                close,
              }),
            delay,
          ),
        ),
    );
    const view = await setup(first);
    fireEvent.click(screen.getByTitle("Open Alpha.pdf"));
    await waitFor(() => expect(first.open).toHaveBeenCalledOnce());
    first.scope.dispose();
    view.rerender(
      <next.Host>
        <StrictMode>
          <Attachments sessionId="session-1" />
        </StrictMode>
      </next.Host>,
    );
    await act(async () => {
      await new Promise((done) => setTimeout(done, delay + 20));
    });
    expect(open).not.toHaveBeenCalled();
    expect(close).toHaveBeenCalledOnce();
    expect(next.open).not.toHaveBeenCalled();
    expect(native.toastError).not.toHaveBeenCalled();
  },
);
it("keeps local offline uploads and disables remote offline, reader and unsupported controls", async () => {
  const value = fixture();
  await setup(value);
  act(() => {
    onlineManager.setOnline(false);
  });
  expect(
    (
      screen.getByRole("button", {
        name: "Add attachments",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(false);
  cleanup();
  onlineManager.setOnline(true);
  const remote = fixture("online", { openAttachmentDirectory: undefined });
  await setup(remote);
  act(() => {
    onlineManager.setOnline(false);
  });
  expect(
    (
      screen.getByRole("button", {
        name: "Add attachments",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect(
    (
      screen.getByRole("button", {
        name: "Remove Alpha.pdf",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  act(() => {
    onlineManager.setOnline(true);
    Object.assign(remote.client, { canEdit: false });
    remote.emit({ kind: "connection", state: "connected", error: null });
  });
  expect(
    (
      screen.getByRole("button", {
        name: "Add attachments",
      }) as HTMLButtonElement
    ).disabled,
  ).toBe(true);
  expect(remote.prepare).not.toHaveBeenCalled();
});
it("formats binary file sizes and extensionless types", () => {
  expect(formatFileSize(1024 ** 2)).toBe("1.0 MB");
  expect(formatFileSize(1024 ** 3)).toBe("1.0 GB");
  expect(formatFileType("")).toBe("File");
  expect(formatFileType("docx")).toBe("DOCX");
});

it("provides on-demand downloaded-copy actions without eager row leases", async () => {
  const value = fixture("online", { openAttachmentDirectory: undefined });
  await setup(value);
  expect(value.open).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "Actions for Alpha.pdf" }),
  );
  await waitFor(() => expect(value.open).toHaveBeenCalledOnce());
  await waitFor(() =>
    expect(
      screen
        .getAllByRole("button")
        .map((button) => button.getAttribute("aria-label")),
    ).toContain("Save original copy"),
  );
  const save = screen.getByRole("button", { name: "Save original copy" });
  fireEvent.click(save);
  await waitFor(() => expect(value.saved).toHaveBeenCalledOnce());
  expect(value.open).toHaveBeenCalledOnce();
  fireEvent.click(
    screen.getByRole("button", { name: "Actions for Alpha.pdf" }),
  );
  await waitFor(() => expect(value.closed).toHaveBeenCalledOnce());
});
