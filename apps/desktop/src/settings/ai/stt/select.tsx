import { Trans, useLingui } from "@lingui/react/macro";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { arch } from "@tauri-apps/plugin-os";
import {
  AlertTriangle,
  Check,
  FolderOpen,
  Info,
  Loader2,
  Trash2,
} from "lucide-react";
import { useEffect } from "react";

import {
  commands as localSttCommands,
  type LocalModel,
} from "@hypr/plugin-local-stt";
import { commands as openerCommands } from "@hypr/plugin-opener2";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@hypr/ui/components/ui/select";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@hypr/ui/components/ui/tooltip";
import { cn } from "@hypr/utils";

import { useSttSettings } from "./context";
import { DiarizationStatus } from "./diarization-status";
import { useConnectionHealth } from "./health";
import { LocalModelLabel } from "./model-icon";
import {
  getDefaultSttSelection,
  getLanguageSupportIssue,
  resolveLiveLanguageSupportMode,
} from "./selection";
import {
  displayModelTitle,
  formatModelSize,
  type ProviderId,
  PROVIDERS,
  sttModelQueries,
} from "./shared";

import { useNotifications } from "~/contexts/notifications";
import { PersistAiSelection } from "~/settings/ai/shared/persist-selection";
import {
  getConfiguredProviderIds,
  getVisibleModelSelection,
} from "~/settings/ai/shared/selection";
import { getBaseLanguageDisplayName } from "~/settings/general/language";
import { useAiProvidersState } from "~/settings/providers";
import { useSetSettingValues } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import {
  getTranscriptionLanguages,
  isConfiguredSttModel,
  isFmtrLocalSttModel,
  isLiveTranscriptionSupported,
  isRealtimeLocalModel,
  isSupportedLanguagesBatch,
  isSupportedLanguagesLive,
  isSupportedLocalSttModel,
} from "~/stt/capabilities";
import { getPreferredProviderModel } from "~/stt/model-selection";

