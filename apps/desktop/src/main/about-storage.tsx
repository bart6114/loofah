import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { InfoIcon, RefreshCwIcon } from "lucide-react";

import { commands as settingsCommands } from "@hypr/plugin-settings";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@hypr/ui/components/ui/tooltip";
import { cn } from "@hypr/utils";

import {
  commands,
  type VaultStorageCategory,
  type VaultStorageStats,
} from "~/types/tauri.gen";

export function formatStorageBytes(bytes: number) {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const unit = Math.min(Math.floor(Math.log10(bytes) / 3), units.length - 1);
  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: unit >= 3 ? 2 : 1 }).format(bytes / 1000 ** unit)} ${units[unit]}`;
}

export function formatStorageShare(bytes: number, total: number) {
  const share = total > 0 ? (bytes / total) * 100 : 0;
  if (share > 0 && share < 0.1) return "<0.1%";
  return new Intl.NumberFormat(undefined, {
    style: "percent",
    maximumFractionDigits: 1,
  }).format(share / 100);
}

function useStorageStats() {
  const queryClient = useQueryClient();
  const vault = useQuery({
    queryKey: ["vault-base-path"],
    queryFn: async () => {
      const result = await settingsCommands.vaultBase();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    networkMode: "always",
    retry: false,
  });
  const queryKey = ["about", "vault-storage", vault.data];
  const query = useQuery({
    queryKey,
    enabled: !!vault.data,
    queryFn: () => readStorage(false),
    staleTime: 60_000,
    refetchOnWindowFocus: false,
    networkMode: "always",
    retry: false,
  });
  const refresh = useMutation({
    mutationFn: () => readStorage(true),
    onSuccess: (data) => queryClient.setQueryData(queryKey, data),
    networkMode: "always",
  });
  return { query, refresh, vault };
}

async function readStorage(refresh: boolean) {
  const result = await commands.vaultStorageStats(refresh);
  if (result.status === "error") throw new Error(result.error);
  return result.data;
}

const categoryColors: Record<VaultStorageCategory, string> = {
  mp3: "bg-brand",
  wav: "bg-brand/50",
  images: "bg-[hsl(var(--chart-3))]",
  pdf: "bg-[hsl(var(--chart-4))]",
  json: "bg-[hsl(var(--chart-2))]",
  markdown: "bg-foreground/70",
  other: "bg-muted-foreground/60",
};

export function StorageSection() {
  const { t, i18n } = useLingui();
  const { query, refresh, vault } = useStorageStats();
  const data = query.data;
  const pending = refresh.isPending || query.isFetching;
  const error = query.isError || refresh.isError || vault.isError;
  const partial = !!data && (data.scan_limited || data.unreadable_entries > 0);
  const labels: Record<VaultStorageCategory, string> = {
    mp3: t`MP3 audio`,
    wav: t`WAV audio`,
    images: t`Images`,
    pdf: t`PDFs`,
    json: t`JSON & transcripts`,
    markdown: t`Markdown`,
    other: t`Other files`,
  };
  const retry = () => {
    if (vault.isError) void vault.refetch();
    else refresh.mutate();
  };
  return (
    <div className="border-t px-6 py-5 text-sm">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <div className="flex items-center gap-1">
          <h3 className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            {partial ? (
              <Trans>Measured vault size</Trans>
            ) : (
              <Trans>Vault storage</Trans>
            )}
          </h3>
          {data && <StorageMeasurementInfo data={data} />}
        </div>
        <div className="ml-auto flex items-center gap-1">
          {data && (
            <time
              className="text-muted-foreground text-xs leading-none whitespace-nowrap"
              dateTime={data.measured_at}
              title={i18n.date(new Date(data.measured_at), {
                dateStyle: "medium",
                timeStyle: "short",
              })}
              aria-live="polite"
            >
              <Trans>
                Measured{" "}
                {i18n.date(new Date(data.measured_at), {
                  timeStyle: "short",
                })}
              </Trans>
            </time>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground h-7 gap-1.5 px-2 text-sm"
            disabled={pending || vault.isPending}
            onClick={retry}
          >
            <RefreshCwIcon className="size-3" />
            {pending && data ? (
              <Trans>Updating…</Trans>
            ) : (
              <Trans>Refresh</Trans>
            )}
          </Button>
        </div>
      </div>
      {error && (
        <div role="status" className="bg-muted mt-3 rounded-md p-3 text-sm">
          {data ? (
            <Trans>
              Couldn’t update storage. Showing the previous measurement.
            </Trans>
          ) : (
            <Trans>Couldn’t calculate vault storage.</Trans>
          )}
          <Button
            variant="link"
            size="sm"
            className="h-auto px-2 py-0 text-sm"
            disabled={pending}
            onClick={retry}
          >
            <Trans>Retry</Trans>
          </Button>
        </div>
      )}
      {!data && !error && (
        <div role="status" className="mt-3 space-y-3">
          <p className="text-muted-foreground text-sm">
            <Trans>Calculating storage…</Trans>
          </p>
          <div className="bg-muted h-9 w-32 rounded" />
          <div className="bg-muted h-2.5 rounded" />
          {Array.from({ length: 7 }, (_, index) => (
            <div key={index} className="bg-muted h-10 rounded" />
          ))}
        </div>
      )}
      {data && (
        <>
          <div className="mt-1 flex flex-wrap items-baseline gap-2.5">
            <span className="text-3xl font-semibold tracking-tight tabular-nums">
              {formatStorageBytes(data.total_bytes)}
            </span>
            <span className="text-muted-foreground text-xs">
              {data.files === 1 ? (
                <Trans>1 file</Trans>
              ) : (
                <Trans>{i18n.number(data.files)} files</Trans>
              )}
            </span>
          </div>
          {partial && (
            <p role="status" className="bg-muted mt-3 rounded-md p-3 text-sm">
              {data.scan_limited ? (
                <Trans>
                  Measurement stopped before all files could be counted. Shares
                  show measured files only.
                </Trans>
              ) : (
                <Trans>
                  Some files couldn’t be measured. Shares show measured files
                  only.
                </Trans>
              )}
            </p>
          )}
          {data.files === 0 && !partial && (
            <p className="text-muted-foreground mt-2 text-sm">
              <Trans>No files in this vault yet.</Trans>
            </p>
          )}
          <div
            className="bg-muted mt-3.5 mb-3 flex h-2.5 overflow-hidden rounded"
            aria-hidden="true"
          >
            {data.categories.map((bucket) => (
              <span
                key={bucket.category}
                className={cn([
                  "h-full shrink-0",
                  categoryColors[bucket.category],
                ])}
                style={{
                  width: `${data.total_bytes ? (bucket.bytes / data.total_bytes) * 100 : 0}%`,
                }}
              />
            ))}
          </div>
          <table
            className="w-full table-fixed text-sm"
            aria-label={t`Vault size by file type`}
          >
            <thead className="text-muted-foreground text-xs">
              <tr>
                <th scope="col" className="w-[56%] pb-1 text-left font-normal">
                  <Trans>File type</Trans>
                </th>
                <th scope="col" className="w-[26%] pb-1 text-right font-normal">
                  <Trans>Size</Trans>
                </th>
                <th scope="col" className="w-[18%] pb-1 text-right font-normal">
                  <Trans>Share</Trans>
                </th>
              </tr>
            </thead>
            <tbody>
              {data.categories.map((bucket) => (
                <tr key={bucket.category} className="border-t">
                  <th scope="row" className="py-2 text-left font-normal">
                    <div className="flex items-center gap-2">
                      <span
                        className={cn([
                          "size-2 shrink-0 rounded-xs",
                          categoryColors[bucket.category],
                        ])}
                        aria-hidden="true"
                      />
                      <span>
                        {labels[bucket.category]}
                        <span className="text-muted-foreground block text-xs">
                          {bucket.files === 1 ? (
                            <Trans>1 file</Trans>
                          ) : (
                            <Trans>{i18n.number(bucket.files)} files</Trans>
                          )}
                        </span>
                      </span>
                    </div>
                  </th>
                  <td className="text-right whitespace-nowrap tabular-nums">
                    {formatStorageBytes(bucket.bytes)}
                  </td>
                  <td className="text-muted-foreground text-right whitespace-nowrap tabular-nums">
                    {formatStorageShare(bucket.bytes, data.total_bytes)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="mt-3 flex items-baseline justify-between gap-2 border-t pt-3 text-sm">
            <span className="text-muted-foreground">
              <Trans>Trash (included)</Trans>
            </span>
            <span className="tabular-nums">
              {formatStorageBytes(data.trash_bytes)}
            </span>
          </div>
        </>
      )}
    </div>
  );
}

function StorageMeasurementInfo({ data }: { data: VaultStorageStats }) {
  const { t } = useLingui();
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          aria-label={t`How storage is measured`}
          className="text-muted-foreground hover:text-foreground focus-visible:ring-ring flex size-5 items-center justify-center rounded focus-visible:ring-2 focus-visible:outline-none"
        >
          <InfoIcon className="size-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent className="max-w-64 space-y-2">
        <p>
          <Trans>
            Total file sizes, including attachments and hidden files. Space used
            on this Mac may differ for cloud files.
          </Trans>
        </p>
        {data.skipped_links > 0 && (
          <p>
            <Trans>Symbolic links and their targets are excluded.</Trans>
          </p>
        )}
      </TooltipContent>
    </Tooltip>
  );
}
