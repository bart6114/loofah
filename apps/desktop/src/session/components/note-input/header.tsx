import { useLingui } from "@lingui/react/macro";
import { writeText as writeClipboardText } from "@tauri-apps/plugin-clipboard-manager";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  HeartIcon,
  PaperclipIcon,
  PlusIcon,
  SearchIcon,
  XIcon,
} from "lucide-react";
import { useCallback, useMemo, useRef, useState } from "react";

import { json2md, parseJsonContent } from "@hypr/editor/markdown";
import { DancingSticks } from "@hypr/ui/components/ui/dancing-sticks";
import {
  AppFloatingPanel,
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@hypr/ui/components/ui/popover";
import { Spinner } from "@hypr/ui/components/ui/spinner";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn } from "@hypr/utils";

import { useAITaskTask } from "~/ai/hooks";
import {
  isMainAITaskHostWindow,
  requestMainEnhance,
} from "~/ai/task-window-sync";
import * as AudioPlayer from "~/audio-player";
import { getEnhancerService } from "~/services/enhancer";
import { useEnhancedNoteActions } from "~/session/components/note-input/enhanced-actions";
import { useRegenerateTranscript } from "~/session/components/note-input/transcript/actions";
import {
  buildTranscriptExportSegments,
  formatTranscriptExportSegments,
} from "~/session/components/note-input/transcript/export-data";
import { useSessionTranscriptRenderData } from "~/session/components/note-input/transcript/render-request-hooks";
import { useCanShowTranscript } from "~/session/components/shared";
import { useSummarySource } from "~/session/hooks/useSummarySource";
import {
  deleteEnhancedNote,
  useEnhancedNote,
  useEnhancedNoteRecords,
  useSession,
} from "~/session/queries";
import {
  type MenuItemDef,
  useNativeContextMenu,
} from "~/shared/hooks/useNativeContextMenu";
import { useWebResources } from "~/shared/ui/resource-list";
import { flushDatabaseWrites } from "~/shared/write-queue";
import { createTaskId } from "~/store/zustand/ai-task/task-configs";
import { type EditorView } from "~/store/zustand/tabs/schema";
import { useListener } from "~/stt/contexts";
import {
  filterWebTemplatesAgainstUserTemplates,
  DEFAULT_TEMPLATE_ICON,
  parseWebTemplates,
  TemplateIconGlyph,
  useOpenTemplatesTab,
  useCreateTemplate,
  useUserTemplate,
  useUserTemplates,
  type WebTemplate,
  type TemplateIcon,
} from "~/templates";

function getStoredNoteMarkdown(content: string | undefined) {
  const trimmed = content?.trim() ?? "";

  if (!trimmed) {
    return "";
  }

  if (!trimmed.startsWith("{")) {
    return trimmed;
  }

  return json2md(parseJsonContent(trimmed)).trim();
}

const UUID_TITLE_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const ISO_TITLE_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/;

function IconHeaderView({
  isActive,
  label,
  hoverLabel,
  icon,
  onClick,
  onContextMenu,
  title,
  size = "tray",
  className,
  iconOnly = false,
}: {
  isActive: boolean;
  label: string;
  hoverLabel?: string;
  icon: React.ReactNode;
  onClick?: () => void;
  onContextMenu?: React.MouseEventHandler<HTMLButtonElement>;
  title?: string;
  size?: "tray" | "standalone";
  className?: string;
  iconOnly?: boolean;
}) {
  return (
    <button
      data-main-area-window-drag-region
      data-tauri-drag-region="false"
      type="button"
      aria-label={label}
      aria-current={isActive ? "page" : undefined}
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={title}
      data-hover-label={hoverLabel}
      className={iconHeaderViewClassName(
        isActive,
        size,
        cn([
          "max-w-40 gap-1.5 px-2.5",
          hoverLabel
            ? "after:hidden after:min-w-0 after:truncate after:text-xs after:font-medium after:content-[attr(data-hover-label)] hover:after:block"
            : null,
          className,
        ]),
      )}
    >
      {icon}
      <span
        className={cn([
          iconOnly ? "sr-only" : "min-w-0 truncate text-xs font-medium",
          hoverLabel ? "group-hover/header-view:hidden" : null,
        ])}
      >
        {label}
      </span>
    </button>
  );
}

function HeaderViewAttachments({
  isActive,
  onClick,
}: {
  isActive: boolean;
  onClick: () => void;
}) {
  const { t } = useLingui();

  return (
    <IconHeaderView
      isActive={isActive}
      label={t`Attachments`}
      title={t`Attachments`}
      icon={<PaperclipIcon className="size-3.5" />}
      iconOnly
      onClick={onClick}
      className="w-7 px-0"
    />
  );
}

function iconHeaderViewClassName(
  isActive: boolean,
  size: "tray" | "standalone" = "tray",
  className?: string,
) {
  const heightClassName = size === "tray" ? "h-[26px]" : "h-7";

  return cn([
    "group/header-view flex shrink-0 items-center justify-center rounded-md transition-colors select-none [&>svg]:shrink-0",
    isActive
      ? [
          "text-foreground bg-card shadow-xs",
          "dark:bg-accent dark:text-foreground dark:shadow-none",
        ]
      : [
          "text-muted-foreground/70",
          "hover:bg-background/60 hover:text-foreground",
          "dark:hover:bg-accent/80 dark:hover:text-foreground",
        ],
    heightClassName,
    className,
  ]);
}

