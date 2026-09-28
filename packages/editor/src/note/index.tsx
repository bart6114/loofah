import "prosemirror-view/style/prosemirror.css";
import "prosemirror-gapcursor/style/gapcursor.css";
import "../styles/prosemirror.css";

import {
  ProseMirror,
  ProseMirrorDoc,
  reactKeys,
  useEditorEffect,
} from "@handlewithcare/react-prosemirror";
import { dropCursor } from "prosemirror-dropcursor";
import { gapCursor } from "prosemirror-gapcursor";
import { history } from "prosemirror-history";
import { Node as PMNode } from "prosemirror-model";
import {
  EditorState,
  Plugin,
  PluginKey,
  Selection,
  TextSelection,
  type Transaction,
} from "prosemirror-state";
import type { EditorView } from "prosemirror-view";
import {
  type ComponentProps,
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import { useDebounceCallback } from "usehooks-ts";

import { cn } from "@hypr/utils";

import { EditorErrorBoundary } from "../editor-error-boundary";
import {
  type AttachmentResolver,
  AttachmentEditingContext,
  AttachmentResolverContext,
  FileAttachmentView,
  getNodeViewFallbackTag,
  MentionNodeView,
  ResizableImageView,
  TaskItemView,
  withNodeViewErrorBoundary,
} from "../node-views";
import {
  autolinkPlugin,
  type FileHandlerConfig,
  type FileUploadCandidate,
  type PlaceholderFunction,
  SearchQuery,
  clearMarksOnEnterPlugin,
  clipboardPlugin,
  clipPastePlugin,
  type CommentAnchorsEvent,
  commentAnchorsPlugin,
  docChangeListenerPlugin,
  ensureImageTrailingParagraphs,
  fileHandlerPlugin,
  handleFileDrop,
  handleNativeFileDrop,
  getSearchState,
  hashtagPlugin,
  imageTrailingParagraphPlugin,
  type LinkOpenHandler,
  linkBoundaryGuardPlugin,
  linkOpenPlugin,
  placeholderPlugin,
  searchPlugin,
  searchReplaceAll,
  searchReplaceCurrent,
  setSearchState,
  taskIdentityPlugin,
} from "../plugins";
import { TaskSourceProvider } from "../task-source";
import { useTaskStorageOptional } from "../task-storage";
import {
  extractTasksFromContent,
  hydrateTaskContent,
  normalizeTaskContent,
  type TaskSource,
} from "../tasks";
import {
  FormatToolbar,
  type MentionConfig,
  MentionSuggestion,
  SlashCommandMenu,
  mentionSkipPlugin,
} from "../widgets";
import { areEquivalentEditorContents } from "./content-equivalence";
import { buildInputRules, buildKeymap } from "./keymap";
import {
  LinkedItemOpenBehaviorContext,
  type LinkedItemOpenBehavior,
  useLinkedItemOpenBehavior,
} from "./linked-item-open-behavior";
import { schema } from "./schema";
import {
  normalizeTitleHeadingDoc,
  titleHeadingPlugin,
  titleTrailerPlugin,
} from "./title-layout";
import {
  focusTrailingEmptyLine,
  trailingEmptyLineClickPlugin,
} from "./trailing-empty-line-click";

export type {
  MentionConfig,
  FileHandlerConfig,
  FileUploadCandidate,
  PlaceholderFunction,
};
export { handleFileDrop, handleNativeFileDrop };
export {
  normalizePortableAttachmentUrls,
  parsePortableAttachmentSrc,
  toPortableAttachmentSrc,
} from "./portable-attachments";
export { schema };
export { areEquivalentEditorContents };
export {
  type CommentAnchorInput,
  type CommentAnchorsEvent,
  commentAnchorsPluginKey,
  getCommentAnchorRanges,
  getCommentAnchorScreenPositions,
  getSelectionScreenRect,
  setActiveCommentAnchor,
  setCommentAnchors,
} from "../plugins/comment-anchors";
export { useLinkedItemOpenBehavior };

export interface JSONContent {
  type?: string;
  attrs?: Record<string, any>;
  content?: JSONContent[];
  marks?: { type: string; attrs?: Record<string, any> }[];
  text?: string;
}

export interface SearchReplaceParams {
  query: string;
  replacement: string;
  caseSensitive: boolean;
  wholeWord: boolean;
  all: boolean;
  matchIndex: number;
}

export interface EditorCommands {
  focus: () => void;
  focusAtStart: () => void;
  focusAtTrailingEmptyLine: () => void;
  focusAtPixelWidth: (pixelWidth: number) => void;
  insertAtStartAndFocus: (content: string) => void;
  replaceContent: (content: JSONContent) => void;
  setSearch: (query: string, caseSensitive: boolean) => void;
  replace: (params: SearchReplaceParams) => void;
}

export interface NoteEditorRef {
  view: EditorView | null;
  commands: EditorCommands;
  flushPendingChanges: () => void;
}

export type SessionMentionDropData = {
  id: string;
  label: string;
};

export type SessionMentionDropConfig = {
  has?: (
    dataTransfer: Pick<DataTransfer, "types"> | null | undefined,
  ) => boolean;
  read: (
    dataTransfer: Pick<DataTransfer, "getData" | "types"> | null | undefined,
  ) => SessionMentionDropData | null;
};

type NodeViewComponents = NonNullable<
  ComponentProps<typeof ProseMirror>["nodeViewComponents"]
>;

export type EditorSaveResult = void | (() => JSONContent);

export interface NoteEditorProps {
  className?: string;
  handleChange?: (content: JSONContent) => void | Promise<EditorSaveResult>;
  onDraftChange?: (getContent: () => JSONContent) => void;
  initialContent?: JSONContent;
  initialDraft?: JSONContent | (() => JSONContent | undefined);
  confirmedDraft?: () => JSONContent;
  resolveAttachment?: AttachmentResolver;
  mentionConfig?: MentionConfig;
  placeholderComponent?: PlaceholderFunction;
  fileHandlerConfig?: FileHandlerConfig;
  onNavigateToTitle?: (pixelWidth?: number) => void;
  onLinkOpen?: LinkOpenHandler;
  linkedItemOpenBehavior?: LinkedItemOpenBehavior;
  taskSource?: TaskSource;
  extraNodeViews?: NodeViewComponents;
  sessionMentionDropConfig?: SessionMentionDropConfig;
  showFormatToolbar?: boolean;
  showSlashCommand?: boolean;
  readOnly?: boolean;
  onViewReady?: (view: EditorView) => void;
  onViewDisposed?: (view: EditorView) => void;
  syncContentWhenFocused?: boolean;
  enforceTitleHeading?: boolean;
  /** Host-owned element rendered as a widget between the title (first node)
   * and the body. Fixed at mount; update its children, not the element. */
  titleTrailerElement?: HTMLElement;
  /** Fixed at mount: plugins are not reconfigurable afterwards. */
  commentAnchorsEnabled?: boolean;
  onCommentAnchorsEvent?: (event: CommentAnchorsEvent) => void;
}

const baseNodeViews = {
  fileAttachment: withNodeViewErrorBoundary<HTMLDivElement>(
    FileAttachmentView,
    { name: "fileAttachment" },
  ),
  image: withNodeViewErrorBoundary<HTMLDivElement>(ResizableImageView, {
    name: "image",
  }),
  "mention-@": withNodeViewErrorBoundary<HTMLElement>(MentionNodeView, {
    name: "mention-@",
  }),
  taskItem: withNodeViewErrorBoundary<HTMLLIElement>(TaskItemView, {
    name: "taskItem",
  }),
};

const COMPOSITION_SYNC_GRACE_MS = 500;

export type CompositionState = {
  active: boolean;
  endedAt: number;
};

function isSameContent(
  left: JSONContent | undefined,
  right: JSONContent | undefined,
) {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function shouldReplaceEditorContent({
  currentContent,
  nextContent,
  hasFocus,
  isComposing,
  syncContentWhenFocused,
}: {
  currentContent: JSONContent;
  nextContent: JSONContent;
  hasFocus: boolean;
  isComposing: boolean;
  syncContentWhenFocused: boolean;
}) {
  if (isSameContent(currentContent, nextContent)) {
    return false;
  }

  if (isComposing) {
    return false;
  }

  if (hasFocus && !syncContentWhenFocused) {
    return false;
  }

  return true;
}

function wrapNodeViewComponents(nodeViews?: NodeViewComponents) {
  if (!nodeViews) {
    return {};
  }

  return Object.fromEntries(
    Object.entries(nodeViews).map(([name, Component]) => [
      name,
      withNodeViewErrorBoundary(Component, {
        fallbackTag: getNodeViewFallbackTag(name),
        name,
      }),
    ]),
  ) as NodeViewComponents;
}

export function getEditorCompositionWaitMs(
  view: Pick<EditorView, "composing">,
  compositionState: CompositionState,
) {
  if (view.composing || compositionState.active) {
    return COMPOSITION_SYNC_GRACE_MS;
  }

  return Math.max(
    COMPOSITION_SYNC_GRACE_MS - (Date.now() - compositionState.endedAt),
    0,
  );
}

function createCompositionStatePlugin(
  setCompositionActive: (active: boolean) => void,
) {
  return new Plugin({
    key: new PluginKey("imeCompositionState"),
    props: {
      handleDOMEvents: {
        compositionstart() {
          setCompositionActive(true);
          return false;
        },
        compositionupdate() {
          setCompositionActive(true);
          return false;
        },
        compositionend() {
          setCompositionActive(false);
          return false;
        },
      },
    },
  });
}

export function createReadOnlyPlugin() {
  return new Plugin({
    filterTransaction: (transaction) => !transaction.docChanged,
  });
}

function createSessionMentionDropPlugin(config: SessionMentionDropConfig) {
  const hasSession = (dataTransfer: DataTransfer | null) =>
    config.has ? config.has(dataTransfer) : Boolean(config.read(dataTransfer));
  const readSession = (dataTransfer: DataTransfer | null) =>
    config.read(dataTransfer);

  return new Plugin({
    key: new PluginKey("sessionMentionDrop"),
    props: {
      handleDOMEvents: {
        dragover(_view, event) {
          const dataTransfer = (event as DragEvent).dataTransfer;
          if (!hasSession(dataTransfer)) {
            return false;
          }

          event.preventDefault();
          if (dataTransfer) {
            dataTransfer.dropEffect = "copy";
          }
          return true;
        },
      },
      handleDrop(view, event) {
        const session = readSession(event.dataTransfer);
        const mentionType = view.state.schema.nodes["mention-@"];
        if (!session || !mentionType) {
          return false;
        }

        event.preventDefault();
        event.stopPropagation();

        const mentionNode = mentionType.create({
          id: session.id,
          type: "session",
          label: session.label,
        });
        const space = view.state.schema.text(" ");
        const pos =
          view.posAtCoords({
            left: event.clientX,
            top: event.clientY,
          })?.pos ?? view.state.selection.from;

        try {
          const tr = view.state.tr.insert(pos, [mentionNode, space]);
          tr.setSelection(
            TextSelection.create(
              tr.doc,
              pos + mentionNode.nodeSize + space.nodeSize,
            ),
          );
          view.dispatch(tr.scrollIntoView());
        } catch {
          const tr = view.state.tr
            .replaceSelectionWith(mentionNode)
            .insertText(" ");
          view.dispatch(tr.scrollIntoView());
        }

        view.focus();
        return true;
      },
    },
  });
}

function ViewCapture({
  viewRef,
  onViewReady,
  onViewDisposed,
}: {
  viewRef: React.RefObject<EditorView | null>;
  onViewReady: (view: EditorView) => void;
  onViewDisposed?: (view: EditorView) => void;
}) {
  const callbacksRef = useRef({ onViewReady, onViewDisposed });
  callbacksRef.current = { onViewReady, onViewDisposed };

  useEditorEffect(
    (view) => {
      if (view && viewRef.current !== view) {
        viewRef.current = view;
        callbacksRef.current.onViewReady(view);
      }

      return () => {
        if (viewRef.current === view) {
          viewRef.current = null;
          callbacksRef.current.onViewDisposed?.(view);
        }
      };
    },
    [viewRef],
  );
  return null;
}

const noopCommands: EditorCommands = {
  focus: () => {},
  focusAtStart: () => {},
  focusAtTrailingEmptyLine: () => {},
  focusAtPixelWidth: () => {},
  insertAtStartAndFocus: () => {},
  replaceContent: () => {},
  setSearch: () => {},
  replace: () => {},
};

function EditorCommandsBridge({
  commandsRef,
}: {
  commandsRef: React.RefObject<EditorCommands>;
}) {
  useEditorEffect(
    (view) => {
      if (!view) {
        commandsRef.current = noopCommands;
        return;
      }

      commandsRef.current = {
        focus: () => {
          view.focus();
        },
        focusAtStart: () => {
          view.dispatch(
            view.state.tr.setSelection(Selection.atStart(view.state.doc)),
          );
          view.focus();
        },
        focusAtTrailingEmptyLine: () => {
          focusTrailingEmptyLine(view);
        },
        focusAtPixelWidth: (pixelWidth: number) => {
          const blockStart = Selection.atStart(view.state.doc).from;
          const firstTextNode = view.dom.querySelector(".ProseMirror > *");
          if (firstTextNode) {
            const editorStyle = window.getComputedStyle(firstTextNode);
            const canvas = document.createElement("canvas");
            const ctx = canvas.getContext("2d");
            if (ctx) {
              ctx.font = `${editorStyle.fontWeight} ${editorStyle.fontSize} ${editorStyle.fontFamily}`;
              const firstBlock = view.state.doc.firstChild;
              if (firstBlock && firstBlock.textContent) {
                const text = firstBlock.textContent;
                let charPos = 0;
                for (let i = 0; i <= text.length; i++) {
                  const currentWidth = ctx.measureText(text.slice(0, i)).width;
                  if (currentWidth >= pixelWidth) {
                    charPos = i;
                    break;
                  }
                  charPos = i;
                }
                const targetPos = Math.min(
                  blockStart + charPos,
                  view.state.doc.content.size - 1,
                );
                view.dispatch(
                  view.state.tr.setSelection(
                    TextSelection.create(view.state.doc, targetPos),
                  ),
                );
                view.focus();
                return;
              }
            }
          }

          view.dispatch(
            view.state.tr.setSelection(Selection.atStart(view.state.doc)),
          );
          view.focus();
        },
        insertAtStartAndFocus: (content: string) => {
          if (!content) return;
          const pos = Selection.atStart(view.state.doc).from;
          const tr = view.state.tr.insertText(content, pos);
          tr.setSelection(TextSelection.create(tr.doc, pos));
          view.dispatch(tr);
          view.focus();
        },
        replaceContent: (content: JSONContent) => {
          if (content.type !== "doc") return;

          try {
            const nextDoc = PMNode.fromJSON(schema, content);
            if (nextDoc.eq(view.state.doc)) {
              return;
            }

            view.dispatch(
              view.state.tr.replaceWith(
                0,
                view.state.doc.content.size,
                nextDoc.content,
              ),
            );
          } catch {
            // invalid content
          }
        },
        setSearch: (query: string, caseSensitive: boolean) => {
          const q = new SearchQuery({ search: query, caseSensitive });
          const current = getSearchState(view.state);
          if (current && current.query.eq(q)) return;
          view.dispatch(setSearchState(view.state.tr, q));
        },
        replace: (params: SearchReplaceParams) => {
          const query = new SearchQuery({
            search: params.query,
            replace: params.replacement,
            caseSensitive: params.caseSensitive,
            wholeWord: params.wholeWord,
          });

          view.dispatch(setSearchState(view.state.tr, query));

          if (params.all) {
            searchReplaceAll(view.state, (tr) => view.dispatch(tr));
          } else {
            let result = query.findNext(view.state);
            let idx = 0;
            while (result && idx < params.matchIndex) {
              result = query.findNext(view.state, result.to);
              idx++;
            }
            if (!result) return;
            view.dispatch(
              view.state.tr.setSelection(
                TextSelection.create(view.state.doc, result.from, result.to),
              ),
            );
            searchReplaceCurrent(view.state, (tr) => view.dispatch(tr));
          }
        },
      };

      return () => {
        commandsRef.current = noopCommands;
      };
    },
    [commandsRef],
  );

  return null;
}

export const NoteEditor = forwardRef<NoteEditorRef, NoteEditorProps>(
  function NoteEditor(props, ref) {
    const {
      handleChange,
      onDraftChange,
      className,
      initialContent,
      initialDraft,
      confirmedDraft,
      resolveAttachment,
      mentionConfig,
      placeholderComponent,
      fileHandlerConfig,
      onNavigateToTitle,
      onLinkOpen,
      linkedItemOpenBehavior = "current",
      taskSource,
      extraNodeViews,
      sessionMentionDropConfig,
      showFormatToolbar = true,
      showSlashCommand = true,
      readOnly = false,
      onViewReady: onViewReadyProp,
      onViewDisposed,
      syncContentWhenFocused = false,
      enforceTitleHeading = true,
      titleTrailerElement,
      commentAnchorsEnabled = false,
      onCommentAnchorsEvent,
    } = props;

    const commentAnchorsEventRef = useRef(onCommentAnchorsEvent);
    commentAnchorsEventRef.current = onCommentAnchorsEvent;

    const taskStorage = useTaskStorageOptional();
    const reconciledInitialContent = useMemo(() => {
      if (!initialContent) {
        return initialContent;
      }

      const hydrated =
        taskSource && taskStorage
          ? (() => {
              const sourceTasks = taskStorage.getTasksForSource(taskSource);
              if (sourceTasks.length === 0) return initialContent;
              return hydrateTaskContent({
                content: initialContent,
                sourceTasks,
                getTask: taskStorage.getTask,
              });
            })()
          : initialContent;

      const normalized = normalizeTaskContent(hydrated);
      return normalized && ensureImageTrailingParagraphs(normalized);
    }, [initialContent, taskSource, taskStorage]);
    const [mountedDraft] = useState(() =>
      typeof initialDraft === "function" ? initialDraft() : initialDraft,
    );
    const previousContentRef = useRef<JSONContent | undefined>(undefined);
    const incomingDocRef = useRef<PMNode | null>(null);
    const syncIncomingRef = useRef<(() => void) | null>(null);
    const draftChangeRef = useRef(onDraftChange);
    draftChangeRef.current = onDraftChange;
    const confirmedSourceRef = useRef<(() => JSONContent) | null>(null);
    const saveStateRef = useRef({
      revision: 0,
      acknowledgedRevision: mountedDraft ? -1 : 0,
      savedRevision: 0,
    });
    const viewRef = useRef<EditorView | null>(null);
    const commandsRef = useRef<EditorCommands>(noopCommands);
    const compositionStateRef = useRef<CompositionState>({
      active: false,
      endedAt: 0,
    });

    const acknowledgeSavedContent = useCallback(() => {
      const view = viewRef.current;
      const incoming = incomingDocRef.current;
      const save = saveStateRef.current;
      if (
        view &&
        incoming &&
        save.savedRevision >= save.revision &&
        areEquivalentEditorContents(view.state.doc.toJSON(), incoming.toJSON())
      ) {
        save.acknowledgedRevision = save.revision;
      }
    }, []);

    const syncTasks = useCallback(
      (content: JSONContent) => {
        if (readOnly || !taskSource || !taskStorage) {
          return;
        }

        const previousTasks = new Map(
          taskStorage
            .getTasksForSource(taskSource)
            .map((task) => [task.taskId, task]),
        );
        taskStorage.upsertTasksForSource(
          taskSource,
          extractTasksFromContent(content, taskSource, previousTasks),
        );
      },
      [readOnly, taskSource, taskStorage],
    );

    const flushChange = useCallback(
      (doc: PMNode) => {
        const content = doc.toJSON() as JSONContent;
        syncTasks(content);
        if (!handleChange) {
          return;
        }

        const revision = saveStateRef.current.revision;
        try {
          void Promise.resolve(handleChange(content)).then(
            (confirmed) => {
              const save = saveStateRef.current;
              if (
                typeof confirmed === "function" &&
                revision === save.revision
              ) {
                try {
                  confirmed();
                } catch {
                  return;
                }
                // The host refreshed its authoritative cache. Read it directly
                // so delayed React props cannot replay an older save snapshot.
                confirmedSourceRef.current = confirmed;
                save.acknowledgedRevision = revision;
              }
              save.savedRevision = Math.max(save.savedRevision, revision);
              acknowledgeSavedContent();
              if (confirmed && revision === save.acknowledgedRevision) {
                previousContentRef.current = undefined;
                syncIncomingRef.current?.();
              }
            },
            () => {
              // The host reports persistence failures and keeps its draft.
            },
          );
        } catch {
          // A failed save must not release ownership of the local document.
        }
      },
      [handleChange, syncTasks, acknowledgeSavedContent],
    );

    const flushChangeRef = useRef(flushChange);
    flushChangeRef.current = flushChange;
    const flushLatestChange = useCallback(
      (doc: PMNode) => flushChangeRef.current(doc),
      [],
    );
    const onUpdate = useDebounceCallback(flushLatestChange, 500);
    const onUpdateRef = useRef(onUpdate);
    onUpdateRef.current = onUpdate;

    useImperativeHandle(
      ref,
      () => ({
        get view() {
          return viewRef.current;
        },
        get commands() {
          return commandsRef.current;
        },
        flushPendingChanges: () => {
          const view = viewRef.current;
          onUpdate.cancel();
          const save = saveStateRef.current;
          if (view && save.revision !== save.acknowledgedRevision) {
            flushChangeRef.current(view.state.doc);
          }
        },
      }),
      [onUpdate],
    );

    const setCompositionActive = useCallback((active: boolean) => {
      compositionStateRef.current = {
        active,
        endedAt: active ? 0 : Date.now(),
      };
    }, []);

    const plugins = useMemo(
      () => [
        reactKeys(),
        createCompositionStatePlugin(setCompositionActive),
        ...(readOnly ? [createReadOnlyPlugin()] : []),
        docChangeListenerPlugin((doc) => {
          saveStateRef.current.revision += 1;
          draftChangeRef.current?.(() => doc.toJSON() as JSONContent);
          onUpdateRef.current(doc);
        }),
        new Plugin({
          props: {
            handleDOMEvents: {
              blur() {
                void Promise.resolve().then(() => syncIncomingRef.current?.());
                return false;
              },
            },
          },
        }),
        buildInputRules(),
        ...(enforceTitleHeading ? [titleHeadingPlugin()] : []),
        ...(titleTrailerElement
          ? [titleTrailerPlugin(titleTrailerElement)]
          : []),
        taskIdentityPlugin(),
        buildKeymap(onNavigateToTitle, enforceTitleHeading),
        trailingEmptyLineClickPlugin(),
        history(),
        dropCursor(),
        gapCursor(),
        clipboardPlugin(),
        hashtagPlugin(),
        ...(commentAnchorsEnabled
          ? [
              commentAnchorsPlugin({
                onEvent: (event) => commentAnchorsEventRef.current?.(event),
              }),
            ]
          : []),
        imageTrailingParagraphPlugin(),
        searchPlugin(),
        placeholderPlugin(placeholderComponent),
        clearMarksOnEnterPlugin(),
        clipPastePlugin(),
        autolinkPlugin(),
        linkBoundaryGuardPlugin(),
        ...(onLinkOpen ? [linkOpenPlugin(onLinkOpen)] : []),
        ...(mentionConfig ? [mentionSkipPlugin()] : []),
        ...(sessionMentionDropConfig
          ? [createSessionMentionDropPlugin(sessionMentionDropConfig)]
          : []),
        ...(fileHandlerConfig ? [fileHandlerPlugin(fileHandlerConfig)] : []),
      ],
      [
        placeholderComponent,
        fileHandlerConfig,
        mentionConfig,
        sessionMentionDropConfig,
        onNavigateToTitle,
        onLinkOpen,
        enforceTitleHeading,
        titleTrailerElement,
        readOnly,
        setCompositionActive,
        commentAnchorsEnabled,
      ],
    );
    const nodeViews = useMemo(
      () => ({ ...baseNodeViews, ...wrapNodeViewComponents(extraNodeViews) }),
      [extraNodeViews],
    );

    const defaultState = useMemo(() => {
      let doc: PMNode;
      try {
        const content = mountedDraft ?? reconciledInitialContent;
        doc =
          content && content.type === "doc"
            ? PMNode.fromJSON(schema, content)
            : schema.node("doc", null, [schema.node("paragraph")]);
        if (enforceTitleHeading) {
          doc = normalizeTitleHeadingDoc(doc);
        }
      } catch {
        doc = schema.node("doc", null, [
          enforceTitleHeading
            ? schema.node("heading", { level: 1 })
            : schema.node("paragraph"),
        ]);
      }
      return EditorState.create({ doc, plugins });
    }, [mountedDraft, reconciledInitialContent, plugins, enforceTitleHeading]);
    const [editorState, setEditorState] = useState(defaultState);
    const dispatchTransaction = useCallback((transaction: Transaction) => {
      setEditorState((state) => state.applyTransaction(transaction).state);
    }, []);

    useEffect(() => {
      let retryTimeout: ReturnType<typeof setTimeout> | undefined;

      // A save started by the previous editor instance can finish after remount.
      // Its confirmation must never acknowledge edits made by this new instance.
      if (
        mountedDraft &&
        confirmedDraft &&
        saveStateRef.current.revision === 0
      ) {
        try {
          confirmedDraft();
          confirmedSourceRef.current = confirmedDraft;
          saveStateRef.current.acknowledgedRevision = 0;
          previousContentRef.current = undefined;
        } catch {
          // Keep the restored draft when its source cannot be read.
        }
      }

      const syncContent = () => {
        const view = viewRef.current;
        if (!view) return;
        if (
          !confirmedSourceRef.current &&
          previousContentRef.current === reconciledInitialContent
        )
          return;

        let doc: PMNode;
        try {
          const content =
            confirmedSourceRef.current?.() ?? reconciledInitialContent;
          if (!content || content.type !== "doc") return;
          const normalized = normalizeTaskContent(content);
          doc = PMNode.fromJSON(
            schema,
            normalized ? ensureImageTrailingParagraphs(normalized) : content,
          );
          if (enforceTitleHeading) doc = normalizeTitleHeadingDoc(doc);
        } catch {
          return;
        }
        incomingDocRef.current = doc;
        acknowledgeSavedContent();
        const currentContent = view.state.doc.toJSON() as JSONContent;
        if (areEquivalentEditorContents(currentContent, doc.toJSON())) {
          acknowledgeSavedContent();
          previousContentRef.current = reconciledInitialContent;
          return;
        }

        const save = saveStateRef.current;
        if (handleChange && save.revision !== save.acknowledgedRevision) {
          previousContentRef.current = reconciledInitialContent;
          return;
        }

        const compositionWaitMs = getEditorCompositionWaitMs(
          view,
          compositionStateRef.current,
        );
        if (compositionWaitMs > 0) {
          retryTimeout = setTimeout(syncContent, compositionWaitMs);
          return;
        }

        if (
          !shouldReplaceEditorContent({
            currentContent,
            nextContent: doc.toJSON(),
            hasFocus: view.hasFocus(),
            isComposing: false,
            syncContentWhenFocused,
          })
        ) {
          return;
        }

        try {
          const state = EditorState.create({
            doc,
            plugins: view.state.plugins,
          });
          onUpdate.cancel();
          setEditorState(state);
          previousContentRef.current = reconciledInitialContent;
        } catch {
          // invalid content
        }
      };

      syncIncomingRef.current = syncContent;
      syncContent();

      return () => {
        if (syncIncomingRef.current === syncContent)
          syncIncomingRef.current = null;
        if (retryTimeout) {
          clearTimeout(retryTimeout);
        }
      };
    }, [
      reconciledInitialContent,
      initialDraft,
      confirmedDraft,
      mountedDraft,
      syncContentWhenFocused,
      enforceTitleHeading,
      onUpdate,
      handleChange,
      acknowledgeSavedContent,
    ]);

    const onViewReady = useCallback(
      (view: EditorView) => {
        onViewReadyProp?.(view);
        syncTasks(view.state.doc.toJSON() as JSONContent);
        syncIncomingRef.current?.();
      },
      [onViewReadyProp, syncTasks],
    );

    const handleViewDisposed = useCallback(
      (view: EditorView) => {
        onUpdate.flush();
        compositionStateRef.current = {
          active: false,
          endedAt: 0,
        };
        onViewDisposed?.(view);
      },
      [onUpdate, onViewDisposed],
    );

    return (
      <TaskSourceProvider source={taskSource ?? null}>
        <AttachmentEditingContext.Provider value={!readOnly}>
          <AttachmentResolverContext.Provider value={resolveAttachment ?? null}>
            <LinkedItemOpenBehaviorContext.Provider
              value={linkedItemOpenBehavior}
            >
              <EditorErrorBoundary
                resetKey={
                  taskSource ? `${taskSource.type}:${taskSource.id}` : "note"
                }
              >
                <ProseMirror
                  state={editorState}
                  dispatchTransaction={dispatchTransaction}
                  nodeViewComponents={nodeViews}
                  editable={() => !readOnly}
                  attributes={{
                    spellCheck: "false",
                    autoComplete: "off",
                    autoCorrect: "off",
                    autoCapitalize: "off",
                    role: readOnly ? "document" : "textbox",
                    "aria-readonly": readOnly ? "true" : "false",
                    class: cn([
                      "prosemirror-editor",
                      enforceTitleHeading && "note-title-editor",
                      className,
                    ]),
                  }}
                >
                  <ProseMirrorDoc />
                  <ViewCapture
                    viewRef={viewRef}
                    onViewReady={onViewReady}
                    onViewDisposed={handleViewDisposed}
                  />
                  <EditorCommandsBridge commandsRef={commandsRef} />
                  {showFormatToolbar && !readOnly && <FormatToolbar />}
                  {showSlashCommand && !readOnly && <SlashCommandMenu />}
                  {mentionConfig && !readOnly && (
                    <MentionSuggestion config={mentionConfig} />
                  )}
                </ProseMirror>
              </EditorErrorBoundary>
            </LinkedItemOpenBehaviorContext.Provider>
          </AttachmentResolverContext.Provider>
        </AttachmentEditingContext.Provider>
      </TaskSourceProvider>
    );
  },
);
