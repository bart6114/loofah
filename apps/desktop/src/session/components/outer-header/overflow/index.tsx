import { Trans } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import {
  AudioLinesIcon,
  FileDownIcon,
  FileTextIcon,
  MoreHorizontalIcon,
  PictureInPicture2Icon,
  RefreshCwIcon,
  SquareArrowOutUpRightIcon,
} from "lucide-react";
import { useState, useCallback } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import {
  AppFloatingPanel,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@hypr/ui/components/ui/dropdown-menu";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { DeleteNote } from "./delete";
import { ExportModal } from "./export-modal";
import { Listening } from "./listening";
import { ShowInFinder } from "./misc";

import { useAudioPlayer } from "~/audio-player";
import {
  useLegacyDesktopAccess,
  useLegacySessionActivity,
} from "~/main/use-legacy-activity";
import { openFloatingMeetingPanel } from "~/meeting-float/host";
import { useRegenerateTranscript } from "~/session/components/note-input/transcript/actions";
import { useHasTranscript } from "~/session/components/shared";
import { useSessionDeletionAccess } from "~/session/hooks/useDeleteSession";
import { openStandaloneNoteWindow } from "~/session/window";
import { openScopedNoteWindow } from "~/shared/application/open-note-window";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { useConfigValue } from "~/shared/config";
import type { EditorView } from "~/store/zustand/tabs/schema";
import { useUploadFile } from "~/stt/useUploadFile";

export function OverflowButton(props: {
  allowListening?: boolean;
  standaloneWindow?: boolean;
  sessionId: string;
  currentView: EditorView;
  onStartRecording?: () => void;
}) {
  const legacy = useLegacyDesktopAccess();
  const { isAvailable } = legacy;
  const deletion = useSessionDeletionAccess();
  if (isAvailable("legacy_desktop_session_controls"))
    return <LegacyOverflowButton {...props} />;
  if (
    !deletion.shared &&
    !legacy.scope.client.exportSession &&
    !legacy.scope.client.openNoteWindow
  )
    return null;
  return (
    <SharedOverflowButton
      {...props}
      canDelete={deletion.shared}
      canExport={!!legacy.scope.client.exportSession}
    />
  );
}

function SharedOverflowButton({
  sessionId,
  currentView,
  canDelete,
  canExport,
}: {
  sessionId: string;
  currentView: EditorView;
  canDelete: boolean;
  canExport: boolean;
}) {
  const [exportOpen, setExportOpen] = useState(false);
  const owner = useVaultReadScope();
  const openWindow = useMutation({
    mutationFn: () => openScopedNoteWindow(owner.scope, sessionId),
    onError: (error) => sonnerToast.error(error.message),
  });
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon" aria-label="Note actions">
            <MoreHorizontalIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          {owner.scope.client.openNoteWindow && (
            <DropdownMenuItem
              disabled={
                openWindow.isPending ||
                !owner.current() ||
                owner.connection.state !== "connected"
              }
              onSelect={() => openWindow.mutate()}
            >
              <SquareArrowOutUpRightIcon />
              <Trans>Open in New Window</Trans>
            </DropdownMenuItem>
          )}
          {canExport && (
            <DropdownMenuItem onSelect={() => setExportOpen(true)}>
              <FileDownIcon />
              <Trans>Export</Trans>
            </DropdownMenuItem>
          )}
          {canDelete && <DeleteNote sessionId={sessionId} />}
        </DropdownMenuContent>
      </DropdownMenu>
      <ExportModal
        sessionId={sessionId}
        currentView={currentView}
        open={exportOpen}
        onOpenChange={setExportOpen}
      />
    </>
  );
}

