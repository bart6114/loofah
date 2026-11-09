import { Trans, useLingui } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";

import { activeJob } from "./job-submissions";
import type { ProcessingJobSnapshot } from "./protocol.gen";
import type { VaultScope } from "./vault-scope";

export function VaultProcessingPreview({
  scope,
  job,
}: {
  scope: VaultScope;
  job: ProcessingJobSnapshot;
}) {
  const [host, setHost] = useState(scope);
  useEffect(() => setHost(scope), [scope]);
  if (
    host !== scope ||
    !scope.client.processingJobPreview ||
    !scope.client.features.includes("processing_previews")
  )
    return null;
  return (
    <Preview
      key={JSON.stringify([job.id, job.session_id, job.target])}
      scope={scope}
      job={job}
    />
  );
}

function Preview({
  scope,
  job,
}: {
  scope: VaultScope;
  job: ProcessingJobSnapshot;
}) {
  const { t } = useLingui();
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  const connected = connection.state === "connected";
  const [visible, setVisible] = useState(document.visibilityState !== "hidden");
  useEffect(() => {
    const change = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", change);
    return () => document.removeEventListener("visibilitychange", change);
  }, []);
  const result = useQuery(
    {
      ...scope.processingJobPreviewOptions(job.id),
      enabled:
        connected &&
        visible &&
        job.state !== "queued" &&
        job.state !== "succeeded",
      refetchInterval: connected && visible && activeJob(job) ? 2_000 : false,
      refetchIntervalInBackground: false,
    },
    scope.queryClient,
  );
  const value = result.data;
  const preview = value?.preview;
  if (
    !connected ||
    result.error ||
    job.state === "succeeded" ||
    value?.job.state === "succeeded" ||
    !preview?.markdown ||
    value?.job.id !== job.id ||
    value.job.session_id !== job.session_id ||
    value.job.target.kind !== job.target.kind ||
    (job.target.kind === "template_output" &&
      (value.job.target.kind !== "template_output" ||
        value.job.target.document_id !== job.target.document_id))
  )
    return null;
  return (
    <section
      className="w-full rounded bg-neutral-50 p-2"
      aria-label={t`Partial summary preview`}
    >
      <p className="text-muted-foreground mb-2">
        {activeJob(value.job) ? (
          <Trans>Generating… Partial text is not saved.</Trans>
        ) : (
          <Trans>Incomplete generation. This partial text was not saved.</Trans>
        )}
      </p>
      <pre className="max-h-48 overflow-y-auto text-xs break-words whitespace-pre-wrap">
        {preview.markdown}
      </pre>
      {preview.truncated && (
        <p className="text-muted-foreground mt-2">
          <Trans>The partial preview is shortened.</Trans>
        </p>
      )}
    </section>
  );
}