function getEnhancedNoteTitle({
  rawTitle,
  templateTitle,
  templateId,
}: {
  rawTitle: unknown;
  templateTitle: string | null;
  templateId: string | undefined;
}) {
  if (templateTitle) {
    return templateTitle;
  }

  const title = typeof rawTitle === "string" ? rawTitle.trim() : "";
  if (!title) {
    return "Summary";
  }

  const isGeneratedTitle =
    title === "Summary" ||
    title === templateId ||
    UUID_TITLE_RE.test(title) ||
    ISO_TITLE_RE.test(title);

  if (isGeneratedTitle) {
    return "Summary";
  }

  return title;
}

async function copyTextToClipboard(
  text: string,
  messages?: {
    success: string;
    error: string;
  },
) {
  try {
    try {
      await navigator.clipboard.write([
        new ClipboardItem({
          "text/plain": new Blob([text], {
            type: "text/plain",
          }),
          "text/markdown": new Blob([text], {
            type: "text/markdown",
          }),
        }),
      ]);
    } catch {
      // WKWebView rejects text/markdown ClipboardItems, and the async
      // clipboard API needs transient user activation that expires after
      // awaited IPC calls — the Tauri plugin has neither restriction.
      await writeClipboardText(text);
    }

    if (messages) {
      sonnerToast.success(messages.success);
    }

    return true;
  } catch (error) {
    console.error("Failed to copy note content", error);

    if (messages) {
      sonnerToast.error(messages.error);
    }

    return false;
  }
}

export type TemplateSelection = {
  templateId: string | null;
  title: string;
};

function HeaderViewRaw({
  isActive,
  onClick = () => {},
  sessionId,
  standalone = false,
}: {
  isActive: boolean;
  onClick?: () => void;
  sessionId: string;
  standalone?: boolean;
}) {
  if (!isActive) {
    return (
      <HeaderViewRawButton
        isActive={isActive}
        onClick={onClick}
        standalone={standalone}
      />
    );
  }

  return (
    <HeaderViewRawActive
      isActive={isActive}
      onClick={onClick}
      sessionId={sessionId}
      standalone={standalone}
    />
  );
}

function HeaderViewRawButton({
  isActive,
  onClick,
  onContextMenu,
  standalone,
}: {
  isActive: boolean;
  onClick?: () => void;
  onContextMenu?: React.MouseEventHandler<HTMLButtonElement>;
  standalone: boolean;
}) {
  const { t } = useLingui();

  return (
    <IconHeaderView
      isActive={isActive}
      label={t`Note`}
      icon={null}
      onClick={onClick}
      onContextMenu={onContextMenu}
      size={standalone ? "standalone" : "tray"}
      className={standalone ? "border-0 shadow-none" : undefined}
    />
  );
}

function HeaderViewRawActive({
  isActive,
  onClick,
  sessionId,
  standalone,
}: {
  isActive: boolean;
  onClick?: () => void;
  sessionId: string;
  standalone: boolean;
}) {
  const rawMd = useSession(sessionId)?.raw_md;
  const memoMarkdown = useMemo(() => getStoredNoteMarkdown(rawMd), [rawMd]);
  const contextMenu = useMemo<MenuItemDef[]>(
    () => [
      {
        id: `copy-memo-${sessionId}`,
        text: "Copy",
        action: () => {
          void copyTextToClipboard(memoMarkdown, {
            success: "Memo copied to clipboard",
            error: "Failed to copy memo",
          });
        },
        disabled: memoMarkdown.length === 0,
      },
    ],
    [memoMarkdown, sessionId],
  );
  const showContextMenu = useNativeContextMenu(contextMenu);

  return (
    <HeaderViewRawButton
      isActive={isActive}
      onClick={onClick}
      onContextMenu={showContextMenu}
      standalone={standalone}
    />
  );
}

function HeaderViewEnhanced({
  isActive,
  onClick = () => {},
  sessionId,
  enhancedNoteId,
  canRemove = false,
  onRemove,
  onSelectNote,
}: {
  isActive: boolean;
  onClick?: () => void;
  sessionId: string;
  enhancedNoteId: string;
  canRemove?: boolean;
  onRemove?: () => void;
  onSelectNote?: (enhancedNoteId: string) => void;
}) {
  if (!isActive) {
    return (
      <HeaderViewEnhancedInactive
        enhancedNoteId={enhancedNoteId}
        onClick={onClick}
      />
    );
  }

  return (
    <HeaderViewEnhancedActive
      sessionId={sessionId}
      enhancedNoteId={enhancedNoteId}
      canRemove={canRemove}
      onRemove={onRemove}
      onSelectNote={onSelectNote}
    />
  );
}

