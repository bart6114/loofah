import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { type ReactNode, useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  attachmentDir: vi.fn(),
  attachmentRemove: vi.fn(),
  invalidateQueries: vi.fn(),
  isHovering: false,
  nativeCallbacks: null as any,
  nativeTargetRef: null as any,
  onFocusChanged: vi.fn(),
  openPath: vi.fn(),
  removeUpload: vi.fn(),
  refetch: vi.fn(),
  selectFile: vi.fn(),
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
  uploadFile: vi.fn(),
  uploadStates: [] as any[],
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onFocusChanged: mocks.onFocusChanged }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.selectFile }));

vi.mock("@hypr/plugin-fs-sync", () => ({
  commands: {
    attachmentDir: mocks.attachmentDir,
    attachmentRemove: mocks.attachmentRemove,
  },
}));

vi.mock("@hypr/plugin-opener2", () => ({
  commands: { openPath: mocks.openPath },
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: {
    error: mocks.toastError,
    success: mocks.toastSuccess,
  },
}));

vi.mock("~/session/hooks/useAttachmentResolver", () => ({
  sessionAttachmentPathsQueryKey: (sessionId: string) => [
    "session",
    sessionId,
    "attachment-paths",
  ],
  useSessionAttachments: () => ({
    data: [
      {
        attachmentId: "zeta",
        path: "/vault/sessions/s1/attachments/zeta",
        extension: "",
        size: 5,
        modifiedAt: "2026-08-31T00:00:00Z",
      },
      {
        attachmentId: "Alpha.pdf",
        path: "/vault/sessions/s1/attachments/Alpha.pdf",
        extension: "pdf",
        size: 1536,
        modifiedAt: "2026-08-31T00:00:00Z",
      },
    ],
    isError: false,
    isLoading: false,
    refetch: mocks.refetch,
  }),
}));

vi.mock("~/shared/hooks/useFileUpload", () => ({
  useFileUpload: () => mocks.uploadFile,
  useFileUploadStates: () => ({
    states: mocks.uploadStates,
    remove: mocks.removeUpload,
  }),
}));

vi.mock("~/shared/hooks/useNativeFileDrop", () => ({
  useNativeFileDrop: (ref: unknown, callbacks: unknown) => {
    mocks.nativeTargetRef = ref;
    mocks.nativeCallbacks = callbacks;
    return { isHovering: mocks.isHovering };
  },
}));

import { Attachments, formatFileSize, formatFileType } from "./attachments";

import type { VaultClient } from "~/shared/application/vault-client";
import { VaultHostContext } from "~/shared/application/vault-provider";
import { VaultScope } from "~/shared/application/vault-scope";

const scopes: VaultScope[] = [];

