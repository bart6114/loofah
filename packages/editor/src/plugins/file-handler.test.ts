// @vitest-environment jsdom

import { waitFor } from "@testing-library/react";
import { EditorState, TextSelection } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { schema } from "../note/schema";
import {
  type FileHandlerConfig,
  fileHandlerPlugin,
  handleFileDrop,
  handleNativeFileDrop,
} from "./file-handler";
import { imageTrailingParagraphPlugin } from "./image-trailing-paragraph";

const views: EditorView[] = [];

afterEach(() => {
  for (const view of views.splice(0)) {
    view.destroy();
  }
});

describe("handleFileDrop", () => {
  it("inserts GIF files as images and moves the caret after the image", async () => {
    const config = { onFileUpload: createUploader() };
    const view = createView(config, { imageTrailingParagraphs: true });
    const file = new File(["gif"], "motion.gif", { type: "image/gif" });

    expect(handleFileDrop(view, config, [file])).toBe(true);

    await waitFor(() => expect(view.state.doc.childCount).toBe(3));
    expect(view.state.doc.child(1).type).toBe(schema.nodes.image);
    expect(view.state.doc.child(1).attrs).toMatchObject({
      attachmentId: "motion.gif",
      src: "asset:/motion.gif",
    });
    expect(view.state.doc.child(2).type).toBe(schema.nodes.paragraph);
    expect(view.state.selection).toBeInstanceOf(TextSelection);
    expect(view.state.selection.$from.parent).toBe(view.state.doc.child(2));
  });

  it("preserves a selection moved while an image upload is pending", async () => {
    let resolveUpload!: (value: {
      attachmentId: string;
      path: string;
      url: string;
    }) => void;
    const config = {
      onFileUpload: vi.fn(
        () =>
          new Promise<{
            attachmentId: string;
            path: string;
            url: string;
          }>((resolve) => {
            resolveUpload = resolve;
          }),
      ),
    };
    const later = schema.node("paragraph", null, [schema.text("later")]);
    const view = createView(config, {
      content: [schema.node("paragraph"), later],
      imageTrailingParagraphs: true,
    });
    const file = new File(["png"], "diagram.png", { type: "image/png" });

    handleFileDrop(view, config, [file]);
    view.dispatch(
      view.state.tr.setSelection(TextSelection.create(view.state.doc, 3)),
    );

    resolveUpload({
      attachmentId: "diagram.png",
      path: "/vault/attachments/diagram.png",
      url: "asset:/diagram.png",
    });

    await waitFor(() =>
      expect(view.state.doc.child(1).type).toBe(schema.nodes.image),
    );
    expect(view.state.selection.$from.parent.textContent).toBe("later");
  });

  it("moves the caret after an image inserted between paragraphs", async () => {
    const config = { onFileUpload: createUploader() };
    const before = schema.node("paragraph", null, [schema.text("before")]);
    const after = schema.node("paragraph", null, [schema.text("after")]);
    const view = createView(config, {
      content: [before, after],
      imageTrailingParagraphs: true,
    });
    view.dispatch(
      view.state.tr.setSelection(TextSelection.create(view.state.doc, 7)),
    );
    const file = new File(["png"], "diagram.png", { type: "image/png" });

    handleFileDrop(view, config, [file]);

    await waitFor(() => expect(view.state.doc.childCount).toBe(3));
    expect(view.state.doc.child(1).type).toBe(schema.nodes.image);
    expect(view.state.selection.$from.parent.textContent).toBe("after");
    expect(view.state.selection.$from.parentOffset).toBe(0);
  });

  it("inserts arbitrary files as attachment cards in drop order", async () => {
    const config = { onFileUpload: createUploader() };
    const view = createView(config);
    const files = [
      new File(["pdf"], "brief.pdf", { type: "application/pdf" }),
      new File(["print('hi')"], "script.py"),
    ];

    handleFileDrop(view, config, files, view.state.doc.content.size);

    await waitFor(() => expect(view.state.doc.childCount).toBe(3));
    expect(view.state.doc.child(1).type).toBe(schema.nodes.fileAttachment);
    expect(view.state.doc.child(1).attrs).toMatchObject({
      name: "brief.pdf",
      mimeType: "application/pdf",
    });
    expect(view.state.doc.child(2).attrs).toMatchObject({
      name: "script.py",
      mimeType: "",
    });
  });

  it("inserts only files remaining after host drop handling", async () => {
    const audio = new File(["audio"], "clip.mp3", { type: "audio/mpeg" });
    const attachment = new File(["code"], "script.py");
    const onFileUpload = createUploader();
    const config = {
      onDrop: () => ({ remainingFiles: [attachment] }),
      onFileUpload,
    };
    const view = createView(config);

    handleFileDrop(
      view,
      config,
      [audio, attachment],
      view.state.doc.content.size,
    );

    await waitFor(() => expect(view.state.doc.childCount).toBe(2));
    expect(onFileUpload).toHaveBeenCalledOnce();
    expect(onFileUpload).toHaveBeenCalledWith(
      { kind: "file", file: attachment, name: attachment.name },
      expect.any(String),
    );
  });

  it("reports upload failures without inserting a node", async () => {
    const error = new Error("disk full");
    const file = new File(["code"], "script.py");
    const onFileUploadError = vi.fn();
    const config = {
      onFileUpload: vi.fn().mockRejectedValue(error),
      onFileUploadError,
    };
    const view = createView(config);

    handleFileDrop(view, config, [file], view.state.doc.content.size);

    await waitFor(() =>
      expect(onFileUploadError).toHaveBeenCalledWith(
        { kind: "file", file, name: file.name },
        error,
      ),
    );
    expect(view.state.doc.childCount).toBe(1);
    expect(
      document.querySelector("[data-file-upload-placeholder]"),
    ).not.toBeNull();
  });

  it("renders a placeholder synchronously and maps it through edits", async () => {
    let resolveUpload!: (value: {
      attachmentId: string;
      path: string;
      url: string;
    }) => void;
    const config = {
      onFileUpload: vi.fn(
        () =>
          new Promise<{
            attachmentId: string;
            path: string;
            url: string;
          }>((resolve) => {
            resolveUpload = resolve;
          }),
      ),
    };
    const view = createView(config);
    const file = new File(["pdf"], "brief.pdf", { type: "application/pdf" });

    handleFileDrop(view, config, [file], view.state.doc.content.size);

    expect(
      document.querySelector("[data-file-upload-placeholder]")?.textContent,
    ).toContain("brief.pdf");
    view.dispatch(view.state.tr.insertText("before", 1));
    resolveUpload({
      attachmentId: "brief.pdf",
      path: "/vault/attachments/brief.pdf",
      url: "asset:/brief.pdf",
    });

    await waitFor(() => expect(view.state.doc.childCount).toBe(2));
    expect(view.state.doc.textContent).toContain("before");
    expect(view.state.doc.child(1).attrs.attachmentId).toBe("brief.pdf");
  });
});

