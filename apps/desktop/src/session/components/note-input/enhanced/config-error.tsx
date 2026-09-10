import { Trans } from "@lingui/react/macro";
import { useCallback } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { cn } from "@hypr/utils";

import { useTabs } from "~/store/zustand/tabs";

export function ConfigError({
  sessionTitle,
  titleTrailerElement,
}: {
  sessionTitle: string;
  titleTrailerElement?: HTMLElement;
}) {
  const openNew = useTabs((state) => state.openNew);
  const title = sessionTitle.trim();

  // The speakers row lives in a detached node normally mounted by the note
  // editor's title-trailer widget; adopt it here since the editor never mounts.
  const mountTrailer = useCallback(
    (node: HTMLDivElement | null) => {
      if (node && titleTrailerElement) {
        node.appendChild(titleTrailerElement);
      }
    },
    [titleTrailerElement],
  );

  return (
    <div className="flex h-full flex-col">
      <div className="flex min-h-[1.875rem] items-start">
        <h1
          className={cn([
            "text-[1.5rem] leading-[1.875rem] font-semibold",
            title ? "text-foreground" : "text-muted-foreground opacity-60",
          ])}
        >
          {title || "Untitled"}
        </h1>
      </div>
      <div ref={mountTrailer} />
      <div
        role="alert"
        className="flex min-h-[400px] flex-1 flex-col items-center justify-center px-6"
      >
        <div className="mb-6 flex max-w-md flex-col gap-2 text-center">
          <p className="text-base font-medium">
            <Trans>Set up AI summaries</Trans>
          </p>
          <p className="text-muted-foreground text-sm leading-relaxed">
            <Trans>
              Connect an Intelligence provider to generate a summary from your
              notes or transcript.
            </Trans>
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            className="shadow-none"
            onClick={() =>
              openNew({ type: "settings", state: { tab: "intelligence" } })
            }
          >
            <Trans>Set up Intelligence</Trans>
          </Button>
        </div>
      </div>
    </div>
  );
}