function useEnhancedViewTitle(enhancedNoteId: string) {
  const enhancedNote = useEnhancedNote(enhancedNoteId);
  const rawTitle = enhancedNote?.title;
  const templateId = enhancedNote?.templateId;
  const { data: template } = useUserTemplate(templateId);
  const templateTitle = template?.title?.trim() || null;
  const viewTitle = getEnhancedNoteTitle({
    rawTitle,
    templateTitle,
    templateId,
  });

  return {
    viewTitle,
    templateTooltip:
      templateId && templateTitle
        ? `${templateTitle} was used to generate this summary.`
        : undefined,
  };
}

function useEnhancedViewGenerating(enhancedNoteId: string) {
  const taskId = createTaskId(enhancedNoteId, "enhance");
  const enhanceTask = useAITaskTask(taskId, "enhance");

  return enhanceTask.isGenerating;
}

function HeaderViewEnhancedInactive({
  onClick = () => {},
  enhancedNoteId,
}: {
  enhancedNoteId: string;
  onClick?: () => void;
}) {
  const { viewTitle, templateTooltip } = useEnhancedViewTitle(enhancedNoteId);
  const isGenerating = useEnhancedViewGenerating(enhancedNoteId);

  return (
    <button
      data-main-area-window-drag-region
      data-tauri-drag-region="false"
      type="button"
      aria-label={viewTitle}
      onClick={onClick}
      title={templateTooltip}
      className={iconHeaderViewClassName(
        false,
        "tray",
        "max-w-40 gap-1.5 px-2.5",
      )}
    >
      {isGenerating ? <Spinner size={14} className="shrink-0" /> : null}
      <span className="min-w-0 truncate text-xs font-medium">{viewTitle}</span>
    </button>
  );
}

function HeaderViewEnhancedActive({
  sessionId,
  enhancedNoteId,
  canRemove = false,
  onRemove,
  onSelectNote,
}: {
  sessionId: string;
  enhancedNoteId: string;
  canRemove?: boolean;
  onRemove?: () => void;
  onSelectNote?: (enhancedNoteId: string) => void;
}) {
  const { isGenerating, isError, onRegenerate } = useEnhanceLogic(
    sessionId,
    enhancedNoteId,
  );
  const hasSource = useSummarySource(sessionId);
  const content = useEnhancedNote(enhancedNoteId)?.content;
  const { viewTitle, templateTooltip } = useEnhancedViewTitle(enhancedNoteId);
  const noteMarkdown = useMemo(() => getStoredNoteMarkdown(content), [content]);

  const handleCopy = useCallback(() => {
    return copyTextToClipboard(noteMarkdown, {
      success: `${viewTitle} copied to clipboard`,
      error: `Failed to copy ${viewTitle}`,
    });
  }, [noteMarkdown, viewTitle]);
  const handleRegenerate = useCallback(() => {
    void onRegenerate(null);
  }, [onRegenerate]);
  const handleSelectTemplate = useCallback(
    (selection: TemplateSelection) => {
      if (isGenerating || !hasSource) {
        return;
      }

      if (!isMainAITaskHostWindow()) {
        void flushDatabaseWrites([`session:${sessionId}:note`])
          .then(() =>
            requestMainEnhance(sessionId, {
              templateId: selection.templateId,
              targetNoteId: enhancedNoteId,
              templateTitle: selection.templateId ? selection.title : undefined,
            }),
          )
          .then((result) => {
            if (result.type === "no_model")
              sonnerToast.error(
                "Set up Intelligence in Settings before generating a summary.",
              );
          })
          .catch((error) =>
            sonnerToast.error(
              error instanceof Error ? error.message : String(error),
            ),
          );
        return;
      }
      const service = getEnhancerService();
      if (!service) {
        return;
      }

      onSelectNote?.(enhancedNoteId);

      void Promise.resolve(
        service.enhance(sessionId, {
          templateId: selection.templateId,
          targetNoteId: enhancedNoteId,
          templateTitle: selection.templateId ? selection.title : undefined,
        }),
      )
        .then((result) => {
          if (
            (result.type === "started" || result.type === "already_active") &&
            result.noteId !== enhancedNoteId
          ) {
            onSelectNote?.(result.noteId);
          }
        })
        .catch((error) => {
          console.error("[enhancer] failed to replace summary template", error);
          sonnerToast.error(
            error instanceof Error ? error.message : String(error),
          );
        });
    },
    [enhancedNoteId, isGenerating, hasSource, onSelectNote, sessionId],
  );
  const contextMenu = useMemo<MenuItemDef[]>(() => {
    const items: MenuItemDef[] = [
      {
        id: `copy-enhanced-${enhancedNoteId}`,
        text: "Copy",
        action: () => {
          void handleCopy();
        },
        disabled: noteMarkdown.length === 0,
      },
      {
        id: `regenerate-enhanced-${enhancedNoteId}`,
        text: "Regenerate",
        action: handleRegenerate,
        disabled: isGenerating || !hasSource,
      },
    ];

    if (canRemove) {
      items.push({ separator: true });
      items.push({
        id: `remove-enhanced-${enhancedNoteId}`,
        text: "Remove",
        action: () => {
          onRemove?.();
        },
        disabled: isGenerating || !onRemove,
      });
    }

    return items;
  }, [
    canRemove,
    enhancedNoteId,
    handleCopy,
    handleRegenerate,
    hasSource,
    isGenerating,
    noteMarkdown.length,
    onRemove,
  ]);
  const showContextMenu = useNativeContextMenu(contextMenu);
  const templateMenuTrigger = (
    <button
      data-main-area-window-drag-region
      data-tauri-drag-region="false"
      type="button"
      aria-label={viewTitle}
      aria-current="page"
      aria-disabled={isGenerating}
      tabIndex={isGenerating ? -1 : 0}
      onClick={(event) => event.stopPropagation()}
      onPointerDown={(event) => event.stopPropagation()}
      onContextMenu={showContextMenu}
      title={templateTooltip}
      className={iconHeaderViewClassName(
        true,
        "tray",
        cn([
          "max-w-56 min-w-[62px] gap-1.5 pr-1.5 pl-2",
          isGenerating ? "cursor-not-allowed opacity-70" : "cursor-pointer",
          isError
            ? [
                "text-destructive hover:bg-destructive/10 hover:text-destructive focus-visible:bg-destructive/10",
              ]
            : [
                "focus-visible:text-foreground focus-visible:bg-card",
                "dark:focus-visible:text-foreground dark:focus-visible:bg-accent",
              ],
        ]),
      )}
    >
      {isGenerating ? <Spinner size={14} className="shrink-0" /> : null}
      <span className="min-w-0 truncate text-xs font-medium">{viewTitle}</span>
      <ChevronDownIcon className="size-3.5" />
    </button>
  );

  return (
    <TemplatePickerPopover
      onSelectTemplate={handleSelectTemplate}
      trigger={templateMenuTrigger}
    />
  );
}

