import { Trans, useLingui } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { getIdentifier } from "@tauri-apps/api/app";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { commands as miscCommands } from "@hypr/plugin-misc";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";

import { useAboutDialog } from "~/store/zustand/about-dialog";
import { commands, type VaultStats } from "~/types/tauri.gen";

export function AboutDialog() {
  const open = useAboutDialog((state) => state.open);
  const setOpen = useAboutDialog((state) => state.setOpen);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="max-w-sm gap-0 p-0">
        <DialogTitle className="sr-only">
          <Trans>About Loofah</Trans>
        </DialogTitle>
        {open && <AboutContent />}
      </DialogContent>
    </Dialog>
  );
}

function AboutContent() {
  const info = useQuery({
    queryKey: ["about", "device-info"],
    staleTime: Infinity,
    queryFn: async () => {
      const [result, identifier] = await Promise.all([
        miscCommands.getDeviceInfo(navigator.language),
        getIdentifier().catch(() => ""),
      ]);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return { ...result.data, identifier };
    },
  });

  const stats = useQuery({
    queryKey: ["about", "vault-stats"],
    queryFn: async () => {
      const result = await commands.vaultStats();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
  });

  const channel = info.data?.identifier.endsWith(".dev")
    ? "dev"
    : info.data?.identifier.endsWith(".staging")
      ? "staging"
      : null;

  return (
    <div className="flex flex-col">
      <div className="flex flex-col items-center gap-1 px-6 pt-8 pb-6">
        <img
          src="/assets/app-icon.png"
          alt=""
          className="mb-2 size-16 rounded-2xl shadow-sm"
          draggable={false}
        />
        <h2 className="text-base font-semibold">Loofah</h2>
        <div className="text-muted-foreground flex items-center gap-1.5 text-xs">
          {info.data ? (
            <>
              <span>
                <Trans>Version {info.data.appVersion}</Trans>
              </span>
              {channel && (
                <span className="bg-muted rounded-full px-1.5 py-px font-medium">
                  {channel}
                </span>
              )}
            </>
          ) : (
            <span>&nbsp;</span>
          )}
        </div>
        {info.data?.buildHash && (
          <CopyBuildInfo
            version={info.data.appVersion}
            sha={info.data.buildHash}
          />
        )}
      </div>

      <VaultSection stats={stats.data} loading={stats.isPending} />
    </div>
  );
}

function CopyBuildInfo({ version, sha }: { version: string; sha: string }) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  return (
    <Button
      variant="ghost"
      size="sm"
      className="text-muted-foreground h-6 gap-1.5 px-2 font-mono text-[11px]"
      onClick={() => {
        void navigator.clipboard
          .writeText(`Loofah ${version} (${sha})`)
          .then(() => setCopied(true));
      }}
    >
      {sha.slice(0, 10)}
      {copied ? (
        <CheckIcon className="size-3" />
      ) : (
        <CopyIcon className="size-3" />
      )}
    </Button>
  );
}

function VaultSection({
  stats,
  loading,
}: {
  stats: VaultStats | undefined;
  loading: boolean;
}) {
  const { i18n } = useLingui();

  if (loading) {
    return (
      <div className="border-t px-6 py-6">
        <div className="grid grid-cols-2 gap-2">
          {Array.from({ length: 4 }, (_, index) => (
            <div
              key={index}
              className="bg-muted h-16 animate-pulse rounded-lg"
            />
          ))}
        </div>
      </div>
    );
  }

  if (!stats || stats.sessions === 0) {
    return null;
  }

  const since = stats.first_session_at
    ? i18n.date(new Date(stats.first_session_at), {
        month: "long",
        year: "numeric",
      })
    : null;

  const secondary = [
    stats.enhanced_docs > 0 && (
      <Trans key="docs">{formatCount(stats.enhanced_docs)} AI summaries</Trans>
    ),
    stats.tasks_done > 0 && (
      <Trans key="tasks">{formatCount(stats.tasks_done)} tasks completed</Trans>
    ),
    stats.transcript_words > 0 && (
      <Trans key="words">
        {formatCount(stats.transcript_words)} words transcribed
      </Trans>
    ),
    stats.recording_bytes > 0 && (
      <span key="bytes">{formatBytes(stats.recording_bytes)}</span>
    ),
  ].filter(Boolean);

  return (
    <div className="flex flex-col gap-5 border-t px-6 py-6">
      <div className="flex items-baseline justify-between">
        <span className="text-muted-foreground text-[11px] font-medium tracking-wide uppercase">
          <Trans>Your vault</Trans>
        </span>
        {since && (
          <span className="text-muted-foreground text-[11px]">
            <Trans>since {since}</Trans>
          </span>
        )}
      </div>

      <div className="grid grid-cols-2 gap-2">
        <StatTile
          value={formatCount(stats.sessions)}
          label={<Trans>Notes</Trans>}
        />
        <StatTile
          value={formatCount(stats.recordings)}
          label={<Trans>Recordings</Trans>}
        />
        <StatTile value={formatCount(stats.tags)} label={<Trans>Tags</Trans>} />
        <StatTile
          value={formatDuration(stats.duration_seconds)}
          label={<Trans>Meeting recordings</Trans>}
        />
      </div>

      {secondary.length > 0 && (
        <p className="text-muted-foreground text-center text-xs">
          {secondary.map((item, index) => (
            <span key={index}>
              {index > 0 && <span className="mx-1.5 opacity-50">·</span>}
              {item}
            </span>
          ))}
        </p>
      )}

      {stats.years.length > 1 && <YearChart years={stats.years} />}
    </div>
  );
}

