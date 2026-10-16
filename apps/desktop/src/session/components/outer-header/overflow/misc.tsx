import { Icon } from "@iconify-icon/react";
import { useMutation } from "@tanstack/react-query";
import { platform } from "@tauri-apps/plugin-os";
import { Loader2Icon } from "lucide-react";

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";
import { commands as openerCommands } from "@hypr/plugin-opener2";
import { DropdownMenuItem } from "@hypr/ui/components/ui/dropdown-menu";

export function ShowInFinder({ sessionId }: { sessionId: string }) {
  const { mutate, isPending } = useMutation({
    mutationFn: async () => {
      const result = await fsSyncCommands.sessionDir(sessionId);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      await openerCommands.openPath(result.data, null);
    },
  });

  return (
    <DropdownMenuItem
      onClick={(e) => {
        e.preventDefault();
        mutate();
      }}
      disabled={isPending}
      className="cursor-pointer"
    >
      {isPending ? (
        <Loader2Icon className="animate-spin" />
      ) : (
        <Icon
          icon={
            platform() === "windows" ? "ri:folder-open-line" : "ri:finder-line"
          }
        />
      )}
      <span>
        {isPending
          ? "Opening..."
          : platform() === "windows"
            ? "Show in File Explorer"
            : "Show in Finder"}
      </span>
    </DropdownMenuItem>
  );
}