function renderAttachments(children?: ReactNode) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  queryClient.invalidateQueries = mocks.invalidateQueries;
  const scope = new VaultScope({
    scope: "local-test",
    networkMode: "always",
    canEdit: true,
    features: ["attachment_metadata", "attachment_remove"],
    subscribe: async (listener: Parameters<VaultClient["subscribe"]>[0]) => {
      listener({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
    attachmentMetadata: async () => ({
      filename: "Alpha.pdf",
      contentType: "application/pdf",
      sizeBytes: 1536,
      sha256: "0".repeat(64),
    }),
    removeAttachment: mocks.attachmentRemove,
  } as unknown as VaultClient);
  scopes.push(scope);
  void scope.start();
  return render(
    <QueryClientProvider client={queryClient}>
      <VaultHostContext.Provider
        value={{ scope, error: null, retry: () => {} }}
      >
        <Attachments sessionId="s1">{children}</Attachments>
      </VaultHostContext.Provider>
    </QueryClientProvider>,
  );
}

describe("Attachments", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.isHovering = false;
    mocks.uploadStates = [];
    mocks.attachmentDir.mockResolvedValue({
      status: "ok",
      data: "/vault/sessions/s1/attachments",
    });
    mocks.attachmentRemove.mockResolvedValue(true);
    mocks.onFocusChanged.mockResolvedValue(() => {});
    mocks.openPath.mockResolvedValue({ status: "ok", data: null });
    mocks.selectFile.mockResolvedValue([]);
    mocks.uploadFile.mockResolvedValue({
      attachmentId: "new.txt",
      path: "/vault/sessions/s1/attachments/new.txt",
      url: "asset:new.txt",
    });
  });

  afterEach(() => {
    cleanup();
    for (const scope of scopes.splice(0)) scope.dispose();
  });

  it("sorts files and shows their type and size", () => {
    renderAttachments();

    const openButtons = screen.getAllByTitle(/^Open /);
    expect(openButtons.map((button) => button.getAttribute("title"))).toEqual([
      "Open Alpha.pdf",
      "Open zeta",
    ]);
    expect(screen.getByText("PDF")).toBeTruthy();
    expect(screen.getByText("1.5 KB")).toBeTruthy();
    expect(screen.getByText("File")).toBeTruthy();
    expect(screen.getByText("5 B")).toBeTruthy();
  });

  it("opens a file and the resolved attachment directory", async () => {
    renderAttachments();

    fireEvent.click(screen.getByTitle("Open Alpha.pdf"));
    await waitFor(() =>
      expect(mocks.openPath).toHaveBeenCalledWith(
        "/vault/sessions/s1/attachments/Alpha.pdf",
        null,
      ),
    );

    fireEvent.click(screen.getByRole("button", { name: "Show in Finder" }));
    await waitFor(() =>
      expect(mocks.openPath).toHaveBeenCalledWith(
        "/vault/sessions/s1/attachments",
        null,
      ),
    );
  });

  it("adds picker selections and native dropped paths", async () => {
    const { container } = renderAttachments();
    const dropTarget = container.firstElementChild;

    expect(dropTarget?.getAttribute("data-allow-file-drop")).toBe("true");

    mocks.selectFile.mockResolvedValueOnce([
      "/Users/me/selected.txt",
      "/Users/me/second.csv",
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Add attachments" }));
    await waitFor(() =>
      expect(mocks.uploadFile).toHaveBeenCalledWith({
        kind: "path",
        path: "/Users/me/selected.txt",
        name: "selected.txt",
      }),
    );

    mocks.nativeCallbacks.onDrop(["/Users/me/dropped.csv"], {
      x: 10,
      y: 10,
    });
    await waitFor(() =>
      expect(mocks.uploadFile).toHaveBeenCalledWith({
        kind: "path",
        path: "/Users/me/dropped.csv",
        name: "dropped.csv",
      }),
    );
  });

  it("uses the full attachment view for the drop target and visual hint", () => {
    mocks.isHovering = true;
    const { container } = renderAttachments(
      <div data-testid="session-metadata">Session metadata</div>,
    );
    const dropTarget = container.firstElementChild!;
    const metadata = screen.getByTestId("session-metadata");

    expect(metadata.closest("[data-allow-file-drop='true']")).toBe(dropTarget);

    expect(screen.getByText("Drop files to attach them")).toBeTruthy();
  });

  it("uses the outer session panel as the drop target when provided", () => {
    function FullPanel() {
      const dropTargetRef = useRef<HTMLDivElement>(null);
      return (
        <div
          ref={dropTargetRef}
          data-testid="attachment-panel"
          data-allow-file-drop="true"
        >
          <div data-testid="attachment-tab-header">Tabs</div>
          <Attachments sessionId="s1" dropTargetRef={dropTargetRef} />
        </div>
      );
    }

    const queryClient = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
        mutations: { retry: false },
      },
    });
    render(
      <QueryClientProvider client={queryClient}>
        <VaultHostContext.Provider
          value={{ scope: null, error: null, retry: () => {} }}
        >
          <FullPanel />
        </VaultHostContext.Provider>
      </QueryClientProvider>,
    );

    const panel = screen.getByTestId("attachment-panel");
    expect(mocks.nativeTargetRef.current).toBe(panel);
    expect(
      screen
        .getByTestId("attachment-tab-header")
        .closest("[data-allow-file-drop='true']"),
    ).toBe(panel);
  });

  it("renders pending and failed rows with retry and remove actions", () => {
    const pending = {
      clientId: "pending-1",
      candidate: { kind: "path", path: "/tmp/large.zip", name: "large.zip" },
      status: "pending",
      error: null,
      submittedAt: 1,
    };
    const failed = {
      clientId: "failed-1",
      candidate: { kind: "path", path: "/tmp/bad.pdf", name: "bad.pdf" },
      status: "error",
      error: new Error("disk full"),
      submittedAt: 2,
    };
    mocks.uploadStates = [pending, failed];
    renderAttachments();

    expect(screen.getByText("Copying…")).toBeTruthy();
    expect(screen.getByText("Copy failed")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Retry bad.pdf" }));
    expect(mocks.uploadFile).toHaveBeenCalledWith(failed.candidate, "failed-1");
    fireEvent.click(screen.getByRole("button", { name: "Remove bad.pdf" }));
    expect(mocks.removeUpload).toHaveBeenCalledWith("failed-1");
  });

  it("confirms removal before moving an attachment to trash", async () => {
    renderAttachments();

    fireEvent.click(screen.getByRole("button", { name: "Remove Alpha.pdf" }));
    expect(screen.getByText(/Links to this file/)).toBeTruthy();
    expect(mocks.attachmentRemove).not.toHaveBeenCalled();

    fireEvent.click(
      await screen.findByRole("button", { name: "Move to Trash" }),
    );
    await waitFor(() =>
      expect(mocks.attachmentRemove).toHaveBeenCalledWith(
        "s1",
        "Alpha.pdf",
        { expected_sha256: "0".repeat(64) },
        { requestId: expect.any(String) },
      ),
    );
    expect(mocks.invalidateQueries).toHaveBeenCalledWith({
      queryKey: ["session", "s1", "attachment-paths"],
    });
  });
});

describe("attachment formatting", () => {
  it("formats binary file sizes and extensionless types", () => {
    expect(formatFileSize(1024 * 1024)).toBe("1.0 MB");
    expect(formatFileSize(1024 * 1024 * 1024)).toBe("1.0 GB");
    expect(formatFileType("")).toBe("File");
    expect(formatFileType("docx")).toBe("DOCX");
  });
});
