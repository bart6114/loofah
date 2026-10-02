import { act, cleanup, render, screen } from "@testing-library/react";
import type { EditorView } from "prosemirror-view";
import { useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { FileHandlerConfig, NoteEditorRef } from "@hypr/editor/note";

import { FileDropTarget } from "./file-drop-target";
import { useNoteFileHandlerConfig } from "./file-handler";

const mocks = vi.hoisted(() => ({
  config: null as FileHandlerConfig | null,
  fileUpload: vi.fn(),
  handleNativeFileDrop: vi.fn(),
  handleNativeEditorDrag: vi.fn(),
  nativeCallbacks: null as any,
  processAudioFile: vi.fn(),
  processAudioPath: vi.fn(),
  removeUpload: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset:${path}`,
}));

vi.mock("@hypr/editor/note", () => ({
  handleNativeFileDrop: mocks.handleNativeFileDrop,
  handleNativeEditorDrag: mocks.handleNativeEditorDrag,
}));

vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: mocks.toastError },
}));

vi.mock("~/shared/hooks/useFileUpload", () => ({
  useFileUpload: () => mocks.fileUpload,
  useFileUploadStates: () => ({ states: [], remove: mocks.removeUpload }),
}));

vi.mock("~/shared/hooks/useNativeFileDrop", () => ({
  useNativeFileDrop: (_ref: unknown, callbacks: unknown) => {
    mocks.nativeCallbacks = callbacks;
    return { isHovering: false };
  },
}));

vi.mock("~/stt/useUploadFile", () => ({
  AUDIO_EXTENSIONS: ["wav", "mp3", "m4a"],
  isAudioUploadFile: (file: Pick<File, "name" | "type">) =>
    file.type.startsWith("audio/") || /\.(wav|mp3|m4a)$/i.test(file.name),
  useUploadFile: () => ({
    processAudioFile: mocks.processAudioFile,
    processAudioPath: mocks.processAudioPath,
  }),
}));

beforeEach(() => {
  vi.clearAllMocks();
  mocks.config = null;
  mocks.nativeCallbacks = null;
});

afterEach(cleanup);

describe("useNoteFileHandlerConfig", () => {
  it("routes internal native drags to the editor without uploading files", () => {
    const view = createEditorView(7);
    render(<Harness view={view} />);
    const event = { type: "drop", point: { x: 12, y: 34 } };

    act(() => mocks.nativeCallbacks.onInternalDrag(event));

    expect(mocks.handleNativeEditorDrag).toHaveBeenCalledWith(view, event);
    expect(mocks.handleNativeFileDrop).not.toHaveBeenCalled();
    expect(screen.queryByRole("status")).toBeNull();
  });
  it("shows native attachment, audio, and mixed hover hints", () => {
    render(<Harness />);

    act(() => mocks.nativeCallbacks.onHoverPaths(["/tmp/script.py"]));
    expect(screen.getByText("Drop files here to attach to note")).toBeTruthy();

    act(() => mocks.nativeCallbacks.onHoverPaths(["/tmp/clip.mp3"]));
    expect(
      screen.getByText("Drop to upload and transcribe audio"),
    ).toBeTruthy();

    act(() =>
      mocks.nativeCallbacks.onHoverPaths(["/tmp/clip.mp3", "/tmp/script.py"]),
    );
    expect(
      screen.getByText(
        "Audio will be transcribed; other files will be attached.",
      ),
    ).toBeTruthy();

    act(() => mocks.nativeCallbacks.onHoverEnd());
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("uses logical pointer coordinates and preserves native path order", () => {
    const view = createEditorView(7);
    render(<Harness view={view} />);

    act(() =>
      mocks.nativeCallbacks.onDrop(["/tmp/two.pdf", "/tmp/one.png"], {
        x: 12,
        y: 34,
      }),
    );

    expect(view.posAtCoords).toHaveBeenCalledWith({ left: 12, top: 34 });
    expect(mocks.handleNativeFileDrop).toHaveBeenCalledWith(
      view,
      expect.any(Object),
      [
        {
          kind: "path",
          path: "/tmp/two.pdf",
          name: "two.pdf",
          previewUrl: undefined,
        },
        {
          kind: "path",
          path: "/tmp/one.png",
          name: "one.png",
          previewUrl: "asset:/tmp/one.png",
        },
      ],
      7,
    );
  });

  it("routes one native audio path and leaves the other files to attach", () => {
    mocks.handleNativeFileDrop.mockImplementation(
      (_view: EditorView, config: FileHandlerConfig, candidates: any[]) =>
        config.onNativeDrop?.(candidates),
    );
    render(<Harness />);

    act(() =>
      mocks.nativeCallbacks.onDrop(["/tmp/clip.mp3", "/tmp/script.py"], {
        x: 1,
        y: 1,
      }),
    );

    expect(mocks.processAudioPath).toHaveBeenCalledOnce();
    expect(mocks.processAudioPath).toHaveBeenCalledWith("/tmp/clip.mp3");
    expect(mocks.handleNativeFileDrop.mock.results[0]?.value).toEqual({
      remainingCandidates: [
        expect.objectContaining({ path: "/tmp/script.py" }),
      ],
    });
  });

  it("keeps MIME-based audio classification for clipboard files", () => {
    render(<Harness />);
    const audio = new File(["audio"], "clip");
    const attachment = new File(["code"], "script.py");
    const items = createItems(["audio/mpeg", "text/x-python"]);

    const result = mocks.config?.onPaste?.([audio, attachment], items);

    expect(mocks.processAudioFile).toHaveBeenCalledWith(audio, {
      allowUnknownAudio: true,
      contentType: "audio/mpeg",
    });
    expect(result).toEqual({ remainingFiles: [attachment] });
  });

  it("reports upload failures using the candidate name", () => {
    render(<Harness />);
    mocks.config?.onFileUploadError?.(
      { kind: "path", path: "/tmp/script.py", name: "script.py" },
      new Error("disk full"),
    );

    expect(mocks.toastError).toHaveBeenCalledWith(
      "Couldn’t attach “script.py”: disk full",
      expect.any(Object),
    );
  });
});

function Harness({ view = createEditorView(null) }: { view?: EditorView }) {
  const editorRef = useRef<NoteEditorRef>({
    commands: {} as NoteEditorRef["commands"],
    flushPendingChanges: vi.fn(),
    view,
  });
  const result = useNoteFileHandlerConfig("session-1", editorRef);
  mocks.config = result.fileHandlerConfig;

  return (
    <div ref={result.fileDropTargetRef} data-testid="drop-target">
      <FileDropTarget kind={result.fileDragKind} />
    </div>
  );
}

function createEditorView(pos: number | null) {
  return {
    posAtCoords: vi.fn(() => (pos === null ? null : { inside: 0, pos })),
    state: { doc: { content: { size: 42 } } },
  } as unknown as EditorView;
}

function createItems(types: string[]) {
  return types.map((type) => ({
    getAsFile: () => null,
    kind: "file",
    type,
  })) as unknown as DataTransferItemList;
}
