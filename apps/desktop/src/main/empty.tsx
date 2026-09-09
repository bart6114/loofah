import { Trans } from "@lingui/react/macro";
import { platform } from "@tauri-apps/plugin-os";
import { useCallback } from "react";

import { Kbd } from "@hypr/ui/components/ui/kbd";
import { cn } from "@hypr/utils";

import { StandardContentWrapper } from "~/shared/main";
import { useNewNote, useNewNoteAndListen } from "~/shared/useNewNote";
import { useAudioImport } from "~/store/zustand/audio-import";
import { type Tab, useTabs } from "~/store/zustand/tabs";

export function TabContentEmpty({
  tab: _tab,
}: {
  tab: Extract<Tab, { type: "empty" }>;
}) {
  return (
    <StandardContentWrapper>
      <EmptyView />
    </StandardContentWrapper>
  );
}

function EmptyView() {
  const newNote = useNewNote({ behavior: "current" });
  const newNoteAndListen = useNewNoteAndListen({ behavior: "current" });
  const openCurrent = useTabs((state) => state.openCurrent);
  const setAudioImportDialogOpen = useAudioImport(
    (state) => state.setDialogOpen,
  );

  const openSettings = useCallback(
    () => openCurrent({ type: "settings" }),
    [openCurrent],
  );

  const openAudioImport = useCallback(
    () => setAudioImportDialogOpen(true),
    [setAudioImportDialogOpen],
  );

  return (
    <div
      data-tauri-drag-region
      className="flex h-full flex-col items-center justify-center gap-6"
    >
      <div className="flex min-w-[280px] flex-col gap-1 text-center">
        <ActionItem
          label={<Trans>New Note</Trans>}
          shortcut={[platform() === "windows" ? "Ctrl" : "⌘", "N"]}
          onClick={newNote}
        />
        <ActionItem
          label={<Trans>Start Recording</Trans>}
          shortcut={[
            platform() === "windows" ? "Ctrl" : "⌘",
            platform() === "windows" ? "Shift" : "⇧",
            "N",
          ]}
          onClick={newNoteAndListen}
        />
        <ActionItem
          label={<Trans>Import Audio</Trans>}
          onClick={openAudioImport}
        />
        <div className="bg-accent my-1 h-px" />
        <ActionItem
          label={<Trans>Settings</Trans>}
          shortcut={[platform() === "windows" ? "Ctrl" : "⌘", ","]}
          onClick={openSettings}
        />
      </div>
    </div>
  );
}

function ActionItem({
  label,
  shortcut,
  icon,
  onClick,
}: {
  label: React.ReactNode;
  shortcut?: string[];
  icon?: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      data-tauri-drag-region="false"
      className={cn([
        "group",
        "flex items-center justify-between gap-8",
        "text-foreground text-sm",
        "rounded-full px-4 py-2",
        "hover:bg-accent cursor-pointer transition-colors",
      ])}
    >
      <span>{label}</span>
      {shortcut && shortcut.length > 0 ? (
        <Kbd
          className={cn([
            "transition-all duration-100",
            "group-hover:-translate-y-0.5 group-hover:shadow-[0_2px_0_0_var(--kbd-shadow-outer-hover),inset_0_1px_0_0_var(--kbd-shadow-inset)]",
            "group-active:translate-y-0.5 group-active:shadow-none",
          ])}
        >
          {shortcut.join(" ")}
        </Kbd>
      ) : (
        icon
      )}
    </button>
  );
}
