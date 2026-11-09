import { useLingui } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { BotIcon } from "lucide-react";
import {
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { cn } from "@hypr/utils";

import { VaultHostContext } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";

const terminal = new Set([
  "closed",
  "access_denied",
  "login_required",
  "incompatible",
]);

/// Marks a note whose `_meta.json` carries an `author` (an agent or other
/// external writer) as not written by the vault owner. Owner-authored
/// sessions have no author and render nothing.
export function SessionAuthorBadge({
  sessionId,
  className,
}: {
  sessionId: string;
  className?: string;
}) {
  const scope = useContext(VaultHostContext)?.scope;
  const [host, setHost] = useState(scope);
  useEffect(() => setHost(scope), [scope]);
  return scope && host === scope ? (
    <ScopedSessionAuthorBadge
      key={sessionId}
      scope={scope}
      sessionId={sessionId}
      className={className}
    />
  ) : null;
}

function ScopedSessionAuthorBadge({
  scope,
  sessionId,
  className,
}: {
  scope: VaultScope;
  sessionId: string;
  className?: string;
}) {
  const { t } = useLingui();
  const owner = useRef({
    retired: terminal.has(scope.getSnapshot().state),
  }).current;
  const subscribe = useCallback(
    (changed: () => void) =>
      scope.subscribe(() => {
        if (terminal.has(scope.getSnapshot().state)) owner.retired = true;
        changed();
      }),
    [scope, owner],
  );
  const connection = useSyncExternalStore(subscribe, scope.getSnapshot);
  if (terminal.has(connection.state)) owner.retired = true;
  const readable =
    !owner.retired &&
    (connection.state === "connected" || connection.state === "reconnecting");
  const metadata = useQuery(
    {
      ...scope.metadataOptions(sessionId),
      enabled: Boolean(sessionId) && readable,
      select: (snapshot) =>
        snapshot.meta.id === sessionId ? (snapshot.meta.author ?? null) : null,
    },
    scope.queryClient,
  );
  const author = readable && !metadata.isError ? metadata.data : null;

  if (!author) {
    return null;
  }

  return (
    <div className={cn(["flex items-center", className])}>
      <span
        title={t`Written by ${author} — not by you`}
        className="bg-accent/60 text-muted-foreground inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium"
      >
        <BotIcon className="size-3" />
        {author}
      </span>
    </div>
  );
}