function HeaderViewTranscript({
  isActive,
  isTranscribing,
  onClick = () => {},
  sessionId,
}: {
  isActive: boolean;
  isTranscribing: boolean;
  onClick?: () => void;
  sessionId: string;
}) {
  const liveState = useTranscriptLiveViewState(sessionId);

  if (!isActive) {
    return (
      <HeaderViewTranscriptButton
        isActive={isActive}
        isTranscribing={isTranscribing}
        onClick={onClick}
        live={liveState.live}
      />
    );
  }

  return (
    <HeaderViewTranscriptActive
      isActive={isActive}
      isTranscribing={isTranscribing}
      onClick={onClick}
      sessionId={sessionId}
      live={liveState.live}
    />
  );
}

function HeaderViewTranscriptButton({
  isActive,
  isTranscribing,
  onClick,
  onContextMenu,
  live,
}: {
  isActive: boolean;
  isTranscribing: boolean;
  onClick?: () => void;
  onContextMenu?: React.MouseEventHandler<HTMLButtonElement>;
  live?: {
    amplitude: number;
    degraded: boolean;
  };
}) {
  const { t } = useLingui();

  return (
    <IconHeaderView
      isActive={isActive}
      label={t`Transcript`}
      hoverLabel={undefined}
      icon={
        live ? (
          <HeaderViewTranscriptLiveIcon live={live} />
        ) : isTranscribing ? (
          <Spinner size={14} className="shrink-0" />
        ) : null
      }
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={undefined}
      className={cn([
        live
          ? [
              "group/transcript-live",
              isActive ? "w-[98px] min-w-[98px] gap-1.5 pr-1.5 pl-2" : null,
              isActive
                ? live.degraded
                  ? ["bg-brand/10 text-brand hover:bg-brand/20"]
                  : ["bg-recording/10 text-recording hover:bg-recording/20"]
                : null,
            ]
          : null,
      ])}
    />
  );
}

function HeaderViewTranscriptLiveIcon({
  live,
}: {
  live: {
    amplitude: number;
    degraded: boolean;
  };
}) {
  const color = live.degraded ? "hsl(var(--brand))" : "hsl(var(--recording))";

  return (
    <span className="relative flex size-4 items-center justify-center">
      <DancingSticks
        amplitude={live.amplitude}
        color={color}
        height={16}
        width={16}
      />
    </span>
  );
}

function useTranscriptLiveViewState(sessionId: string) {
  const { amplitude, degraded, mode } = useListener((state) => {
    const mode = state.getSessionMode(sessionId);
    return {
      amplitude: state.live.amplitude,
      degraded: state.live.degraded,
      mode,
    };
  });
  return {
    live:
      mode === "active"
        ? {
            amplitude: Math.min(
              Math.hypot(amplitude.mic, amplitude.speaker),
              1,
            ),
            degraded: Boolean(degraded),
          }
        : undefined,
  };
}