function StatTile({ value, label }: { value: string; label: React.ReactNode }) {
  return (
    <div className="border-border/60 flex flex-col items-center gap-0.5 rounded-lg border py-3">
      <span className="text-xl font-semibold tabular-nums">{value}</span>
      <span className="text-muted-foreground text-xs">{label}</span>
    </div>
  );
}

function YearChart({ years }: { years: VaultStats["years"] }) {
  const firstYear = years[0].year;
  const lastYear = years[years.length - 1].year;
  const byYear = new Map(years.map((year) => [year.year, year]));
  const series = Array.from(
    { length: lastYear - firstYear + 1 },
    (_, index) =>
      byYear.get(firstYear + index) ?? {
        year: firstYear + index,
        sessions: 0,
        recordings: 0,
        transcript_words: 0,
        enhanced_docs: 0,
        duration_seconds: 0,
      },
  );
  const max = Math.max(...series.map((year) => year.sessions), 1);
  const [activeYear, setActiveYear] = useState(lastYear);
  const active =
    series.find((year) => year.year === activeYear) ??
    series[series.length - 1];
  const peak = series.reduce((highest, year) =>
    year.sessions > highest.sessions ? year : highest,
  );
  const ticks = [0, 1 / 3, 2 / 3, 1]
    .map((position) => series[Math.round((series.length - 1) * position)].year)
    .filter((year, index, all) => all.indexOf(year) === index);
  const gap = series.length > 48 ? 0 : series.length > 24 ? 1 : 4;

  return (
    <div className="flex flex-col">
      <div className="flex items-baseline justify-between">
        <span className="text-muted-foreground text-[11px] font-medium tracking-wide uppercase">
          <Trans>Notes over time</Trans>
        </span>
        <span className="text-muted-foreground text-[11px] tabular-nums">
          {active.year} · {formatCount(active.sessions)} <Trans>notes</Trans>
        </span>
      </div>

      <div
        className="border-border/60 mt-3 grid h-24 items-end border-b"
        style={{
          gridTemplateColumns: `repeat(${series.length}, minmax(0, 1fr))`,
          gap,
        }}
        onMouseLeave={() => setActiveYear(lastYear)}
      >
        {series.map((year) => (
          <div
            key={year.year}
            role="img"
            tabIndex={0}
            aria-label={`${year.year}: ${formatCount(year.sessions)} notes`}
            title={`${year.year} · ${formatCount(year.sessions)} notes`}
            className="bg-primary/60 hover:bg-primary focus-visible:bg-primary w-full rounded-t-[3px] transition-none outline-none"
            style={{
              height:
                year.sessions === 0
                  ? 0
                  : `${Math.max((year.sessions / max) * 100, 2)}%`,
            }}
            onMouseEnter={() => setActiveYear(year.year)}
            onFocus={() => setActiveYear(year.year)}
          />
        ))}
      </div>

      <div className="text-muted-foreground mt-1.5 flex justify-between font-mono text-[10px]">
        {ticks.map((year) => (
          <span key={year}>{year}</span>
        ))}
      </div>

      <div className="text-muted-foreground mt-3 flex justify-between text-[11px]">
        <span>
          <Trans>
            {formatCount(
              series.reduce((total, year) => total + year.sessions, 0),
            )}
            {" notes across "}
            {series.length} years
          </Trans>
        </span>
        <span>
          <Trans>Peak: {peak.year}</Trans>
        </span>
      </div>
    </div>
  );
}

function formatCount(value: number) {
  return new Intl.NumberFormat(undefined, {
    notation: value >= 100_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(value);
}

function formatDuration(seconds: number) {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.round((seconds % 3600) / 60);
  if (hours >= 100) {
    return `${formatCount(hours)}h`;
  }
  if (hours > 0) {
    return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  }
  return `${Math.max(minutes, seconds > 0 ? 1 : 0)}m`;
}

function formatBytes(bytes: number) {
  if (bytes >= 1_000_000_000) {
    return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  }
  if (bytes >= 1_000_000) {
    return `${Math.round(bytes / 1_000_000)} MB`;
  }
  return `${Math.max(1, Math.round(bytes / 1_000))} KB`;
}
