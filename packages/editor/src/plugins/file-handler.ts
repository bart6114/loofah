import { Plugin, PluginKey } from "prosemirror-state";
import { Decoration, DecorationSet, type EditorView } from "prosemirror-view";

export type FileUploadCandidate =
  | { kind: "path"; path: string; name: string; previewUrl?: string }
  | { kind: "file"; file: File; name: string };

export type FileUploadResult = {
  url: string;
  attachmentId: string;
  path: string;
};

export type FileDropResult = boolean | void | { remainingFiles: File[] };
export type NativeFileDropResult =
  | boolean
  | void
  | { remainingCandidates: FileUploadCandidate[] };

export type FileHandlerConfig = {
  onDrop?: (
    files: File[],
    pos?: number,
    items?: DataTransferItemList,
  ) => FileDropResult;
  onNativeDrop?: (
    candidates: FileUploadCandidate[],
    pos?: number,
  ) => NativeFileDropResult;
  onPaste?: (files: File[], items?: DataTransferItemList) => FileDropResult;
  onFileUpload?: (
    candidate: FileUploadCandidate,
    clientId: string,
  ) => Promise<FileUploadResult>;
  onFileUploadError?: (candidate: FileUploadCandidate, error: unknown) => void;
  onFileUploadRemove?: (clientId: string) => void;
};

type UploadDecorationMeta =
  | { type: "add"; decorations: Decoration[] }
  | { type: "replace"; clientId: string; decoration: Decoration | null };

const fileHandlerKey = new PluginKey<DecorationSet>("fileHandler");
const IMAGE_EXTENSIONS = new Set(["gif", "jpeg", "jpg", "png", "webp"]);

function isImageCandidate(candidate: FileUploadCandidate) {
  if (candidate.kind === "file")
    return candidate.file.type.startsWith("image/");
  return IMAGE_EXTENSIONS.has(extensionOf(candidate.name));
}

function isFileDropRemainder(
  result: FileDropResult,
): result is { remainingFiles: File[] } {
  return Boolean(
    result &&
    typeof result === "object" &&
    "remainingFiles" in result &&
    Array.isArray(result.remainingFiles),
  );
}

function isNativeDropRemainder(
  result: NativeFileDropResult,
): result is { remainingCandidates: FileUploadCandidate[] } {
  return Boolean(
    result &&
    typeof result === "object" &&
    "remainingCandidates" in result &&
    Array.isArray(result.remainingCandidates),
  );
}

function createAttachmentNode(
  view: EditorView,
  candidate: FileUploadCandidate,
  result: FileUploadResult,
) {
  if (isImageCandidate(candidate)) {
    return view.state.schema.nodes.image.create({
      src: result.url,
      attachmentId: result.attachmentId,
    });
  }

  const attachmentType = view.state.schema.nodes.fileAttachment;
  if (!attachmentType) return null;
  return attachmentType.create({
    attachmentId: result.attachmentId,
    name: candidate.name,
    mimeType: candidate.kind === "file" ? candidate.file.type : "",
    src: result.url,
    path: result.path,
    size: candidate.kind === "file" ? candidate.file.size : 0,
  });
}

function replaceDecoration(
  view: EditorView,
  clientId: string,
  decoration: Decoration | null,
) {
  view.dispatch(
    view.state.tr.setMeta(fileHandlerKey, {
      type: "replace",
      clientId,
      decoration,
    } satisfies UploadDecorationMeta),
  );
}

function findDecoration(view: EditorView, clientId: string) {
  return fileHandlerKey
    .getState(view.state)
    ?.find(undefined, undefined, (spec) => spec.clientId === clientId)[0];
}

function makePreview(candidate: FileUploadCandidate) {
  if (!isImageCandidate(candidate)) return null;
  if (candidate.kind === "path") {
    return candidate.previewUrl
      ? { url: candidate.previewUrl, dispose: () => {} }
      : null;
  }

  const url = URL.createObjectURL(candidate.file);
  let disposed = false;
  return {
    url,
    dispose: () => {
      if (disposed) return;
      disposed = true;
      URL.revokeObjectURL(url);
    },
  };
}