function createView(
  config: FileHandlerConfig,
  options: {
    content?: Parameters<typeof schema.node>[2];
    imageTrailingParagraphs?: boolean;
  } = {},
) {
  const host = document.createElement("div");
  document.body.append(host);
  const state = EditorState.create({
    schema,
    doc: schema.node(
      "doc",
      null,
      options.content ?? [schema.node("paragraph")],
    ),
    plugins: [
      ...(options.imageTrailingParagraphs
        ? [imageTrailingParagraphPlugin()]
        : []),
      fileHandlerPlugin(config),
    ],
  });
  const view = new EditorView(host, {
    state,
    dispatchTransaction(transaction) {
      view.updateState(view.state.apply(transaction));
    },
  });
  views.push(view);
  return view;
}

function createUploader() {
  return vi.fn(async (candidate: { name: string }) => ({
    attachmentId: candidate.name,
    path: `/vault/attachments/${candidate.name}`,
    url: `asset:/${candidate.name}`,
  }));
}

describe("image preview ownership", () => {
  function setup() {
    const revoke = vi.fn();
    vi.stubGlobal(
      "URL",
      Object.assign(URL, {
        createObjectURL: vi.fn(() => "blob:owned"),
        revokeObjectURL: revoke,
      }),
    );
    let resolve!: (result: {
      url: string;
      path: string;
      attachmentId: string;
    }) => void;
    let reject!: (error: Error) => void;
    const upload = vi.fn(
      () =>
        new Promise<{ url: string; path: string; attachmentId: string }>(
          (yes, no) => {
            resolve = yes;
            reject = no;
          },
        ),
    );
    const config = { onFileUpload: upload };
    const view = createView(config, {
      content: [schema.node("paragraph", null, schema.text("hello"))],
    });
    const file = new File(["image"], "test.png", { type: "image/png" });
    return {
      view,
      config,
      file,
      revoke,
      resolve: () =>
        resolve({
          url: "asset:/test.png",
          path: "test.png",
          attachmentId: "test.png",
        }),
      reject: () => reject(new Error("upload failed")),
    };
  }
  const settle = async () => {
    await Promise.resolve();
    await Promise.resolve();
  };
  const removeDocument = (view: EditorView) =>
    view.dispatch(view.state.tr.delete(0, view.state.doc.content.size));

  it("revokes a failed preview on normal document deletion exactly once", async () => {
    const h = setup();
    handleFileDrop(h.view, h.config, [h.file], 3);
    h.reject();
    await settle();
    expect(h.revoke).not.toHaveBeenCalled();
    removeDocument(h.view);
    expect(h.revoke).toHaveBeenCalledOnce();
    h.view.destroy();
    expect(h.revoke).toHaveBeenCalledOnce();
  });

  it.each(["success", "failure"])(
    "deletion disposes a pending preview before late %s",
    async (outcome) => {
      const h = setup();
      handleFileDrop(h.view, h.config, [h.file], 3);
      removeDocument(h.view);
      expect(h.revoke).toHaveBeenCalledOnce();
      if (outcome === "success") h.resolve();
      else h.reject();
      await settle();
      expect(h.view.state.doc.childCount).toBe(1);
      expect(
        h.view.dom.querySelector("[data-file-upload-placeholder]"),
      ).toBeNull();
      h.view.destroy();
      expect(h.revoke).toHaveBeenCalledOnce();
    },
  );

  it("retains the preview across error/retry and releases on successful upload", async () => {
    const h = setup();
    handleFileDrop(h.view, h.config, [h.file], 3);
    h.reject();
    await settle();
    const retry = [...h.view.dom.querySelectorAll("button")].find(
      (b) => b.textContent === "Retry",
    )!;
    retry.click();
    expect(h.revoke).not.toHaveBeenCalled();
    h.resolve();
    await settle();
    h.view.destroy();
    expect(h.revoke).toHaveBeenCalledOnce();
  });

  it("does not dispose previews when a speculative state is never committed", async () => {
    const h = setup();
    handleFileDrop(h.view, h.config, [h.file], 3);
    h.view.state.apply(
      h.view.state.tr.delete(0, h.view.state.doc.content.size),
    );
    expect(h.revoke).not.toHaveBeenCalled();
    expect(h.view.dom.querySelector("img")?.src).toBe("blob:owned");
    h.view.destroy();
    h.resolve();
    await settle();
    expect(h.revoke).toHaveBeenCalledOnce();
  });

  it("handles explicit removal and never revokes an externally supplied preview", async () => {
    const h = setup();
    handleFileDrop(h.view, h.config, [h.file], 3);
    h.view.dom.querySelector("button")!.click();
    h.reject();
    await settle();
    expect(h.revoke).toHaveBeenCalledOnce();
    handleNativeFileDrop(
      h.view,
      h.config,
      [
        {
          kind: "path",
          path: "test.png",
          name: "test.png",
          previewUrl: "blob:external",
        },
      ],
      3,
    );
    h.view.destroy();
    h.reject();
    await settle();
    expect(h.revoke).toHaveBeenCalledOnce();
  });
});
