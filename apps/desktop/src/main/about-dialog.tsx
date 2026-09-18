import { Trans, useLingui } from "@lingui/react/macro";
import { useQuery } from "@tanstack/react-query";
import { getIdentifier } from "@tauri-apps/api/app";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useId, useState } from "react";

import { commands as miscCommands } from "@hypr/plugin-misc";
import { Button } from "@hypr/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "@hypr/ui/components/ui/dialog";
import { cn } from "@hypr/utils";

import { StorageSection } from "./about-storage";

import { useAboutDialog } from "~/store/zustand/about-dialog";
import { commands, type VaultStats } from "~/types/tauri.gen";

export function AboutDialog() {
  const open = useAboutDialog((state) => state.open);
  const setOpen = useAboutDialog((state) => state.setOpen);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="flex max-h-[calc(100dvh-48px)] w-[calc(100vw-32px)] max-w-[460px] flex-col gap-0 overflow-hidden p-0">
        <DialogTitle className="sr-only">
          <Trans>About Loofah</Trans>
        </DialogTitle>
        {open && <AboutContent />}
      </DialogContent>
    </Dialog>
  );
}

function AboutContent() {
  const { t } = useLingui();
  const [tab, setTab] = useState("overview");
  const tabId = useId();
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
    <div className="flex min-h-0 flex-col">
      <div className="flex shrink-0 flex-col items-center gap-1 px-6 pt-6 pb-4">
        <img
          src="/assets/app-icon.png"
          alt=""
          className="mb-1 size-12 rounded-xl shadow-sm"
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

      <div
        role="tablist"
        aria-label={t`About Loofah`}
        className="bg-muted mx-6 mb-4 flex shrink-0 gap-1 rounded-lg p-1"
      >
        {(["overview", "storage"] as const).map((value) => (
          <button
            key={value}
            role="tab"
            id={`${tabId}-${value}-tab`}
            aria-controls={`${tabId}-${value}-panel`}
            aria-selected={tab === value}
            tabIndex={tab === value ? 0 : -1}
            className={cn([
              "focus-visible:ring-ring flex-1 rounded-md px-3 py-1 text-xs font-medium focus-visible:ring-2 focus-visible:outline-none",
              tab === value
                ? "bg-background text-foreground shadow-sm"
                : "text-muted-foreground hover:bg-accent",
            ])}
            onClick={() => setTab(value)}
            onKeyDown={(event) => {
              if (
                !["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)
              )
                return;
              event.preventDefault();
              const next =
                event.key === "Home"
                  ? "overview"
                  : event.key === "End"
                    ? "storage"
                    : value === "overview"
                      ? "storage"
                      : "overview";
              setTab(next);
              document.getElementById(`${tabId}-${next}-tab`)?.focus();
            }}
          >
            {value === "overview" ? (
              <Trans>Overview</Trans>
            ) : (
              <Trans>Storage</Trans>
            )}
          </button>
        ))}
      </div>
      <div key={tab} className="min-h-0 overflow-y-auto">
        <div
          role="tabpanel"
          id={`${tabId}-${tab}-panel`}
          aria-labelledby={`${tabId}-${tab}-tab`}
        >
          {tab === "overview" ? (
            <VaultSection stats={stats.data} loading={stats.isPending} />
          ) : (
            <StorageSection />
          )}
        </div>
      </div>
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
