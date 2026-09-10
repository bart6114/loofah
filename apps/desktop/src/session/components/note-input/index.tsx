import {
  forwardRef,
  type MouseEventHandler,
  type ReactNode,
  type UIEventHandler,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
} from "react";
import { useHotkeys } from "react-hotkeys-hook";

import type { NoteEditorRef } from "@hypr/editor/note";
import { cn } from "@hypr/utils";

import { Attachments } from "./attachments";
import { Enhanced } from "./enhanced";
import { EmptySummary } from "./enhanced/empty-summary";
import { FileDropTarget } from "./file-drop-target";
import { useNoteFileHandlerConfig } from "./file-handler";
import { Header, useEditorTabs } from "./header";
import { RawEditor } from "./raw";
import { SearchBar } from "./search/bar";
import { useSearch } from "./search/context";
import { Transcript } from "./transcript";

import { MemoImageStrip } from "~/session/components/memo-image-strip";
import { SessionAuthorBadge } from "~/session/components/session-author-badge";
import { SessionDate } from "~/session/components/session-date";
import {
  SessionPeopleFromTranscripts,
  useSessionPeopleTitleTrailer,
} from "~/session/components/session-people";
import { SessionTags } from "~/session/components/session-tags";
import { useCurrentNoteTab } from "~/session/components/shared";
import { TitleInput } from "~/session/components/title-input";
import { useScrollPreservation } from "~/shared/hooks/useScrollPreservation";
import type { SessionMode } from "~/store/zustand/listener/general";
import { type Tab, useTabs } from "~/store/zustand/tabs";
import { type EditorView as TabEditorView } from "~/store/zustand/tabs/schema";
import { useListener } from "~/stt/contexts";
import { useSessionTranscripts } from "~/stt/queries";

export interface NoteInputHandle {
  focus: () => void;
  focusAtStart: () => void;
  focusAtPixelWidth: (pixelWidth: number) => void;
  insertAtStartAndFocus: (content: string) => void;
  prepareForTabChange: () => void;
}

type NoteInputProps = {
  tab: Extract<Tab, { type: "sessions" }>;
  rawMd: string;
  sessionTitle: string;
  onNavigateToTitle?: (pixelWidth?: number) => void;
  onScroll?: UIEventHandler<HTMLDivElement>;
  editorTabs?: TabEditorView[];
  currentTab?: TabEditorView;
  handleTabChange?: (view: TabEditorView) => void;
  hideHeader?: boolean;
  sessionMode?: SessionMode;
  topAudioPlayer?: ReactNode;
};

export function shouldShowTranscriptTabSpinner(sessionMode: SessionMode) {
  return sessionMode === "finalizing" || sessionMode === "running_batch";
}

export const NoteInput = forwardRef<NoteInputHandle, NoteInputProps>(
  function NoteInput(props, ref) {
    if (
      props.editorTabs &&
      props.currentTab &&
      props.handleTabChange &&
      props.sessionMode !== undefined
    ) {
      return (
        <NoteInputContent
          {...props}
          ref={ref}
          editorTabs={props.editorTabs}
          currentTab={props.currentTab}
          commitTabChange={props.handleTabChange}
          sessionMode={props.sessionMode}
        />
      );
    }

    return <NoteInputWithDerivedState {...props} ref={ref} />;
  },
);

const NoteInputWithDerivedState = forwardRef<NoteInputHandle, NoteInputProps>(
  function NoteInputWithDerivedState(
    { tab, editorTabs, currentTab, handleTabChange, ...props },
    ref,
  ) {
    const fallbackEditorTabs = useEditorTabs({ sessionId: tab.id });
    const fallbackCurrentTab: TabEditorView = useCurrentNoteTab(tab);
    const updateSessionTabState = useTabs(
      (state) => state.updateSessionTabState,
    );
    const tabRef = useRef(tab);
    tabRef.current = tab;
    const sessionMode = useListener((state) => state.getSessionMode(tab.id));

    const commitTabChange = useCallback(
      (tabView: TabEditorView) => {
        if (handleTabChange) {
          handleTabChange(tabView);
          return;
        }

        updateSessionTabState(tabRef.current, {
          ...tabRef.current.state,
          view: tabView,
        });
      },
      [handleTabChange, updateSessionTabState],
    );

    return (
      <NoteInputContent
        {...props}
        ref={ref}
        tab={tab}
        editorTabs={editorTabs ?? fallbackEditorTabs}
        currentTab={currentTab ?? fallbackCurrentTab}
        commitTabChange={commitTabChange}
        sessionMode={props.sessionMode ?? sessionMode}
      />
    );
  },
);