function createPlaceholder(
  view: EditorView,
  config: FileHandlerConfig,
  candidate: FileUploadCandidate,
  clientId: string,
  state: "pending" | "error",
  preview: ReturnType<typeof makePreview>,
) {
  const element = document.createElement("div");
  element.className =
    "my-2 flex items-center gap-2 rounded-md border px-3 py-2 text-sm";
  element.dataset.fileUploadPlaceholder = clientId;
  element.contentEditable = "false";

  if (preview) {
    const image = document.createElement("img");
    image.src = preview.url;
    image.alt = "";
    image.className = "size-9 shrink-0 rounded object-cover";
    element.append(image);
  } else {
    const icon = document.createElement("span");
    icon.className = "text-muted-foreground shrink-0";
    icon.textContent = "📎";
    element.append(icon);
  }

  const label = document.createElement("span");
  label.className = "min-w-0 flex-1 truncate";
  const name = document.createElement("span");
  name.className = "block truncate font-medium";
  name.textContent = candidate.name;
  const status = document.createElement("span");
  status.className =
    state === "error"
      ? "text-destructive block text-xs"
      : "text-muted-foreground block text-xs";
  status.textContent = state === "error" ? "Copy failed" : "Copying…";
  label.append(name, status);
  element.append(label);

  if (candidate.kind === "file") {
    const size = document.createElement("span");
    size.className = "text-muted-foreground shrink-0 text-xs";
    size.textContent = formatFileSize(candidate.file.size);
    element.append(size);
  }

  if (state === "error") {
    const retry = document.createElement("button");
    retry.type = "button";
    retry.textContent = "Retry";
    retry.className = "shrink-0 text-xs font-medium";
    retry.addEventListener("click", () => {
      const found = findDecoration(view, clientId);
      if (!found) return;
      replaceDecoration(
        view,
        clientId,
        Decoration.widget(
          found.from,
          createPlaceholder(
            view,
            config,
            candidate,
            clientId,
            "pending",
            preview,
          ),
          { clientId, side: 1, dispose: preview?.dispose },
        ),
      );
      void uploadCandidate(view, config, candidate, clientId, preview);
    });
    element.append(retry);
  }

  const remove = document.createElement("button");
  remove.type = "button";
  remove.textContent = "Remove";
  remove.className = "text-muted-foreground shrink-0 text-xs";
  remove.addEventListener("click", () => {
    replaceDecoration(view, clientId, null);
    preview?.dispose();
    config.onFileUploadRemove?.(clientId);
  });
  element.append(remove);

  return element;
}

async function uploadCandidate(
  view: EditorView,
  config: FileHandlerConfig,
  candidate: FileUploadCandidate,
  clientId: string,
  preview: ReturnType<typeof makePreview>,
) {
  if (!config.onFileUpload) return;

  try {
    const result = await config.onFileUpload(candidate, clientId);
    if ((view as EditorView & { isDestroyed?: boolean }).isDestroyed) {
      preview?.dispose();
      return;
    }
    const decoration = findDecoration(view, clientId);
    if (!decoration) {
      preview?.dispose();
      return;
    }
    const node = createAttachmentNode(view, candidate, result);
    const tr = node
      ? view.state.tr.insert(decoration.from, node)
      : view.state.tr;
    tr.setMeta(fileHandlerKey, {
      type: "replace",
      clientId,
      decoration: null,
    } satisfies UploadDecorationMeta);
    view.dispatch(tr);
    preview?.dispose();
  } catch (error) {
    if ((view as EditorView & { isDestroyed?: boolean }).isDestroyed) {
      preview?.dispose();
      return;
    }
    const decoration = findDecoration(view, clientId);
    if (!decoration) {
      preview?.dispose();
      return;
    }
    replaceDecoration(
      view,
      clientId,
      Decoration.widget(
        decoration.from,
        createPlaceholder(view, config, candidate, clientId, "error", preview),
        { clientId, side: 1, dispose: preview?.dispose },
      ),
    );
    config.onFileUploadError?.(candidate, error);
  }
}