export function SelectProviderAndModel({
  showAlerts = true,
}: {
  showAlerts?: boolean;
} = {}) {
  const { t } = useLingui();
  const { current_stt_provider, current_stt_model } = useConfigValues([
    "current_stt_provider",
    "current_stt_model",
  ] as const);
  const { providers: configuredProviders, isReady } = useConfiguredMapping();
  const { startDownload, queuedDownloads } = useSttSettings();
  const { activeDownloads } = useNotifications();
  const health = useConnectionHealth();
  const setSelection = useSetSettingValues();
  const models = configuredProviders.fmtr.models;
  const selectedSttModel = isConfiguredSttModel(
    current_stt_provider,
    current_stt_model,
  )
    ? current_stt_model
    : undefined;
  const visibleSelection = getVisibleModelSelection(
    current_stt_provider,
    selectedSttModel,
    current_stt_provider === "fmtr",
  );
  const defaultSelection =
    isReady && !visibleSelection.model
      ? getDefaultSttSelection(
          getConfiguredProviderIds(
            PROVIDERS,
            configuredProviders,
            current_stt_provider,
          ),
          configuredProviders,
          current_stt_provider,
          current_stt_model,
        )
      : null;
  const effectiveSelection = defaultSelection ?? visibleSelection;
  const displayedModel = getPreferredProviderModel(
    effectiveSelection.model,
    models,
    {
      keepUnavailableSavedModel: true,
    },
  );
  const selectedModel = models.find((model) => model.id === displayedModel);
  const primaryModel = selectedModel ?? models[0];
  const downloadInfo = activeDownloads.find(
    (download) => download.model === primaryModel?.id,
  );
  const downloadingQuery = useQuery({
    ...sttModelQueries.isDownloading(primaryModel?.id as LocalModel),
    enabled: !!primaryModel && !primaryModel.isDownloaded,
  });
  const isDownloading =
    !!primaryModel &&
    !primaryModel.isDownloaded &&
    (!!downloadInfo ||
      queuedDownloads.includes(primaryModel.id as LocalModel) ||
      downloadingQuery.data === true);
  const hasError = !!selectedModel?.isDownloaded && health.status === "error";
  const handleModelChange = (model: string) => {
    setSelection({ current_stt_provider: "fmtr", current_stt_model: model });
  };
  const downloadModel = (model: string) => {
    handleModelChange(model);
    startDownload(model as LocalModel);
  };

  return (
    <div className="flex flex-col gap-4">
      {defaultSelection ? (
        <PersistAiSelection
          key={`stt:${defaultSelection.provider}:${defaultSelection.model}`}
          type="stt"
          provider={defaultSelection.provider}
          model={defaultSelection.model}
        />
      ) : null}
      {showAlerts && !hasError && selectedModel?.isDownloaded && (
        <TranscriptionLanguageWarningToast />
      )}

      <div className="border-border flex flex-col gap-4 rounded-2xl border p-4">
        <div>
          <h3 className="text-sm font-semibold">
            <Trans>Transcription runs on your Mac</Trans>
          </h3>
          <p className="text-muted-foreground mt-1 text-xs">
            <Trans>Works offline after downloading a model.</Trans>
          </p>
        </div>
        {!isReady ? (
          <p className="text-muted-foreground flex items-center gap-2 text-sm">
            <Loader2 className="size-4 animate-spin" />
            <Trans>Checking transcription models…</Trans>
          </p>
        ) : primaryModel ? (
          <>
            <ModelSelectedValue model={primaryModel} />
            {primaryModel.isDownloaded ? (
              <p role="status" className="flex items-center gap-2 text-sm">
                {hasError ? (
                  <AlertTriangle className="size-4 shrink-0" />
                ) : health.status === "pending" ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : (
                  <Check className="text-brand size-4" />
                )}
                {hasError ? (
                  health.message
                ) : health.status === "pending" ? (
                  <Trans>Starting transcription…</Trans>
                ) : health.status === "success" ? (
                  <Trans>Downloaded and ready</Trans>
                ) : (
                  <Trans>Model downloaded</Trans>
                )}
              </p>
            ) : isDownloading ? (
              <div role="status" className="flex flex-col gap-2 text-sm">
                <span className="flex items-center gap-2">
                  <Loader2 className="size-4 animate-spin" />
                  {downloadInfo ? (
                    <Trans>
                      Downloading · {Math.round(downloadInfo.progress)}%
                    </Trans>
                  ) : (
                    <Trans>Downloading transcription model…</Trans>
                  )}
                </span>
                {downloadInfo && (
                  <progress
                    aria-label={t`Transcription model download`}
                    className="accent-brand h-2 w-full"
                    value={downloadInfo.progress}
                    max={100}
                  />
                )}
                <span className="text-muted-foreground text-xs">
                  <Trans>You can keep using Loofah while it downloads.</Trans>
                </span>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => downloadModel(primaryModel.id)}
                className="bg-primary text-primary-foreground w-fit rounded-full px-4 py-2 text-sm font-medium"
              >
                <Trans>Download transcription model</Trans>
                {formatModelSize(primaryModel.sizeBytes)
                  ? ` · ${formatModelSize(primaryModel.sizeBytes)}`
                  : ""}
              </button>
            )}
          </>
        ) : (
          <p role="status" className="text-muted-foreground text-sm">
            <Trans>No transcription models are available for this Mac.</Trans>
          </p>
        )}
      </div>

      {models.length > 0 && (
        <details className="border-border rounded-2xl border p-4">
          <summary className="cursor-pointer text-sm font-medium">
            <Trans>Choose another model</Trans>
          </summary>
          <div className="mt-4">
            <p className="text-muted-foreground mb-3 text-xs">
              <Trans>
                Models marked Live support transcription while recording. Choose
                when to transcribe below.
              </Trans>
            </p>
            <Select
              value={displayedModel || ""}
              onValueChange={handleModelChange}
            >
              <SelectTrigger
                aria-label={t`Transcription model`}
                className="bg-card text-left shadow-none focus:ring-0"
              >
                <SelectValue placeholder={t`Choose a model`}>
                  {selectedModel ? (
                    <ModelSelectedValue model={selectedModel} />
                  ) : undefined}
                </SelectValue>
              </SelectTrigger>
              <SelectContent align="end">
                {models.map((model) => (
                  <ModelSelectItem
                    key={model.id}
                    model={model}
                    onDownload={() => downloadModel(model.id)}
                  />
                ))}
              </SelectContent>
            </Select>
          </div>
        </details>
      )}
      <DiarizationStatus />
    </div>
  );
}