const NoteInputContent = forwardRef<
  NoteInputHandle,
  Omit<NoteInputProps, "editorTabs" | "currentTab" | "handleTabChange"> & {
    editorTabs: TabEditorView[];
    currentTab: TabEditorView;
    commitTabChange: (view: TabEditorView) => void;
    sessionMode: SessionMode;
  }
>(
  (
    {
      tab,
      rawMd,
      sessionTitle,
      onNavigateToTitle,
      onScroll,
      editorTabs,
      currentTab,
      commitTabChange,
      hideHeader = false,
      sessionMode,
      topAudioPlayer,
    },
    ref,
  ) => {
    const internalEditorRef = useRef<NoteEditorRef>(null);
    const attachmentsDropTargetRef = useRef<HTMLDivElement>(null);
    const sessionId = tab.id;
    const renderedCurrentTab = currentTab;
    const renderedCurrentTabKey =
      renderedCurrentTab.type === "enhanced"
        ? `enhanced:${renderedCurrentTab.id}`
        : renderedCurrentTab.type;
    const transcripts = useSessionTranscripts(sessionId);
    const {
      fileDragKind,
      fileDropTargetProps,
      fileDropTargetRef,
      fileHandlerConfig,
      resetFileDrag,
    } = useNoteFileHandlerConfig(sessionId, internalEditorRef);

    const isMeetingInProgress =
      sessionMode === "active" ||
      sessionMode === "finalizing" ||
      sessionMode === "running_batch";
    const shouldShowTranscriptSpinner =
      shouldShowTranscriptTabSpinner(sessionMode);

    const pendingImageScrollRef = useRef<string | null>(null);
    const { scrollRef, onBeforeTabChange } = useScrollPreservation(
      renderedCurrentTab.type === "enhanced"
        ? `enhanced-${renderedCurrentTab.id}`
        : renderedCurrentTab.type,
      // Restoring the memo tab's saved scroll position would stomp the
      // scroll-to-image navigation from the summary's thumbnail strip.
      { skipRestoration: pendingImageScrollRef.current !== null },
    );

    useImperativeHandle(
      ref,
      () => ({
        focus: () => internalEditorRef.current?.commands.focus(),
        focusAtStart: () => internalEditorRef.current?.commands.focusAtStart(),
        focusAtPixelWidth: (px) =>
          internalEditorRef.current?.commands.focusAtPixelWidth(px),
        insertAtStartAndFocus: (content) =>
          internalEditorRef.current?.commands.insertAtStartAndFocus(content),
        prepareForTabChange: () => {
          internalEditorRef.current?.flushPendingChanges();
          onBeforeTabChange();
        },
      }),
      [currentTab, onBeforeTabChange],
    );

    const handleTabChange = useCallback(
      (tabView: TabEditorView) => {
        if (
          isSameEditorView(tabView, currentTab) ||
          isSameEditorView(tabView, renderedCurrentTab)
        ) {
          return;
        }

        internalEditorRef.current?.flushPendingChanges();
        onBeforeTabChange();
        commitTabChange(tabView);
      },
      [commitTabChange, currentTab, onBeforeTabChange, renderedCurrentTab],
    );

    const handleAdjacentViewShortcut = useCallback(
      (direction: "previous" | "next") => {
        if (editorTabs.length <= 1) {
          return;
        }

        const currentIndex = editorTabs.findIndex((editorTab) =>
          isSameEditorView(editorTab, renderedCurrentTab),
        );
        if (currentIndex === -1) {
          return;
        }

        const nextIndex =
          direction === "previous"
            ? (currentIndex - 1 + editorTabs.length) % editorTabs.length
            : (currentIndex + 1) % editorTabs.length;
        const nextView = editorTabs[nextIndex];
        if (nextView) {
          handleTabChange(nextView);
        }
      },
      [editorTabs, handleTabChange, renderedCurrentTab],
    );

    useHotkeys(
      "mod+alt+left",
      () => handleAdjacentViewShortcut("previous"),
      {
        preventDefault: true,
        enableOnFormTags: true,
        enableOnContentEditable: true,
      },
      [handleAdjacentViewShortcut],
    );

    useHotkeys(
      "mod+alt+right",
      () => handleAdjacentViewShortcut("next"),
      {
        preventDefault: true,
        enableOnFormTags: true,
        enableOnContentEditable: true,
      },
      [handleAdjacentViewShortcut],
    );

    useEffect(() => {
      if (renderedCurrentTab.type === "raw" && isMeetingInProgress) {
        requestAnimationFrame(() => {
          internalEditorRef.current?.commands.focus();
        });
      }
    }, [renderedCurrentTab, isMeetingInProgress]);

    const search = useSearch();
    const showSearchBar = search?.isVisible ?? false;
    const isEditableTab =
      renderedCurrentTab.type === "enhanced" ||
      renderedCurrentTab.type === "raw";

    useEffect(() => {
      resetFileDrag();
    }, [renderedCurrentTabKey, resetFileDrag]);

    useEffect(() => {
      search?.close();
    }, [currentTab]);

    const handleContainerMouseDown: MouseEventHandler<HTMLDivElement> = (
      event,
    ) => {
      if (!isEditableTab) {
        return;
      }

      if (event.button !== 0) {
        return;
      }

      const target = event.target;
      if (!(target instanceof Element)) {
        return;
      }

      if (target.closest(".ProseMirror") !== null) {
        return;
      }

      if (
        target.closest(
          "button, a, input, textarea, select, [role='button'], [contenteditable='true']",
        ) !== null
      ) {
        return;
      }

      if (event.currentTarget.querySelector(".ProseMirror") === null) {
        return;
      }

      event.preventDefault();
      internalEditorRef.current?.commands.focusAtTrailingEmptyLine();
    };

    const handleMemoImageClick = useCallback(
      (src: string) => {
        pendingImageScrollRef.current = src;
        handleTabChange({ type: "raw" });
        // The raw editor mounts asynchronously after the tab switch, so poll
        // briefly for the matching image before giving up.
        const deadline = Date.now() + 3000;
        const tryScroll = () => {
          if (pendingImageScrollRef.current !== src) {
            return;
          }
          const imgs = scrollRef.current?.querySelectorAll(
            "img.prosemirror-image",
          );
          const target = imgs
            ? Array.from(imgs).find((img) => img.getAttribute("src") === src)
            : undefined;
          if (target) {
            pendingImageScrollRef.current = null;
            target.scrollIntoView({ behavior: "smooth", block: "center" });
            return;
          }
          if (Date.now() < deadline) {
            requestAnimationFrame(tryScroll);
          } else {
            pendingImageScrollRef.current = null;
          }
        };
        requestAnimationFrame(tryScroll);
      },
      [handleTabChange, scrollRef],
    );

    const peopleTrailer = useSessionPeopleTitleTrailer(
      transcripts,
      <>
        <SessionAuthorBadge sessionId={sessionId} className="mt-1 mb-3" />
        <SessionTags sessionId={sessionId} className="mt-1 mb-3" />
        {renderedCurrentTab.type === "enhanced" && (
          <MemoImageStrip
            sessionId={sessionId}
            className="mt-1 mb-3"
            onImageClick={handleMemoImageClick}
          />
        )}
      </>,
    );

    return (
      <div
        ref={
          renderedCurrentTab.type === "attachments"
            ? attachmentsDropTargetRef
            : undefined
        }
        data-allow-file-drop={
          renderedCurrentTab.type === "attachments" ? "true" : undefined
        }
        className="relative -mx-2 flex h-full flex-col"
      >
        {!hideHeader && (
          <div className="relative px-2">
            <Header
              sessionId={sessionId}
              editorTabs={editorTabs}
              currentTab={renderedCurrentTab}
              handleTabChange={handleTabChange}
              isTranscribing={shouldShowTranscriptSpinner}
            />
          </div>
        )}

        {showSearchBar && isEditableTab && (
          <div className="px-3 pt-1">
            <SearchBar editorRef={internalEditorRef} />
          </div>
        )}

        {topAudioPlayer && <div className="px-3 pt-1.5">{topAudioPlayer}</div>}

        <div
          ref={isEditableTab ? fileDropTargetRef : undefined}
          {...(isEditableTab ? fileDropTargetProps : {})}
          className="relative flex-1 overflow-hidden"
        >
          <FileDropTarget kind={isEditableTab ? fileDragKind : null} />
          <div
            ref={scrollRef}
            onMouseDown={handleContainerMouseDown}
            onScroll={onScroll}
            className={cn([
              "h-full px-3",
              "pt-2",
              renderedCurrentTab.type === "transcript"
                ? "overflow-hidden pb-0"
                : "overflow-auto pb-6",
            ])}
          >
            {isEditableTab && (
              <div className="mb-0.5">
                <SessionDate sessionId={sessionId} />
              </div>
            )}
            {peopleTrailer.portal}
            {renderedCurrentTab.type === "summary" && (
              <EmptySummary sessionId={sessionId} sessionTitle={sessionTitle} />
            )}
            {renderedCurrentTab.type === "enhanced" && (
              <Enhanced
                ref={internalEditorRef}
                sessionId={sessionId}
                sessionTitle={sessionTitle}
                enhancedNoteId={renderedCurrentTab.id}
                fileHandlerConfig={fileHandlerConfig}
                onNavigateToTitle={onNavigateToTitle}
                titleTrailerElement={peopleTrailer.element}
              />
            )}
            {renderedCurrentTab.type === "raw" && (
              <RawEditor
                ref={internalEditorRef}
                sessionId={sessionId}
                rawMd={rawMd}
                sessionTitle={sessionTitle}
                fileHandlerConfig={fileHandlerConfig}
                onNavigateToTitle={onNavigateToTitle}
                titleTrailerElement={peopleTrailer.element}
              />
            )}
            {renderedCurrentTab.type === "transcript" && (
              <div className="flex h-full min-h-0 flex-col">
                <div data-session-transcript-title className="mb-4 shrink-0">
                  <div className="mb-0.5">
                    <SessionDate sessionId={sessionId} />
                  </div>
                  <TitleInput tab={tab} />
                  {/* mt-2 = the editor title's 0.25rem margin-bottom plus the
                      trailer row's mt-1, so the title→pills gap matches. */}
                  <SessionPeopleFromTranscripts
                    transcripts={transcripts}
                    className="mt-2"
                  />
                  <SessionAuthorBadge sessionId={sessionId} className="mt-2" />
                  <SessionTags sessionId={sessionId} className="mt-2" />
                </div>
                <div className="min-h-0 flex-1">
                  <Transcript
                    sessionId={sessionId}
                    transcripts={transcripts}
                    scrollRef={scrollRef}
                  />
                </div>
              </div>
            )}
            {renderedCurrentTab.type === "attachments" && (
              <Attachments
                sessionId={sessionId}
                dropTargetRef={attachmentsDropTargetRef}
              >
                <div className="mb-4">
                  <div className="mb-0.5">
                    <SessionDate sessionId={sessionId} />
                  </div>
                  <TitleInput tab={tab} />
                  <SessionPeopleFromTranscripts
                    transcripts={transcripts}
                    className="mt-2"
                  />
                  <SessionAuthorBadge sessionId={sessionId} className="mt-2" />
                  <SessionTags sessionId={sessionId} className="mt-2" />
                </div>
              </Attachments>
            )}
          </div>
        </div>
      </div>
    );
  },
);

function isSameEditorView(left: TabEditorView, right: TabEditorView): boolean {
  if (left.type !== right.type) {
    return false;
  }

  if (left.type === "enhanced" && right.type === "enhanced") {
    return left.id === right.id;
  }

  return true;
}
