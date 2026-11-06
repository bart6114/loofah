import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const commands = vi.hoisted(() => ({
  openPath: vi.fn(),
  openUrl: vi.fn(),
  revealItemInDir: vi.fn(),
}));
vi.mock("@hypr/plugin-opener2", () => ({ commands }));
vi.mock("@handlewithcare/react-prosemirror", () => ({
  useEditorEventCallback: (callback: () => void) => callback,
}));

import { scopedAttachmentPresentation } from "./attachment-presentation";
import {
  AttachmentPresentationContext,
  AttachmentResolverContext,
  type AttachmentResolution,
} from "./attachment-resolver";
import { FileAttachmentView } from "./file-attachment-view";

beforeEach(() => vi.clearAllMocks());
afterEach(cleanup);

function file() {
  return (
    <FileAttachmentView
      nodeProps={
        {
          node: {
            attrs: {
              attachmentId: "paper.pdf",
              path: "C:\\private\\secret.pdf",
              src: "asset://localhost/private/secret.pdf",
              name: "paper.pdf",
              mimeType: "application/pdf",
              size: 42,
            },
            nodeSize: 1,
          },
          getPos: () => 1,
        } as any
      }
    />
  );
}

function scoped(resolved: AttachmentResolution | null = null) {
  render(
    <AttachmentPresentationContext.Provider
      value={scopedAttachmentPresentation}
    >
      <AttachmentResolverContext.Provider value={() => resolved}>
        {file()}
      </AttachmentResolverContext.Provider>
    </AttachmentPresentationContext.Provider>,
  );
}

it("keeps default local open and reveal behavior", () => {
  render(file());
  fireEvent.mouseDown(screen.getByTitle("Open file"));
  fireEvent.mouseDown(screen.getByTitle("Show in Finder"));
  expect(commands.openPath).toHaveBeenCalledWith(
    "C:\\private\\secret.pdf",
    null,
  );
  expect(commands.revealItemInDir).toHaveBeenCalledWith(
    "C:\\private\\secret.pdf",
  );
});

it("keeps unavailable files visible without granting device actions", () => {
  scoped();
  expect(screen.getByText("paper.pdf")).toBeTruthy();
  expect(screen.queryByTitle("Open file")).toBeNull();
  expect(screen.queryByTitle("Show in Finder")).toBeNull();
  expect(commands.openPath).not.toHaveBeenCalled();
});

it("uses only scoped client actions and never a resolved server path", () => {
  const open = vi.fn();
  const reveal = vi.fn();
  scoped({
    src: "loofah-media://localhost/authorized",
    path: "/server/vault/secret.pdf",
    open,
    reveal,
  });
  fireEvent.mouseDown(screen.getByTitle("Open file"));
  fireEvent.mouseDown(screen.getByTitle("Show in Finder"));
  expect(open).toHaveBeenCalledOnce();
  expect(reveal).toHaveBeenCalledOnce();
  expect(commands.openPath).not.toHaveBeenCalled();
  expect(commands.revealItemInDir).not.toHaveBeenCalled();
});

it("does not infer native actions from a resolved URL or server path", () => {
  scoped({
    src: "loofah-media://localhost/authorized",
    path: "/server/vault/secret.pdf",
  });
  expect(screen.queryByTitle("Open file")).toBeNull();
  expect(screen.queryByTitle("Show in Finder")).toBeNull();
});
