import { Trans } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import type { RefObject } from "react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { useRegenerateTranscript } from "./actions";
import { TranscriptLoadingState, TranscriptLoadError } from "./loading";
import {
  renderedTranscriptPrefix,
  useTranscriptCacheOwner,
} from "./render-cache";
import { TranscriptViewer } from "./renderer";
import { BatchState } from "./screens/batch";
import { TranscriptEmptyState } from "./screens/empty";
import { TranscriptListeningState } from "./screens/listening";
import { useTranscriptScreen } from "./state";

import {
  legacyTranscriptControlsAvailable,
  useVaultReadScope,
} from "~/shared/application/use-vault-read-scope";
import { useListener, useListenerStore } from "~/stt/contexts";
import {
  type TranscriptRecord,
  useSessionTranscriptsQuery,
} from "~/stt/queries";
import { useUploadFile } from "~/stt/useUploadFile";

const EMPTY_TRANSCRIPTS: TranscriptRecord[] = [];

export function Transcript({
  sessionId,
  scrollRef,
}: {
  sessionId: string;
  scrollRef: RefObject<HTMLDivElement | null>;
}) {
  const owner = useTranscriptCacheOwner();
  const query = useSessionTranscriptsQuery(sessionId);
  const cached =
    owner.available &&
    Boolean(query.data?.length) &&
    query.data!.every((transcript) =>
      owner.scope.queryClient
        .getQueriesData({
          queryKey: renderedTranscriptPrefix(
            owner.scope,
            sessionId,
            transcript.id,
          ),
        })
        .some(([, data]) => data !== undefined),
    );
  return (
    <TranscriptContent
      key={sessionId}
      sessionId={sessionId}
      scrollRef={scrollRef}
      transcripts={query.data ?? EMPTY_TRANSCRIPTS}
      initiallyReady={cached}
      pending={query.isPending}
      error={query.isError}
      retry={() => {
        void query.refetch();
      }}
    />
  );
}

