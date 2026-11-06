import { useLingui } from "@lingui/react/macro";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
  type ReactNode,
} from "react";

import {
  AttachmentResourceContext,
  type AttachmentResourceLoader,
  type AttachmentResolution,
} from "@hypr/editor/node-views";

import type { AttachmentResource } from "./attachments";
import { ApplicationRequestError, connectionClosed } from "./errors";
import type { VaultScope } from "./vault-scope";

export function VaultAttachmentProvider({
  scope,
  sessionId,
  enabled = true,
  children,
}: {
  scope: VaultScope;
  sessionId: string;
  enabled?: boolean;
  children: ReactNode;
}) {
  const Loader = useMemo<AttachmentResourceLoader>(
    () =>
      function Loader(props) {
        return (
          <LoadAttachment {...props} scope={scope} sessionId={sessionId} />
        );
      },
    [scope, sessionId],
  );
  return (
    <AttachmentResourceContext.Provider value={enabled ? Loader : null}>
      {children}
    </AttachmentResourceContext.Provider>
  );
}

function LoadAttachment({
  scope,
  sessionId,
  attachmentId,
  children,
}: {
  scope: VaultScope;
  sessionId: string;
  attachmentId: string;
  children: (resolved: AttachmentResolution | null) => ReactNode;
}) {
  const { t } = useLingui();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const allowed = ![
    "access_denied",
    "login_required",
    "closed",
    "incompatible",
  ].includes(connection.state);
  const query = useQuery(
    { ...scope.attachmentOptions(sessionId, attachmentId), enabled: allowed },
    scope.queryClient,
  );
  const retry = () => {
    void query.refetch();
  };
  return (
    <PresentAttachment
      key={(allowed && query.data?.id) || attachmentId}
      scope={scope}
      resource={allowed ? (query.data ?? null) : null}
      pending={allowed && query.isFetching}
      retry={allowed ? retry : undefined}
      feedback={
        !allowed
          ? {
              message: t`Attachment access is unavailable. Reconnect or sign in again.`,
              error: true,
            }
          : query.isError
            ? {
                message: t`Could not load attachment. Retry when connected.`,
                error: true,
              }
            : !query.data
              ? {
                  message: query.isPending
                    ? t`Loading attachment…`
                    : t`Attachment not found.`,
                  error: !query.isPending,
                }
              : undefined
      }
    >
      {children}
    </PresentAttachment>
  );
}

function PresentAttachment({
  scope,
  resource,
  pending,
  feedback,
  retry,
  children,
}: {
  scope: VaultScope;
  resource: AttachmentResource | null;
  pending: boolean;
  feedback?: AttachmentResolution["feedback"];
  retry?: () => void;
  children: (resolved: AttachmentResolution) => ReactNode;
}) {
  const { t, i18n } = useLingui();
  const controller = useRef<AbortController | null>(null);
  const admitted = useRef(false);
  useEffect(() => {
    const active = new AbortController();
    controller.current = active;
    return () => {
      active.abort();
      controller.current = null;
    };
  }, []);
  const action = useMutation(
    {
      mutationFn: (kind: "open" | "reveal" | "save") =>
        scope.run(async () => {
          if (!resource || !controller.current) throw connectionClosed();
          const signal = controller.current.signal;
          signal.throwIfAborted();
          const operation = resource[kind];
          if (!operation) throw new Error("Attachment action unavailable");
          return operation({ signal });
        }),
      retry: false,
    },
    scope.queryClient,
  );
  const invoke = (kind: "open" | "reveal" | "save") => () => {
    if (admitted.current || pending) return;
    admitted.current = true;
    action.mutate(kind, {
      onSettled: () => {
        admitted.current = false;
      },
    });
  };
  const message = action.isPending
    ? { message: t`Preparing file…` }
    : action.isError
      ? {
          message:
            action.error instanceof ApplicationRequestError &&
            action.error.detail.mutation_outcome === "unknown"
              ? t`Could not confirm the file action. A copy may already be in Downloads/Loofah.`
              : t`Could not complete file action. Check your connection and try again.`,
          error: true,
        }
      : action.isSuccess
        ? {
            message:
              action.variables === "save"
                ? t`Saved original copy.`
                : action.variables === "reveal"
                  ? t`Showing file.`
                  : t`Opened file.`,
          }
        : undefined;
  return children({
    src: resource?.src,
    path: resource?.path,
    open: resource ? invoke("open") : undefined,
    reveal: resource ? invoke("reveal") : undefined,
    save: resource?.save ? invoke("save") : undefined,
    labels: {
      open: resource ? i18n._(resource.labels.open) : "",
      reveal: resource ? i18n._(resource.labels.reveal) : "",
      save: resource?.labels.save ? i18n._(resource.labels.save) : undefined,
      retry: t`Retry`,
    },
    preview: {
      loading: t`Loading preview…`,
      error: t`Could not load preview. Retry or save the original copy.`,
      unavailable: t`Preview unavailable. You can open or save the original file.`,
    },
    pending: pending || action.isPending,
    feedback: feedback ?? message,
    retry,
  });
}
