import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { createPortal } from "react-dom";

import { cn } from "@hypr/utils";

import { useTranscriptOverview } from "~/stt/transcript-overview";

export function useSessionPeopleNames(sessionId: string): string[] {
  const overview = useTranscriptOverview(sessionId);
  const { data: people = [] } = useQuery(
    {
      ...overview.scope.peopleOptions(),
      enabled:
        overview.readable && Boolean(overview.data?.speaker_labels.length),
    },
    overview.scope.queryClient,
  );
  const data = overview.data;
  return useMemo(() => {
    const nameById = new Map(people.map((person) => [person.id, person.name]));
    return [
      ...new Set(
        (data?.speaker_labels ?? []).map((id) => nameById.get(id) ?? id),
      ),
    ];
  }, [people, data]);
}

export function SessionPeople({
  sessionId,
  className,
}: {
  sessionId: string;
  className?: string;
}) {
  const names = useSessionPeopleNames(sessionId);
  return <PeoplePills names={names} className={className} />;
}

function PeoplePills({
  names,
  className,
}: {
  names: readonly string[];
  className?: string;
}) {
  if (names.length === 0) {
    return null;
  }

  return (
    <div className={cn(["flex flex-wrap items-center gap-1.5", className])}>
      {names.map((name) => (
        <span
          key={name}
          className="bg-accent/60 text-muted-foreground rounded-full px-2 py-0.5 text-xs font-medium"
        >
          {name}
        </span>
      ))}
    </div>
  );
}

/// Pills that render *below the in-document title* of the memo/summary
/// editors: the title is the document's first node, so the row rides a
/// ProseMirror title-trailer widget. Pass `element` to the editor and render
/// `portal` anywhere in the React tree. The editor supports a single trailer
/// element, so extra below-title content rides along via `trailing`.
export function useSessionPeopleTitleTrailer(
  sessionId: string,
  trailing?: React.ReactNode,
): {
  element: HTMLElement;
  portal: React.ReactNode;
} {
  const [element] = useState(() => {
    const node = document.createElement("div");
    node.className = "select-none";
    return node;
  });

  return {
    element,
    portal: createPortal(
      <>
        <SessionPeople sessionId={sessionId} className="mt-1 mb-3" />
        {trailing}
      </>,
      element,
    ),
  };
}