type TranscriptContentProps = {
  sessionId: string;
  transcripts: readonly TranscriptRecord[];
  initiallyReady?: boolean;
  pending?: boolean;
  error?: boolean;
  retry?: () => void;
  scrollRef: RefObject<HTMLDivElement | null>;
};
type TranscriptActions = {
  uploadAudio: () => void;
  uploadTranscript: () => void;
  regenerate: () => void;
  stop: () => void;
};
export function TranscriptContent(props: TranscriptContentProps) {
  const owner = useVaultReadScope();
  return owner.current() && legacyTranscriptControlsAvailable(owner.scope) ? (
    <LegacyTranscriptContent {...props} />
  ) : (
    <TranscriptPresentation {...props} />
  );
}
function LegacyTranscriptContent(props: TranscriptContentProps) {
  const { scope, current } = useVaultReadScope();
  const { sessionId } = props;
  const session = useRef(sessionId);
  session.current = sessionId;
  const active = useRef(false);
  useLayoutEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const capture = useListenerStore();
  const available = useCallback(
    () =>
      active.current &&
      current() &&
      session.current === sessionId &&
      legacyTranscriptControlsAvailable(scope),
    [scope, current, sessionId],
  );
  const assertReplacement = useCallback(() => {
    if (!available())
      throw new Error(
        "Transcription controls are unavailable for this connection",
      );
    const state = capture.getState();
    const mode = state.getSessionMode(sessionId);
    if (
      (state.live.sessionId === sessionId && state.live.loading) ||
      mode === "active" ||
      mode === "finalizing"
    )
      throw new Error(
        "Wait for this recording to finish before replacing its transcript or audio",
      );
  }, [available, capture, sessionId]);
  const { uploadAudio, uploadTranscript } = useUploadFile(
    sessionId,
    assertReplacement,
  );
  const regenerate = useRegenerateTranscript(sessionId, assertReplacement);
  const stop = useListener((state) => state.stopTranscription);
  const action = useMutation(
    {
      retry: false,
      networkMode: "always",
      mutationFn: async ({
        operation,
        allowed,
      }: {
        operation: () => unknown;
        allowed: () => boolean;
      }) => {
        if (!allowed())
          throw new Error(
            "Transcription controls are unavailable for this connection",
          );
        await operation();
        if (!allowed()) return;
      },
    },
    scope.queryClient,
  );
  const run = (operation: () => unknown) =>
    action.mutate({ operation, allowed: available });
  return (
    <>
      <TranscriptPresentation
        {...props}
        actions={{
          uploadAudio: () => run(uploadAudio),
          uploadTranscript: () => run(uploadTranscript),
          regenerate: () => run(regenerate),
          stop: () => run(() => stop(sessionId)),
        }}
      />
      {action.error && <p role="alert">{action.error.message}</p>}
    </>
  );
}
function TranscriptPresentation({
  sessionId,
  transcripts,
  initiallyReady = false,
  pending = false,
  error = false,
  retry = () => {},
  scrollRef,
  actions,
}: TranscriptContentProps & { actions?: TranscriptActions }) {
  const screen = useTranscriptScreen({ sessionId, transcripts });
  const [viewerReady, setViewerReady] = useState(initiallyReady);
  useEffect(() => {
    if (screen.kind !== "ready") {
      setViewerReady(false);
      return;
    }

    let renderFrame: number | undefined;
    // WebKit can suspend paint callbacks in an occluded window; content loading
    // must still make progress when no animation frame arrives.
    const fallback = setTimeout(() => setViewerReady(true), 100);
    const loadingFrame = requestAnimationFrame(() => {
      renderFrame = requestAnimationFrame(() => {
        clearTimeout(fallback);
        setViewerReady(true);
      });
    });

    return () => {
      clearTimeout(fallback);
      cancelAnimationFrame(loadingFrame);
      if (renderFrame !== undefined) {
        cancelAnimationFrame(renderFrame);
      }
    };
  }, [screen.kind]);
  const handleStopTranscription = useCallback(() => {
    actions?.stop();
  }, [actions]);

  return (
    <div className="relative flex h-full flex-col overflow-hidden">
      {screen.kind === "running_batch" && (
        <TranscriptEmptyState
          isBatching
          percentage={screen.percentage}
          phase={screen.phase}
          onStopTranscription={
            screen.phase === "importing" ? undefined : handleStopTranscription
          }
        />
      )}
      {screen.kind === "batch_fallback" && (
        <BatchState
          requestedLiveTranscription={screen.requestedLiveTranscription}
          error={screen.error}
        />
      )}
      {!pending && screen.kind === "listening" && (
        <TranscriptListeningState status={screen.status} />
      )}
      {error && <TranscriptLoadError retry={retry} />}
      {pending &&
        screen.kind !== "ready" &&
        screen.kind !== "running_batch" &&
        screen.kind !== "batch_fallback" && <TranscriptLoadingState />}
      {!pending && !error && screen.kind === "empty" && (
        <TranscriptEmptyState
          isBatching={false}
          hasAudio={screen.hasAudio}
          error={screen.error}
          onRetranscribe={actions?.regenerate}
          onUploadAudio={actions?.uploadAudio}
          onUploadTranscript={actions?.uploadTranscript}
        />
      )}
      {!actions && screen.kind === "empty" && (
        <p role="status">
          <Trans>
            Recording and transcription controls are unavailable for this
            connection.
          </Trans>
        </p>
      )}
      {screen.kind === "ready" && !viewerReady && <TranscriptLoadingState />}
      {screen.kind === "ready" && viewerReady && (
        <TranscriptViewer
          sessionId={sessionId}
          transcripts={screen.transcripts}
          liveSegments={screen.liveSegments}
          currentActive={screen.currentActive}
          scrollRef={scrollRef}
        />
      )}
    </div>
  );
}