function LegacyOverflowButton({
  allowListening = true,
  standaloneWindow = false,
  sessionId,
  currentView,
  onStartRecording,
}: {
  allowListening?: boolean;
  standaloneWindow?: boolean;
  sessionId: string;
  currentView: EditorView;
  onStartRecording?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [isExportModalOpen, setIsExportModalOpen] = useState(false);
  const [hasOpenedExportModal, setHasOpenedExportModal] = useState(false);
  const hasTranscript = useHasTranscript(sessionId);
  const { audioExists, audioExistsResolved } = useAudioPlayer();
  const legacy = useLegacySessionActivity(sessionId);
  const { isAvailable } = legacy;
  const assertCurrent = useCallback(() => {
    if (!isAvailable()) throw new Error("Transcript controls are unavailable.");
  }, [isAvailable]);
  const { uploadAudio, uploadTranscript } = useUploadFile(
    sessionId,
    assertCurrent,
  );
  const regenerateTranscript = useRegenerateTranscript(
    sessionId,
    assertCurrent,
  );
  const sessionMode = legacy.mode;
  const floatingBarEnabled = useConfigValue("floating_bar_enabled");
  const isMeetingInProgress =
    sessionMode === "active" || sessionMode === "finalizing";
  const showListeningAction = allowListening;
  const showRetranscribeAction =
    audioExistsResolved && sessionMode === "inactive" && audioExists;
  const showUploadActions =
    audioExistsResolved &&
    !audioExists &&
    !hasTranscript &&
    !isMeetingInProgress;
  const canOpenFloatingPanel =
    allowListening && floatingBarEnabled && sessionMode === "active";
  const hasMeetingActions =
    showListeningAction ||
    showRetranscribeAction ||
    showUploadActions ||
    canOpenFloatingPanel;
  const openExportModal = () => {
    if (!isAvailable("legacy_desktop_session_controls")) return;
    setOpen(false);
    setHasOpenedExportModal(true);
    requestAnimationFrame(() => {
      if (isAvailable("legacy_desktop_session_controls"))
        setIsExportModalOpen(true);
    });
  };
  const handleUploadAudio = () => {
    if (!isAvailable("legacy_desktop_session_controls")) return;
    setOpen(false);
    uploadAudio();
  };
  const handleUploadTranscript = () => {
    if (!isAvailable("legacy_desktop_session_controls")) return;
    setOpen(false);
    uploadTranscript();
  };
  const handleRetranscribe = () => {
    if (!isAvailable("legacy_desktop_session_controls")) return;
    setOpen(false);
    void regenerateTranscript();
  };
  const handleOpenFloatingPanel = () => {
    if (!isAvailable("legacy_desktop_session_controls")) return;
    setOpen(false);
    void openFloatingMeetingPanel({
      sessionId,
      enabled: floatingBarEnabled,
    });
  };
  const handleOpenStandaloneWindow = () => {
    if (!isAvailable("legacy_desktop_session_controls")) return;
    setOpen(false);
    void openStandaloneNoteWindow(sessionId);
  };

  return (
    <>
      <DropdownMenu open={open} onOpenChange={setOpen}>
        <DropdownMenuTrigger asChild>
          <Button
            size="icon"
            variant="ghost"
            data-tauri-drag-region="false"
            className="text-muted-foreground hover:bg-accent hover:text-foreground rounded-full"
          >
            <MoreHorizontalIcon size={16} />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent variant="app" align="end" className="w-56">
          <AppFloatingPanel className="overflow-hidden p-1">
            <DropdownMenuItem
              onClick={openExportModal}
              className="cursor-pointer"
            >
              <FileDownIcon />
              <span>
                <Trans>Export</Trans>
              </span>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            {showListeningAction && (
              <Listening
                sessionId={sessionId}
                resume={audioExists || hasTranscript}
                onStartRecording={onStartRecording}
              />
            )}
            {showRetranscribeAction && (
              <DropdownMenuItem
                onClick={handleRetranscribe}
                className="cursor-pointer"
              >
                <RefreshCwIcon />
                <span>Re-transcribe</span>
              </DropdownMenuItem>
            )}
            {showUploadActions && (
              <>
                <DropdownMenuItem
                  onClick={handleUploadAudio}
                  className="cursor-pointer"
                >
                  <AudioLinesIcon />
                  <span>
                    <Trans>Upload audio</Trans>
                  </span>
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={handleUploadTranscript}
                  className="cursor-pointer"
                >
                  <FileTextIcon />
                  <span>
                    <Trans>Upload transcript</Trans>
                  </span>
                </DropdownMenuItem>
              </>
            )}
            {canOpenFloatingPanel && (
              <DropdownMenuItem
                onClick={handleOpenFloatingPanel}
                className="cursor-pointer"
              >
                <PictureInPicture2Icon />
                <span>
                  <Trans>Open floating panel</Trans>
                </span>
              </DropdownMenuItem>
            )}
            {hasMeetingActions && <DropdownMenuSeparator />}
            {!standaloneWindow && (
              <DropdownMenuItem
                onClick={handleOpenStandaloneWindow}
                className="cursor-pointer"
              >
                <SquareArrowOutUpRightIcon />
                <span>
                  <Trans>Open in New Window</Trans>
                </span>
              </DropdownMenuItem>
            )}
            <ShowInFinder sessionId={sessionId} />
            <DeleteNote sessionId={sessionId} />
          </AppFloatingPanel>
        </DropdownMenuContent>
      </DropdownMenu>
      {hasOpenedExportModal && (
        <ExportModal
          sessionId={sessionId}
          currentView={currentView}
          open={isExportModalOpen}
          onOpenChange={setIsExportModalOpen}
        />
      )}
    </>
  );
}