function HeaderViewTranscriptActive({
  isActive,
  isTranscribing,
  onClick,
  sessionId,
  live,
}: {
  isActive: boolean;
  isTranscribing: boolean;
  onClick?: () => void;
  sessionId: string;
  live?: {
    amplitude: number;
    degraded: boolean;
  };
}) {
  const regenerate = useRegenerateTranscript(sessionId);
  const { request: transcriptExportRequest } =
    useSessionTranscriptRenderData(sessionId);
  const {
    audioExists,
    audioExistsResolved,
    deleteRecording,
    isDeletingRecording,
  } = AudioPlayer.useAudioPlayer();
  const sessionMode = useListener((state) => state.getSessionMode(sessionId));
  const canCopyTranscript = Boolean(transcriptExportRequest);
  const handleCopyTranscript = useCallback(async () => {
    if (!transcriptExportRequest) {
      return;
    }

    try {
      const transcriptSegments = await buildTranscriptExportSegments(
        transcriptExportRequest,
      );
      const transcriptText = formatTranscriptExportSegments(transcriptSegments);
      if (!transcriptText) {
        return;
      }

      await copyTextToClipboard(transcriptText, {
        success: "Transcript copied to clipboard",
        error: "Failed to copy transcript",
      });
    } catch (error) {
      console.error("Failed to copy transcript", error);
      sonnerToast.error("Failed to copy transcript");
    }
  }, [transcriptExportRequest]);
  const handleDeleteRecording = useCallback(() => {
    void deleteRecording();
  }, [deleteRecording]);
  const contextMenu = useMemo<MenuItemDef[]>(() => {
    const items: MenuItemDef[] = [
      {
        id: `copy-transcript-${sessionId}`,
        text: "Copy",
        action: () => {
          void handleCopyTranscript();
        },
        disabled: !canCopyTranscript,
      },
    ];

    if (audioExistsResolved && sessionMode === "inactive" && audioExists) {
      items.push({
        id: `regenerate-transcript-${sessionId}`,
        text: "Re-transcribe",
        action: () => {
          void regenerate();
        },
      });
    }

    if (audioExists) {
      items.push({
        id: `delete-recording-${sessionId}`,
        text: "Delete recording",
        action: handleDeleteRecording,
        disabled: isDeletingRecording,
      });
    }

    return items;
  }, [
    audioExists,
    audioExistsResolved,
    canCopyTranscript,
    handleCopyTranscript,
    handleDeleteRecording,
    isDeletingRecording,
    regenerate,
    sessionMode,
    sessionId,
  ]);
  const showContextMenu = useNativeContextMenu(contextMenu);

  return (
    <HeaderViewTranscriptButton
      isActive={isActive}
      isTranscribing={isTranscribing}
      onClick={onClick}
      onContextMenu={showContextMenu}
      live={live}
    />
  );
}