function handleCandidates(
  view: EditorView,
  config: FileHandlerConfig,
  candidates: FileUploadCandidate[],
  pos?: number,
) {
  if (candidates.length === 0) return false;
  if (!config.onFileUpload) return false;

  const insertPos = pos ?? view.state.selection.from;
  const uploads = candidates.map((candidate, index) => {
    const clientId = crypto.randomUUID();
    const preview = makePreview(candidate);
    return {
      candidate,
      clientId,
      preview,
      decoration: Decoration.widget(
        insertPos,
        createPlaceholder(
          view,
          config,
          candidate,
          clientId,
          "pending",
          preview,
        ),
        { clientId, side: index + 1, dispose: preview?.dispose },
      ),
    };
  });

  view.dispatch(
    view.state.tr.setMeta(fileHandlerKey, {
      type: "add",
      decorations: uploads.map((upload) => upload.decoration),
    } satisfies UploadDecorationMeta),
  );
  for (const upload of uploads) {
    void uploadCandidate(
      view,
      config,
      upload.candidate,
      upload.clientId,
      upload.preview,
    );
  }
  return true;
}

export function handleFileDrop(
  view: EditorView,
  config: FileHandlerConfig,
  files: File[],
  pos?: number,
  items?: DataTransferItemList,
) {
  if (files.length === 0) return false;
  let remaining = files;
  if (config.onDrop) {
    const result = config.onDrop(files, pos, items);
    if (result === true) return true;
    if (result === false) return false;
    if (isFileDropRemainder(result)) remaining = result.remainingFiles;
  }
  return handleCandidates(
    view,
    config,
    remaining.map((file) => ({ kind: "file", file, name: file.name })),
    pos,
  );
}

export function handleNativeFileDrop(
  view: EditorView,
  config: FileHandlerConfig,
  candidates: FileUploadCandidate[],
  pos?: number,
) {
  let remaining = candidates;
  if (config.onNativeDrop) {
    const result = config.onNativeDrop(candidates, pos);
    if (result === true) return true;
    if (result === false) return false;
    if (isNativeDropRemainder(result)) remaining = result.remainingCandidates;
  }
  return handleCandidates(view, config, remaining, pos);
}

export function fileHandlerPlugin(config: FileHandlerConfig) {
  return new Plugin<DecorationSet>({
    key: fileHandlerKey,
    state: {
      init: () => DecorationSet.empty,
      apply(transaction, decorations) {
        let next = decorations.map(transaction.mapping, transaction.doc);
        const meta = transaction.getMeta(fileHandlerKey) as
          | UploadDecorationMeta
          | undefined;
        if (!meta) return next;
        if (meta.type === "add") {
          // DecorationSet building mutates its input, while React may replay this reducer.
          return next.add(transaction.doc, meta.decorations.slice());
        }

        const existing = next.find(
          undefined,
          undefined,
          (spec) => spec.clientId === meta.clientId,
        );
        next = next.remove(existing);
        return meta.decoration
          ? next.add(transaction.doc, [meta.decoration])
          : next;
      },
    },
    props: {
      decorations: (state) => fileHandlerKey.getState(state),
      handleDrop(view, event) {
        const files = Array.from(event.dataTransfer?.files ?? []);
        if (files.length === 0) return false;
        event.preventDefault();
        const pos = view.posAtCoords({
          left: event.clientX,
          top: event.clientY,
        })?.pos;
        return handleFileDrop(
          view,
          config,
          files,
          pos,
          event.dataTransfer?.items,
        );
      },
      handlePaste(view, event) {
        const files = Array.from(event.clipboardData?.files ?? []);
        if (files.length === 0) return false;
        let remaining = files;
        if (config.onPaste) {
          const result = config.onPaste(files, event.clipboardData?.items);
          if (result === true) return true;
          if (result === false) return false;
          if (isFileDropRemainder(result)) remaining = result.remainingFiles;
        }
        return handleCandidates(
          view,
          config,
          remaining.map((file) => ({ kind: "file", file, name: file.name })),
        );
      },
    },
    view: (view) => {
      const previews = () =>
        new Set<() => void>(
          (fileHandlerKey.getState(view.state)?.find() ?? [])
            .map(
              (decoration) =>
                decoration.spec.dispose as (() => void) | undefined,
            )
            .filter((dispose): dispose is () => void => !!dispose),
        );
      let owned = previews();
      return {
        update() {
          const next = previews();
          for (const dispose of owned) if (!next.has(dispose)) dispose();
          owned = next;
        },
        destroy() {
          for (const dispose of owned) dispose();
          owned.clear();
        },
      };
    },
  });
}

function extensionOf(name: string) {
  const index = name.lastIndexOf(".");
  return index > 0 ? name.slice(index + 1).toLowerCase() : "";
}

function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