const TRANSCRIPTION_LANGUAGE_WARNING_TOAST_ID =
  "transcription-language-warning";
const dismissedTranscriptionLanguageWarningKeys = new Set<string>();

export function TranscriptionLanguageWarningToast() {
  const { i18n, t } = useLingui();
  const warning = useTranscriptionLanguageWarning();

  if (!warning || dismissedTranscriptionLanguageWarningKeys.has(warning.key)) {
    return null;
  }

  const model = warning.model;
  const unsupportedLanguages = warning.unsupportedLanguages.map((language) =>
    getBaseLanguageDisplayName(language, i18n.locale),
  );
  const description = warning.liveOnly
    ? unsupportedLanguages.length > 0
      ? t`Live transcription isn't available for ${formatLanguageList(unsupportedLanguages)}. You'll get the transcript right after the recording ends instead.`
      : t`Live transcription isn't available for this language combination. You'll get the transcript right after the recording ends instead.`
    : unsupportedLanguages.length > 0
      ? t`${model} can't transcribe ${formatLanguageList(unsupportedLanguages)}. Try another model or change your spoken languages.`
      : t`${model} can't transcribe all selected languages together. Try another model or use fewer spoken languages.`;

  return (
    <TranscriptionLanguageWarningToastLifecycle
      key={warning.key}
      warningKey={warning.key}
      description={description}
      actionLabel={t`Got it`}
      variant={warning.liveOnly ? "info" : "warning"}
    />
  );
}

function TranscriptionLanguageWarningToastLifecycle({
  warningKey,
  description,
  actionLabel,
  variant,
}: {
  warningKey: string;
  description: string;
  actionLabel: string;
  variant: "info" | "warning";
}) {
  useEffect(() => {
    const showToast =
      variant === "info" ? sonnerToast.info : sonnerToast.warning;
    showToast(description, {
      id: TRANSCRIPTION_LANGUAGE_WARNING_TOAST_ID,
      duration: Infinity,
      icon:
        variant === "info" ? (
          <Info className="text-brand size-4 shrink-0" />
        ) : (
          <AlertTriangle className="text-brand size-4 shrink-0" />
        ),
      action: {
        label: actionLabel,
        onClick: () => {
          dismissedTranscriptionLanguageWarningKeys.add(warningKey);
          clearTranscriptionLanguageWarningToast();
        },
      },
    });

    return clearTranscriptionLanguageWarningToast;
  }, [warningKey, description, actionLabel, variant]);

  return null;
}

function clearTranscriptionLanguageWarningToast() {
  sonnerToast.dismiss(TRANSCRIPTION_LANGUAGE_WARNING_TOAST_ID);
}