export function TemplatePickerPopover({
  onSelectTemplate,
  trigger,
}: {
  onSelectTemplate: (selection: TemplateSelection) => void;
  trigger: React.ReactNode;
}) {
  const { t } = useLingui();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const resultRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const userTemplates = useUserTemplates();
  const createTemplate = useCreateTemplate();
  const { data: rawWebTemplates = [] } =
    useWebResources<Record<string, unknown>>("templates");
  const webTemplates = useMemo(
    () =>
      filterWebTemplatesAgainstUserTemplates({
        userTemplates,
        webTemplates: parseWebTemplates(rawWebTemplates),
      }),
    [rawWebTemplates, userTemplates],
  );
  const openTemplatesTab = useOpenTemplatesTab();

  const handleUseTemplate = useCallback(
    (selection: TemplateSelection) => {
      setOpen(false);
      setSearch("");
      resultRefs.current = [];

      onSelectTemplate(selection);
    },
    [onSelectTemplate],
  );

  const handleOpenChange = useCallback((nextOpen: boolean) => {
    setOpen(nextOpen);
    if (!nextOpen) {
      setSearch("");
      resultRefs.current = [];
    }
  }, []);

  const handleWebTemplateClick = useCallback(
    (template: WebTemplate) => {
      setOpen(false);
      setSearch("");
      resultRefs.current = [];

      void (async () => {
        const templateId = await createTemplate({
          title: template.title,
          description: template.description,
          category: template.category,
          icon: template.icon,
          targets: template.targets,
          sections: template.sections ?? [],
        });
        if (!templateId) {
          return;
        }

        onSelectTemplate({
          templateId,
          title: template.title || "Untitled",
        });
      })();
    },
    [createTemplate, onSelectTemplate],
  );

  const handleCreateTemplate = useCallback(
    (title?: string) => {
      const nextTitle = title?.trim() || "New Template";

      setOpen(false);
      setSearch("");
      resultRefs.current = [];

      void (async () => {
        const templateId = await createTemplate({
          title: nextTitle,
          description: "",
          sections: [],
        });
        if (!templateId) {
          return;
        }

        openTemplatesTab({
          selectedMineId: templateId,
          selectedWebIndex: null,
          isWebMode: false,
          showHomepage: false,
        });
      })();
    },
    [createTemplate, openTemplatesTab],
  );
  const handleSeeAllTemplates = useCallback(() => {
    setOpen(false);
    setSearch("");
    resultRefs.current = [];
    openTemplatesTab({
      showHomepage: false,
      isWebMode: true,
      selectedMineId: null,
      selectedWebIndex: 0,
    });
  }, [openTemplatesTab]);

  const trimmedSearch = search.trim();
  const searchQuery = search.trim().toLowerCase();
  const favoriteTemplates = useMemo(
    () => sortFavoriteTemplates(userTemplates),
    [userTemplates],
  );
  const otherTemplates = useMemo(
    () => sortOtherTemplates(userTemplates),
    [userTemplates],
  );

  const filteredFavoriteTemplates = useMemo(() => {
    if (!searchQuery) {
      return favoriteTemplates;
    }

    return favoriteTemplates.filter((template) =>
      matchesTemplateSearch(template, searchQuery),
    );
  }, [favoriteTemplates, searchQuery]);

  const filteredOtherTemplates = useMemo(() => {
    if (!searchQuery) {
      return otherTemplates;
    }

    return otherTemplates.filter((template) =>
      matchesTemplateSearch(template, searchQuery),
    );
  }, [otherTemplates, searchQuery]);

  const hasSearch = searchQuery.length > 0;
  const filteredWebTemplates = useMemo(() => {
    if (!searchQuery) {
      return webTemplates;
    }

    return webTemplates.filter(
      (template) =>
        template.title?.toLowerCase().includes(searchQuery) ||
        template.description?.toLowerCase().includes(searchQuery) ||
        template.category?.toLowerCase().includes(searchQuery) ||
        template.targets?.some((target) =>
          target.toLowerCase().includes(searchQuery),
        ),
    );
  }, [searchQuery, webTemplates]);
  const templateItems = useMemo<
    Array<{
      key: string;
      title: string;
      icon: TemplateIcon;
      isFavorite?: boolean;
      onClick: () => void;
    }>
  >(() => {
    const favoriteItems = filteredFavoriteTemplates.map((template) => ({
      key: template.id,
      title: template.title || "Untitled",
      icon: template.icon,
      isFavorite: true,
      onClick: () =>
        handleUseTemplate({
          templateId: template.id,
          title: template.title || "Untitled",
        }),
    }));

    const userItems = filteredOtherTemplates.map((template) => ({
      key: template.id,
      title: template.title || "Untitled",
      icon: template.icon,
      onClick: () =>
        handleUseTemplate({
          templateId: template.id,
          title: template.title || "Untitled",
        }),
    }));

    const webItems = filteredWebTemplates.map((template, index) => ({
      key: template.slug || `library-${index}`,
      title: template.title || "Untitled",
      icon: template.icon,
      onClick: () => handleWebTemplateClick(template),
    }));

    const otherItems = [...userItems, ...webItems].sort((a, b) =>
      a.title.localeCompare(b.title),
    );

    return [...favoriteItems, ...otherItems];
  }, [
    filteredFavoriteTemplates,
    filteredOtherTemplates,
    filteredWebTemplates,
    handleWebTemplateClick,
    handleUseTemplate,
  ]);
  const resultSections = useMemo<
    Array<{
      key: string;
      title: string;
      icon?: React.ReactNode;
      uppercase?: boolean;
      showHeader?: boolean;
      emptyMessage?: string;
      items: Array<{
        key: string;
        title: string;
        icon: TemplateIcon;
        isFavorite?: boolean;
        onClick: () => void;
      }>;
    }>
  >(() => {
    const autoSection = {
      key: "auto",
      title: "Auto",
      showHeader: false,
      items: [
        {
          key: "auto",
          title: "Auto",
          icon: {
            type: "icon",
            value: "sparkles",
            color: "#9ca3af",
          } satisfies TemplateIcon,
          onClick: () =>
            handleUseTemplate({
              templateId: null,
              title: "Auto",
            }),
        },
      ],
    };

    if (!hasSearch) {
      return [
        autoSection,
        {
          key: "templates",
          title: "Templates",
          showHeader: false,
          items: templateItems,
          emptyMessage: "No templates yet",
        },
      ];
    }

    return [
      autoSection,
      {
        key: "create",
        title: "Create new template",
        icon: <PlusIcon className="text-brand h-3.5 w-3.5" />,
        uppercase: false,
        items: [
          {
            key: `create-${trimmedSearch}`,
            title: trimmedSearch,
            icon: DEFAULT_TEMPLATE_ICON,
            onClick: () => handleCreateTemplate(trimmedSearch),
          },
        ],
      },
      ...(templateItems.length > 0
        ? [
            {
              key: "templates",
              title: "Templates",
              showHeader: false,
              items: templateItems,
            },
          ]
        : []),
    ];
  }, [
    handleCreateTemplate,
    handleUseTemplate,
    hasSearch,
    templateItems,
    trimmedSearch,
  ]);
  const navigableResults = useMemo(
    () => resultSections.flatMap((section) => section.items),
    [resultSections],
  );
  const focusSearchInput = useCallback(() => {
    searchInputRef.current?.focus();
  }, []);
  const focusResult = useCallback((index: number) => {
    resultRefs.current[index]?.focus();
  }, []);
  const handleSearchInputKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (navigableResults.length === 0) {
        return;
      }

      if (e.key === "ArrowDown") {
        e.preventDefault();
        focusResult(0);
      }

      if (e.key === "ArrowUp") {
        e.preventDefault();
        focusResult(navigableResults.length - 1);
      }
    },
    [focusResult, navigableResults.length],
  );
  const handleResultKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        focusResult(Math.min(index + 1, navigableResults.length - 1));
      }

      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (index === 0) {
          focusSearchInput();
          return;
        }

        focusResult(index - 1);
      }
    },
    [focusResult, focusSearchInput, navigableResults.length],
  );
  let resultIndex = 0;

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent variant="app" className="w-80" align="start">
        <div className="flex flex-col gap-1">
          <AppFloatingPanel className="flex flex-col overflow-hidden">
            <div className="border-border border-b py-1">
              <div
                className={cn([
                  "flex h-8 items-center gap-2 rounded-md px-2.5",
                ])}
              >
                <SearchIcon className="text-muted-foreground h-4 w-4" />
                <input
                  ref={searchInputRef}
                  autoFocus
                  type="text"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  onKeyDown={handleSearchInputKeyDown}
                  placeholder={t`Search templates...`}
                  className="placeholder:text-muted-foreground flex-1 bg-transparent text-sm focus:outline-hidden"
                />
                {search && (
                  <button
                    onClick={() => setSearch("")}
                    className="hover:bg-accent rounded-xs p-0.5"
                  >
                    <XIcon className="text-muted-foreground h-3 w-3" />
                  </button>
                )}
              </div>
            </div>

            <div className="relative">
              <div
                className={cn(["scroll-fade-y max-h-80 overflow-y-auto p-1.5"])}
              >
                <div className="flex flex-col gap-0">
                  {resultSections.map((section) => (
                    <TemplateSection
                      key={section.key}
                      title={section.title}
                      icon={section.icon}
                      uppercase={section.uppercase}
                      showHeader={section.showHeader}
                    >
                      {section.items.length > 0 ? (
                        section.items.map((item) => {
                          const itemIndex = resultIndex;
                          resultIndex += 1;

                          return (
                            <TemplateResultButton
                              key={item.key}
                              buttonRef={(node) => {
                                resultRefs.current[itemIndex] = node;
                              }}
                              title={item.title}
                              icon={item.icon}
                              isFavorite={item.isFavorite}
                              onClick={item.onClick}
                              onKeyDown={(e) =>
                                handleResultKeyDown(e, itemIndex)
                              }
                            />
                          );
                        })
                      ) : (
                        <div className="text-muted-foreground px-2 py-3 text-sm">
                          {section.emptyMessage}
                        </div>
                      )}
                    </TemplateSection>
                  ))}
                </div>
              </div>
            </div>
          </AppFloatingPanel>

          <button
            onClick={handleSeeAllTemplates}
            className={cn([
              "flex h-7 w-full items-center justify-center gap-1 rounded-lg px-3 text-xs font-medium",
              "text-muted-foreground hover:bg-accent hover:text-foreground transition-colors",
            ])}
          >
            {t`See all templates`}
            <ChevronRightIcon className="h-3.5 w-3.5" />
          </button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

