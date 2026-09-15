import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { sessionAttachmentPathsQueryKey } from "~/session/hooks/useAttachmentResolver";
import { subscribeIndexChanged } from "~/shared/index-query";

export function ArtifactInvalidationSync() {
  const queryClient = useQueryClient();

  useEffect(() => {
    const invalidate = (ids: readonly string[]) => {
      if (ids.length === 0) {
        void queryClient.invalidateQueries({
          predicate: ({ queryKey }) =>
            queryKey[0] === "audio" ||
            (queryKey[0] === "session" && queryKey[2] === "attachment-paths"),
        });
        return;
      }
      for (const sessionId of ids) {
        void queryClient.invalidateQueries({ queryKey: ["audio", sessionId] });
        void queryClient.invalidateQueries({
          queryKey: sessionAttachmentPathsQueryKey(sessionId),
        });
      }
    };
    const unsubscribeSessions = subscribeIndexChanged("sessions", invalidate);
    const unsubscribeArtifacts = subscribeIndexChanged("artifacts", invalidate);
    return () => {
      unsubscribeSessions();
      unsubscribeArtifacts();
    };
  }, [queryClient]);

  return null;
}