function useTranscriptionLanguageWarning() {
  const {
    current_stt_provider,
    current_stt_model,
    ai_language,
    spoken_languages,
    meeting_languages,
    transcription_timing,
  } = useConfigValues([
    "current_stt_provider",
    "current_stt_model",
    "ai_language",
    "spoken_languages",
    "meeting_languages",
    "transcription_timing",
  ] as const);
  const health = useConnectionHealth();
  const languages = getTranscriptionLanguages(
    ai_language,
    spoken_languages,
    meeting_languages,
  );
  const supportedModels = useQuery(sttModelQueries.supportedModels());

  const selectedSttModel = isConfiguredSttModel(
    current_stt_provider,
    current_stt_model,
  )
    ? current_stt_model
    : undefined;
  const isConfigured = !!(current_stt_provider && selectedSttModel);
  const isOnDeviceModel = isFmtrLocalSttModel(
    current_stt_provider,
    selectedSttModel,
  );
  const useLiveOnDeviceModel =
    isOnDeviceModel && isRealtimeLocalModel(selectedSttModel);
  const hasError = isConfigured && health.status === "error";
  const liveSupport = useQuery({
    queryKey: ["stt-live-support", current_stt_provider, selectedSttModel],
    queryFn: () =>
      isLiveTranscriptionSupported(current_stt_provider, selectedSttModel),
    enabled: isConfigured,
  });
  const useLiveMode =
    transcription_timing !== "batch" &&
    resolveLiveLanguageSupportMode({
      isOnDeviceModel,
      useLiveOnDeviceModel,
      liveSupported: liveSupport.data,
    });

  const languageSupportIssue = useQuery({
    queryKey: [
      "stt-language-support",
      current_stt_provider,
      selectedSttModel,
      useLiveMode,
      languages,
    ],
    queryFn: async () => {
      const isSupportedLive = (languages: readonly string[]) =>
        isSupportedLanguagesLive(
          current_stt_provider!,
          selectedSttModel ?? null,
          languages,
        );
      const isSupportedBatch = (languages: readonly string[]) =>
        isSupportedLanguagesBatch(
          current_stt_provider!,
          selectedSttModel ?? null,
          languages,
        );

      if (!useLiveMode) {
        const issue = await getLanguageSupportIssue(
          languages,
          isSupportedBatch,
        );
        return issue && { ...issue, liveOnly: false };
      }

      const liveIssue = await getLanguageSupportIssue(
        languages,
        isSupportedLive,
      );
      if (!liveIssue) {
        return null;
      }

      // Recording demotes to the batch model when live can't cover the
      // configured languages, so a live-only gap just delays the transcript.
      const batchIssue = await getLanguageSupportIssue(
        languages,
        isSupportedBatch,
      );
      return batchIssue
        ? { ...batchIssue, liveOnly: false }
        : { ...liveIssue, liveOnly: true };
    },
    enabled:
      isConfigured && liveSupport.data !== undefined && languages.length > 0,
  });

  if (
    !isConfigured ||
    !selectedSttModel ||
    !languageSupportIssue.data ||
    hasError
  ) {
    return null;
  }

  return {
    key: [
      current_stt_provider,
      selectedSttModel,
      languageSupportIssue.data.liveOnly ? "live" : "batch",
      ...languages,
    ].join(":"),
    model:
      supportedModels.data?.find((model) => model.key === selectedSttModel)
        ?.display_name ?? selectedSttModel,
    unsupportedLanguages: languageSupportIssue.data.unsupportedLanguages,
    liveOnly: languageSupportIssue.data.liveOnly,
  };
}

function formatLanguageList(languages: string[]) {
  const visibleLanguages = languages.slice(0, 3);
  const remainingCount = languages.length - visibleLanguages.length;

  if (remainingCount > 0) {
    visibleLanguages.push(`${remainingCount} more`);
  }

  return visibleLanguages.join(", ");
}

type ModelEntry = {
  id: string;
  isDownloaded: boolean;
  displayName?: string;
  isDeprecated?: boolean;
  sizeBytes?: number | null;
  mode?: "realtime" | "batch";
};