export function Header({
  sessionId,
  editorTabs,
  currentTab,
  handleTabChange,
  isTranscribing = false,
}: {
  sessionId: string;
  editorTabs: EditorView[];
  currentTab: EditorView;
  handleTabChange: (view: EditorView) => void;
  isTranscribing?: boolean;
}) {
  const { t } = useLingui();
  const primaryEnhancedTabId = editorTabs.find(
    (view): view is Extract<EditorView, { type: "enhanced" }> =>
      view.type === "enhanced",
  )?.id;
  const shouldUseViewSwitcher = editorTabs.length > 1;

  return (
    <div data-tauri-drag-region className="flex flex-col pl-1">
      <div
        data-tauri-drag-region
        className="flex items-center justify-between gap-2"
      >
        <div data-tauri-drag-region className="relative min-w-0 flex-1">
          <div
            role="group"
            aria-label={t`Session note views`}
            data-tauri-drag-region="false"
            className={cn([
              "pointer-events-auto relative z-10 w-fit max-w-full overflow-visible",
              shouldUseViewSwitcher
                ? "bg-foreground/10 dark:bg-accent/55 flex h-[30px] items-center gap-[2px] rounded-md p-[2px]"
                : null,
            ])}
          >
            {editorTabs.map((view, index) => {
              if (view.type === "enhanced") {
                return (
                  <HeaderViewEnhanced
                    key={`enhanced-${view.id}`}
                    sessionId={sessionId}
                    enhancedNoteId={view.id}
                    canRemove={view.id !== primaryEnhancedTabId}
                    onRemove={
                      view.id !== primaryEnhancedTabId
                        ? () => {
                            const previousView = editorTabs[index - 1];
                            if (
                              currentTab.type === "enhanced" &&
                              currentTab.id === view.id &&
                              previousView
                            ) {
                              handleTabChange(previousView);
                            }

                            void deleteEnhancedNote(view.id, sessionId).catch(
                              (error) => {
                                console.error(
                                  "[session-header] failed to remove summary",
                                  error,
                                );
                              },
                            );
                          }
                        : undefined
                    }
                    onSelectNote={(enhancedNoteId) =>
                      handleTabChange({ type: "enhanced", id: enhancedNoteId })
                    }
                    isActive={
                      currentTab.type === "enhanced" &&
                      currentTab.id === view.id
                    }
                    onClick={() => handleTabChange(view)}
                  />
                );
              }

              if (view.type === "summary") {
                return (
                  <button
                    key="summary"
                    data-main-area-window-drag-region
                    data-tauri-drag-region="false"
                    type="button"
                    aria-current={
                      currentTab.type === "summary" ? "page" : undefined
                    }
                    className={iconHeaderViewClassName(
                      currentTab.type === "summary",
                      "tray",
                      "px-2.5 text-xs font-medium",
                    )}
                    onClick={() => handleTabChange(view)}
                  >
                    Summary
                  </button>
                );
              }

              if (view.type === "raw") {
                return (
                  <HeaderViewRaw
                    key={view.type}
                    sessionId={sessionId}
                    isActive={currentTab.type === view.type}
                    standalone={!shouldUseViewSwitcher}
                    onClick={() => handleTabChange(view)}
                  />
                );
              }

              if (view.type === "transcript") {
                return (
                  <HeaderViewTranscript
                    key={view.type}
                    sessionId={sessionId}
                    isActive={currentTab.type === view.type}
                    isTranscribing={isTranscribing}
                    onClick={() => handleTabChange(view)}
                  />
                );
              }

              if (view.type === "attachments") {
                return (
                  <HeaderViewAttachments
                    key={view.type}
                    isActive={currentTab.type === view.type}
                    onClick={() => handleTabChange(view)}
                  />
                );
              }

              return null;
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

export function useEditorTabs({
  audioExists = false,
  sessionId,
}: {
  audioExists?: boolean;
  sessionId: string;
}): EditorView[] {
  const canShowTranscript = useCanShowTranscript(sessionId, { audioExists });

  const enhancedNoteIds = useEnhancedNoteRecords(sessionId).map(
    (note) => note.id,
  );

  return createEditorTabs({
    enhancedNoteIds,
    canShowTranscript,
  });
}

export function createEditorTabs({
  enhancedNoteIds,
  canShowTranscript,
}: {
  enhancedNoteIds: string[];
  canShowTranscript: boolean;
}): EditorView[] {
  const enhancedTabs: EditorView[] = enhancedNoteIds.map((id) => ({
    type: "enhanced",
    id,
  }));

  return [
    ...(enhancedTabs.length ? enhancedTabs : [{ type: "summary" } as const]),
    { type: "raw" },
    ...(canShowTranscript ? [{ type: "transcript" } as const] : []),
    { type: "attachments" },
  ];
}

const useEnhanceLogic = (sessionId: string, enhancedNoteId: string) =>
  useEnhancedNoteActions({ sessionId, enhancedNoteId });

function matchesTemplateSearch(
  template: {
    title?: string;
    description?: string;
    category?: string;
    targets?: string[];
  },
  query: string,
) {
  return (
    template.title?.toLowerCase().includes(query) ||
    template.description?.toLowerCase().includes(query) ||
    template.category?.toLowerCase().includes(query) ||
    template.targets?.some((target) => target.toLowerCase().includes(query))
  );
}

function sortFavoriteTemplates<
  T extends { pinned?: boolean; pinOrder?: number; title?: string },
>(templates: T[]) {
  return [...templates]
    .filter((template) => template.pinned)
    .sort((a, b) => {
      const orderA = a.pinOrder ?? Infinity;
      const orderB = b.pinOrder ?? Infinity;
      if (orderA !== orderB) {
        return orderA - orderB;
      }
      return (a.title || "").localeCompare(b.title || "");
    });
}

function sortOtherTemplates<T extends { pinned?: boolean; title?: string }>(
  templates: T[],
) {
  return [...templates]
    .filter((template) => !template.pinned)
    .sort((a, b) => (a.title || "").localeCompare(b.title || ""));
}

function TemplateSection({
  title,
  children,
  icon,
  uppercase = true,
  showHeader = true,
}: {
  title: string;
  children: React.ReactNode;
  icon?: React.ReactNode;
  uppercase?: boolean;
  showHeader?: boolean;
}) {
  return (
    <div className="flex flex-col gap-0.5">
      {showHeader ? (
        <div className="flex items-center gap-2 px-2">
          {icon}
          <p
            className={cn([
              "text-muted-foreground font-mono text-[11px] font-medium tracking-wide",
              uppercase && "uppercase",
            ])}
          >
            {title}
          </p>
        </div>
      ) : null}
      <div className="flex flex-col gap-0">{children}</div>
    </div>
  );
}

function TemplateResultButton({
  buttonRef,
  title,
  icon,
  isFavorite = false,
  onClick,
  onKeyDown,
}: {
  buttonRef?: React.Ref<HTMLButtonElement>;
  title: string;
  icon: TemplateIcon;
  isFavorite?: boolean;
  onClick: () => void;
  onKeyDown?: (e: React.KeyboardEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button
      ref={buttonRef}
      className={cn([
        "hover:bg-accent focus:bg-muted h-8 w-full rounded-md px-2.5 text-left transition-colors focus:outline-hidden",
        "flex items-center gap-1.5",
      ])}
      onClick={onClick}
      onKeyDown={onKeyDown}
    >
      <TemplateIconGlyph icon={icon} className="size-4 text-sm" />
      <span className="text-foreground min-w-0 truncate text-sm font-medium">
        {title}
      </span>
      {isFavorite ? (
        <HeartIcon
          aria-hidden
          className="fill-brand text-brand size-3.5 shrink-0"
        />
      ) : null}
    </button>
  );
}
