import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const hoisted = vi.hoisted(() => {
  const scrolledTransaction = {};
  const transaction = {
    scrollIntoView: vi.fn(() => scrolledTransaction),
  };
  const selection = {
    empty: true,
    $from: {
      depth: 1,
      parent: { isTextblock: true },
      before: vi.fn(() => 5),
    },
  };
  const view = {
    state: { selection, tr: transaction },
    dispatch: vi.fn(),
  };

  return { scrolledTransaction, selection, transaction, view };
});

vi.mock("@handlewithcare/react-prosemirror", () => ({
  useEditorEventCallback:
    (callback: (view: typeof hoisted.view, event?: Event) => void) =>
    (event?: Event) =>
      callback(hoisted.view, event),
  useIsNodeSelected: () => false,
}));

import { scopedAttachmentPresentation } from "./attachment-presentation";
import {
  AttachmentPresentationContext,
  AttachmentResolverContext,
} from "./attachment-resolver";
import { ResizableImageView } from "./image-view";

describe("ResizableImageView", () => {
  beforeEach(() => {
    hoisted.selection.empty = true;
    hoisted.selection.$from.parent.isTextblock = true;
    hoisted.selection.$from.before.mockReturnValue(5);
    hoisted.transaction.scrollIntoView.mockClear();
    hoisted.view.dispatch.mockClear();
  });

  afterEach(() => {
    cleanup();
  });

  it("does not render a raw native source under the scoped client policy", () => {
    render(
      <AttachmentPresentationContext.Provider
        value={scopedAttachmentPresentation}
      >
        {image()}
      </AttachmentPresentationContext.Provider>,
    );
    expect(screen.getByRole("presentation").getAttribute("src")).toBeNull();
  });

  it("renders a client-resolved resource instead of the raw native source", () => {
    const resolver = () => ({ src: "loofah-media://localhost/authorized" });
    render(
      <AttachmentPresentationContext.Provider
        value={scopedAttachmentPresentation}
      >
        <AttachmentResolverContext.Provider value={resolver}>
          {image("image.png")}
        </AttachmentResolverContext.Provider>
      </AttachmentPresentationContext.Provider>,
    );
    expect(screen.getByRole("presentation").getAttribute("src")).toBe(
      "loofah-media://localhost/authorized",
    );
  });

  it("scrolls the caret after the image into view once the image loads", () => {
    renderImage();

    fireEvent.load(screen.getByRole("presentation"));

    expect(hoisted.transaction.scrollIntoView).toHaveBeenCalledOnce();
    expect(hoisted.view.dispatch).toHaveBeenCalledWith(
      hoisted.scrolledTransaction,
    );
  });

  it("does not scroll when the caret has moved away from the image", () => {
    hoisted.selection.$from.before.mockReturnValue(9);
    renderImage();

    fireEvent.load(screen.getByRole("presentation"));

    expect(hoisted.transaction.scrollIntoView).not.toHaveBeenCalled();
    expect(hoisted.view.dispatch).not.toHaveBeenCalled();
  });
});

function renderImage() {
  return render(image());
}

function image(attachmentId: string | null = null) {
  return (
    <ResizableImageView
      nodeProps={
        {
          node: {
            attrs: {
              src: "asset:/image.png",
              alt: null,
              title: null,
              attachmentId,
              sharedAttachmentId: null,
              editorWidth: 80,
            },
            nodeSize: 1,
          },
          getPos: () => 4,
        } as any
      }
    />
  );
}