// STT has a single, always-eligible provider ("fmtr", on-device only):
// there is no per-provider config/eligibility to compute anymore, just the
// locally discovered Soniqo models.
function useConfiguredMapping(): {
  providers: Record<
    ProviderId,
    {
      configured: boolean;
      models: ModelEntry[];
    }
  >;
  isReady: boolean;
} {
  const { isReady } = useAiProvidersState("stt");

  const targetArch = useQuery({
    queryKey: ["target-arch"],
    queryFn: () => arch(),
    staleTime: Infinity,
  });

  const isAppleSilicon = targetArch.data === "aarch64";

  const supportedModels = useQuery(sttModelQueries.supportedModels());

  const localModels = supportedModels.data ?? [];
  const selectableModels = localModels.filter(
    (m) => m.model_type === "soniqo" || m.model_type === "whispercpp",
  );

  const downloadedModels = useQueries({
    queries: [
      ...selectableModels.map((m) => sttModelQueries.isDownloaded(m.key)),
    ],
  });

  const models: ModelEntry[] = isAppleSilicon
    ? selectableModels.map((model, i) => ({
        id: model.key,
        isDownloaded: downloadedModels[i]?.data ?? false,
        displayName: model.display_name,
        sizeBytes: model.size_bytes,
        mode: isRealtimeLocalModel(String(model.key)) ? "realtime" : "batch",
      }))
    : [];

  return {
    providers: { fmtr: { configured: true, models } } as Record<
      ProviderId,
      { configured: boolean; models: ModelEntry[] }
    >,
    isReady:
      isReady &&
      !targetArch.isPending &&
      !supportedModels.isPending &&
      !downloadedModels.some((query) => query.isPending),
  };
}

function ModelSelectItem({
  model,
  onDownload,
}: {
  model: ModelEntry;
  onDownload: () => void;
}) {
  const { activeDownloads } = useNotifications();
  const { queuedDownloads } = useSttSettings();
  const downloadInfo = activeDownloads.find((d) => d.model === model.id);
  const isDownloading =
    !!downloadInfo || queuedDownloads.includes(model.id as LocalModel);

  const label = model.displayName ?? model.id;
  const title = displayModelTitle(model.id, model.displayName);
  const sizeLabel = formatModelSize(model.sizeBytes);
  const showLocalActions = model.isDownloaded && isLocalModelId(model.id);
  const isDeprecated = model.isDeprecated === true;
  const content = (
    <div className="flex min-w-0 flex-1 items-center gap-2">
      <LocalModelLabel
        model={model.id}
        label={label}
        title={title}
        className="min-w-0"
      />
      <ModelModeBadge mode={model.mode} />
      {!model.isDownloaded && sizeLabel && (
        <span className="text-muted-foreground ml-auto shrink-0 font-mono text-[11px]">
          {sizeLabel}
        </span>
      )}
    </div>
  );

  if (model.isDownloaded) {
    return (
      <div className="group/model-row relative overflow-hidden rounded-full">
        <SelectItem
          key={model.id}
          value={model.id}
          className={cn([
            "group-hover/model-row:bg-accent group-hover/model-row:text-accent-foreground",
            showLocalActions && "pr-20",
            isDeprecated && "text-muted-foreground focus:text-muted-foreground",
          ])}
        >
          {content}
        </SelectItem>
        {showLocalActions && (
          <LocalModelDropdownActions model={model.id as LocalModel} />
        )}
      </div>
    );
  }

  const handleAction = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (isDownloading) {
      return;
    }
    onDownload();
  };

  return (
    <div
      className={cn([
        "relative flex items-center justify-between",
        "rounded-full px-2 py-1.5 text-sm outline-hidden",
        "cursor-pointer select-none",
        "hover:bg-accent hover:text-accent-foreground",
        "group",
      ])}
    >
      <div className="text-muted-foreground min-w-0 flex-1">{content}</div>
      {isDownloading ? (
        <span
          className={cn([
            "rounded-full px-2 py-0.5 text-[11px] font-medium",
            "flex items-center gap-1",
            "from-muted to-accent text-muted-foreground bg-linear-to-t",
          ])}
        >
          <Loader2 className="size-3 animate-spin" />
          {downloadInfo ? (
            <span>{Math.round(downloadInfo.progress)}%</span>
          ) : (
            <Trans>Starting</Trans>
          )}
        </span>
      ) : (
        <button
          className={cn([
            "rounded-full px-2 text-[11px] font-medium",
            "transition-all duration-150",
            "from-muted to-accent text-foreground bg-linear-to-t py-0.5 shadow-xs hover:shadow-md",
          ])}
          onClick={handleAction}
        >
          <Trans>Download</Trans>
        </button>
      )}
    </div>
  );
}

function ModelSelectedValue({ model }: { model: ModelEntry }) {
  const isDeprecated = model.isDeprecated === true;

  return (
    <div className="flex max-w-full min-w-0 items-center gap-2">
      <LocalModelLabel
        model={model.id}
        label={model.displayName ?? model.id}
        title={displayModelTitle(model.id, model.displayName)}
        className={cn(["min-w-0", isDeprecated && "opacity-60"])}
        labelClassName={cn([isDeprecated && "text-muted-foreground"])}
      />
      <ModelModeBadge mode={model.mode} />
    </div>
  );
}

function ModelModeBadge({ mode }: { mode?: ModelEntry["mode"] }) {
  if (!mode) {
    return null;
  }

  const isRealtime = mode === "realtime";

  return (
    <Tooltip delayDuration={100}>
      <TooltipTrigger asChild>
        <span
          className={cn([
            "shrink-0 cursor-help rounded-md px-1.5 py-0.5 text-[11px] font-medium",
            isRealtime
              ? "bg-recording/10 text-recording"
              : "bg-muted text-muted-foreground",
          ])}
        >
          {isRealtime ? <Trans>Live</Trans> : <Trans>After recording</Trans>}
        </span>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-64 text-xs">
        {isRealtime ? (
          <Trans>Can transcribe while the meeting is happening.</Trans>
        ) : (
          <Trans>
            Runs after the recording finishes, not during the meeting.
          </Trans>
        )}
      </TooltipContent>
    </Tooltip>
  );
}

function isLocalModelId(model: string): model is LocalModel {
  return isSupportedLocalSttModel(model);
}

function LocalModelDropdownActions({ model }: { model: LocalModel }) {
  const { t } = useLingui();
  const queryClient = useQueryClient();

  const stopSelect = (event: React.SyntheticEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
  };

  const handleOpen = () => {
    const resultPromise = String(model).startsWith("soniqo-")
      ? localSttCommands.soniqoModelDir(model)
      : localSttCommands.modelsDir();

    void resultPromise.then((result) => {
      if (result.status === "ok") {
        void openerCommands.openPath(result.data, null);
      }
    });
  };

  const deleteModel = useMutation({
    mutationFn: () => localSttCommands.deleteModel(model),
    onSuccess: (result) => {
      if (result.status === "ok") {
        void queryClient.invalidateQueries({
          queryKey: sttModelQueries.isDownloaded(model).queryKey,
        });
      }
    },
  });

  const handleDelete = () => {
    if (deleteModel.isPending) {
      return;
    }
    deleteModel.mutate();
  };

  return (
    <div
      className={cn([
        "absolute top-0 right-0 bottom-0 z-10 flex items-center justify-end gap-1 rounded-r-full pl-6",
        "pointer-events-none opacity-0 transition-opacity duration-150",
        "group-hover/model-row:pointer-events-auto group-hover/model-row:opacity-100",
        "group-focus-within/model-row:pointer-events-auto group-focus-within/model-row:opacity-100",
        deleteModel.isPending && "pointer-events-auto opacity-100",
      ])}
    >
      <button
        type="button"
        aria-label={t`Show in Finder`}
        className={cn([
          "flex size-6 items-center justify-center rounded-full",
          "text-muted-foreground hover:text-foreground",
        ])}
        onPointerDown={stopSelect}
        onClick={(event) => {
          stopSelect(event);
          handleOpen();
        }}
      >
        <FolderOpen className="size-3.5" />
      </button>
      <button
        type="button"
        aria-label={t`Delete model`}
        disabled={deleteModel.isPending}
        className={cn([
          "flex size-6 items-center justify-center rounded-full",
          "text-destructive hover:text-destructive/80",
          "disabled:opacity-70",
        ])}
        onPointerDown={stopSelect}
        onClick={(event) => {
          stopSelect(event);
          handleDelete();
        }}
      >
        {deleteModel.isPending ? (
          <Loader2 className="size-3.5 animate-spin" />
        ) : (
          <Trash2 className="size-3.5" />
        )}
      </button>
    </div>
  );
}
