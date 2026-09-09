import { Trans, useLingui } from "@lingui/react/macro";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { platform } from "@tauri-apps/plugin-os";
import {
  AlertTriangle,
  Check,
  FolderOpen,
  Info,
  Loader2,
  Trash2,
} from "lucide-react";
import { useRef, useState } from "react";

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
import { HealthStatusIndicator, useConnectionHealth } from "./health";
import { LocalModelBackendBadge, LocalModelLabel } from "./model-icon";
import {
  getDefaultSttSelection,
  getLanguageSupportIssue,
  resolveLiveLanguageSupportMode,
} from "./selection";
import {
  displayModelLabel,
  displayModelTitle,
  formatModelSize,
  type ProviderId,
  PROVIDERS,
  sttModelQueries,
} from "./shared";

import { useNotifications } from "~/contexts/notifications";
import { ProviderIconSlot } from "~/settings/ai/shared";
import { PersistAiSelection } from "~/settings/ai/shared/persist-selection";
import {
  getConfiguredProviderIds,
  getConfiguredProviders,
  getVisibleModelSelection,
} from "~/settings/ai/shared/selection";
import { getBaseLanguageDisplayName } from "~/settings/general/language";
import { useAiProvidersState } from "~/settings/providers";
import { useSetSettingValues } from "~/settings/queries";
import { useConfigValues } from "~/shared/config";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { SettingsAlertToast } from "~/shared/ui/settings-alert";
import {
  isConfiguredSttModel,
  isFmtrLocalSttModel,
  isLiveTranscriptionSupported,
  isRealtimeLocalModel,
  isSupportedLanguagesBatch,
  isSupportedLanguagesLive,
  isSupportedLocalSttModel,
} from "~/stt/capabilities";
import {
  getDefaultSttModel,
  getPreferredProviderModel,
} from "~/stt/model-selection";

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
  const { providers: configuredProviders, isReady: providerSettingsReady } =
    useConfiguredMapping();
  const { startDownload } = useSttSettings();
  const health = useConnectionHealth();
  const [pendingProvider, setPendingProvider] = useState<ProviderId | null>(
    null,
  );

  const selectedSttModel = isConfiguredSttModel(
    current_stt_provider,
    current_stt_model,
  )
    ? current_stt_model
    : undefined;
  const selectedProvider = current_stt_provider as ProviderId | undefined;
  const selectedProviderConfigured = selectedProvider
    ? (configuredProviders[selectedProvider]?.configured ?? false)
    : false;
  const visibleSelection = getVisibleModelSelection(
    selectedProvider,
    selectedSttModel,
    selectedProviderConfigured,
  );
  const selectableProviders = PROVIDERS.filter(({ disabled }) => !disabled);
  const configuredProviderIds = getConfiguredProviderIds(
    selectableProviders,
    configuredProviders,
    selectedProvider,
  );
  const defaultSelection =
    providerSettingsReady && !visibleSelection.model
      ? getDefaultSttSelection(
          configuredProviderIds,
          configuredProviders,
          selectedProvider,
          current_stt_model,
        )
      : null;
  const effectiveSelection = pendingProvider
    ? { provider: pendingProvider, model: "" }
    : (defaultSelection ?? visibleSelection);
  const visibleProvider = effectiveSelection.provider as ProviderId | "";
  const isConfigured = !!(visibleProvider && effectiveSelection.model);
  const hasError = isConfigured && health.status === "error";
  const alertDescription = !providerSettingsReady
    ? undefined
    : !isConfigured
      ? t`Transcription model is needed to make Loofah listen to your conversations.`
      : hasError
        ? health.message
        : undefined;
  const selectedModels = visibleProvider
    ? (configuredProviders[visibleProvider]?.models ?? [])
    : [];
  const displayedSttModel = effectiveSelection.model
    ? getPreferredProviderModel(effectiveSelection.model, selectedModels, {
        keepUnavailableSavedModel: true,
      })
    : undefined;
  const selectedModel = selectedModels.find(
    (model) => model.id === displayedSttModel,
  );
  const providerOptions = getConfiguredProviders(
    selectableProviders,
    configuredProviders,
  );

  const setSelection = useSetSettingValues();
  const lastSelectedModelsRef = useRef<Record<string, string>>(
    current_stt_provider && selectedSttModel
      ? { [current_stt_provider]: selectedSttModel }
      : {},
  );
  const rememberModel = (provider?: string, model?: string) => {
    if (!provider || model === undefined) {
      return;
    }

    lastSelectedModelsRef.current[provider] = model;
  };

  const handleProviderChange = (provider: string) => {
    rememberModel(current_stt_provider, selectedSttModel);

    const providerId = provider as ProviderId;
    const nextModels = configuredProviders[providerId]?.models ?? [];
    const nextModel =
      getPreferredProviderModel(
        lastSelectedModelsRef.current[provider],
        nextModels,
      ) ||
      getDefaultSttModel(providerId) ||
      "";

    if (!nextModel) {
      setPendingProvider(providerId);
      return;
    }

    setPendingProvider(null);
    rememberModel(provider, nextModel);
    setSelection({
      current_stt_provider: provider,
      current_stt_model: nextModel,
    });
  };

  const handleModelChange = (model: string) => {
    if (!visibleProvider) {
      return;
    }

    rememberModel(visibleProvider, model);
    setPendingProvider(null);
    setSelection({
      current_stt_provider: visibleProvider,
      current_stt_model: model,
    });
  };
  return (
    <div className="flex flex-col gap-4">
      {defaultSelection && !pendingProvider ? (
        <PersistAiSelection
          key={`stt:${defaultSelection.provider}:${defaultSelection.model}`}
          type="stt"
          provider={defaultSelection.provider}
          model={defaultSelection.model}
        />
      ) : null}
      {showAlerts && (
        <SettingsAlertToast
          id="stt-settings-alert"
          description={alertDescription}
          variant={hasError ? "error" : "warning"}
        />
      )}
      {showAlerts && !alertDescription && <TranscriptionLanguageWarningToast />}

      <h3 className="text-md font-sans font-semibold">
        <Trans>Model being used</Trans>
      </h3>
      <div className="flex flex-row items-center gap-4">
        <div className="min-w-0 flex-2" data-stt-provider-selector>
          <Select value={visibleProvider} onValueChange={handleProviderChange}>
            <SelectTrigger className="bg-card shadow-none focus:ring-0">
              <SelectValue placeholder={t`Select a provider`} />
            </SelectTrigger>
            <SelectContent>
              {providerOptions.map((provider) => {
                const configured =
                  configuredProviders[provider.id]?.configured ?? false;
                return (
                  <SelectItem
                    key={provider.id}
                    value={provider.id}
                    disabled={provider.disabled}
                    className={cn([
                      "data-disabled:text-muted-foreground data-disabled:!opacity-100",
                      !configured && "text-muted-foreground",
                    ])}
                  >
                    <div className="flex flex-col gap-0.5">
                      <div className="flex items-center gap-2">
                        <ProviderIconSlot>{provider.icon}</ProviderIconSlot>
                        <span>{provider.displayName}</span>
                      </div>
                    </div>
                  </SelectItem>
                );
              })}
            </SelectContent>
          </Select>
        </div>

        <span className="text-muted-foreground">/</span>

        <div className="min-w-0 flex-3">
          <Select
            value={displayedSttModel || ""}
            onValueChange={handleModelChange}
            disabled={selectedModels.length === 0}
          >
            <SelectTrigger
              className={cn([
                "bg-card text-left shadow-none focus:ring-0",
                "[&>span]:!flex [&>span]:w-full [&>span]:min-w-0 [&>span]:items-center [&>span]:justify-start [&>span]:gap-2 [&>span]:overflow-visible [&>span]:[-webkit-line-clamp:unset]",
                isConfigured && "[&>svg:last-child]:hidden",
              ])}
            >
              <SelectValue placeholder={t`Select a model`}>
                {selectedModel ? (
                  <ModelSelectedValue model={selectedModel} />
                ) : undefined}
              </SelectValue>
              {isConfigured && <HealthStatusIndicator />}
              {isConfigured && health.status === "success" && (
                <Check className="text-brand -mr-1 h-4 w-4 shrink-0" />
              )}
            </SelectTrigger>
            <SelectContent align="end">
              {selectedModels.map((model, i) => {
                const prevCategory =
                  i > 0 ? selectedModels[i - 1].category : null;
                const showHeader =
                  model.category && model.category !== prevCategory;
                const categoryLabel = showHeader
                  ? getModelCategoryLabel(model.category)
                  : null;
                return (
                  <span key={model.id}>
                    {categoryLabel && (
                      <div className="text-muted-foreground px-2 pt-2 pb-1 text-[11px] font-medium tracking-wide uppercase">
                        {categoryLabel}
                      </div>
                    )}
                    <ModelSelectItem
                      model={model}
                      onDownload={() => startDownload(model.id as LocalModel)}
                    />
                  </span>
                );
              })}
            </SelectContent>
          </Select>
        </div>
      </div>

      <DiarizationStatus />
    </div>
  );
}

const TRANSCRIPTION_LANGUAGE_WARNING_TOAST_ID =
  "transcription-language-warning";
const dismissedTranscriptionLanguageWarningKeys = new Set<string>();

function TranscriptionLanguageWarningToast() {
  const { i18n, t } = useLingui();
  const warning = useTranscriptionLanguageWarning();

  if (!warning || dismissedTranscriptionLanguageWarningKeys.has(warning.key)) {
    return null;
  }

  const model = displayModelLabel(warning.model);
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
  useMountEffect(() => {
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
  });

  return null;
}

function clearTranscriptionLanguageWarningToast() {
  sonnerToast.dismiss(TRANSCRIPTION_LANGUAGE_WARNING_TOAST_ID);
}

function useTranscriptionLanguageWarning() {
  const { current_stt_provider, current_stt_model, spoken_languages } =
    useConfigValues([
      "current_stt_provider",
      "current_stt_model",
      "spoken_languages",
    ] as const);
  const health = useConnectionHealth();

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
  const useLiveMode = resolveLiveLanguageSupportMode({
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
      spoken_languages,
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
          spoken_languages ?? [],
          isSupportedBatch,
        );
        return issue && { ...issue, liveOnly: false };
      }

      const liveIssue = await getLanguageSupportIssue(
        spoken_languages ?? [],
        isSupportedLive,
      );
      if (!liveIssue) {
        return null;
      }

      // Recording demotes to the batch model when live can't cover the
      // configured languages, so a live-only gap just delays the transcript.
      const batchIssue = await getLanguageSupportIssue(
        spoken_languages ?? [],
        isSupportedBatch,
      );
      return batchIssue
        ? { ...batchIssue, liveOnly: false }
        : { ...liveIssue, liveOnly: true };
    },
    enabled:
      isConfigured &&
      liveSupport.data !== undefined &&
      !!spoken_languages?.length,
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
      ...(spoken_languages ?? []),
    ].join(":"),
    model: selectedSttModel,
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

type ModelCategory = "latest" | null;
type ModelEntry = {
  id: string;
  isDownloaded: boolean;
  displayName?: string;
  isDeprecated?: boolean;
  category?: ModelCategory;
  sizeBytes?: number | null;
  mode?: "realtime" | "batch";
};

function getModelCategoryLabel(category?: ModelCategory) {
  if (category === "latest") {
    return "Recommended";
  }

  return null;
}

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

  const supportedModels = useQuery({
    queryKey: ["list-supported-models"],
    queryFn: async () => {
      const result = await localSttCommands.listSupportedModels();
      return result.status === "ok" ? result.data : [];
    },
    staleTime: Infinity,
  });

  const localModels = supportedModels.data ?? [];
  const soniqoModels = localModels.filter(
    (m) => m.model_type === "soniqo" || m.model_type === "onnx",
  );

  const soniqoDownloaded = useQueries({
    queries: [...soniqoModels.map((m) => sttModelQueries.isDownloaded(m.key))],
  });

  const models: ModelEntry[] = soniqoModels.map((model, i) => ({
    id: model.key,
    isDownloaded: soniqoDownloaded[i]?.data ?? false,
    displayName: model.display_name,
    sizeBytes: model.size_bytes,
    mode: isRealtimeLocalModel(String(model.key)) ? "realtime" : "batch",
    category: "latest",
  }));

  return {
    providers: { fmtr: { configured: true, models } } as Record<
      ProviderId,
      { configured: boolean; models: ModelEntry[] }
    >,
    isReady,
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

  const label = displayModelLabel(model.id, model.displayName);
  const title = displayModelTitle(model.id, model.displayName);
  const sizeLabel = formatModelSize(model.sizeBytes);
  const showLocalActions = model.isDownloaded && isLocalModelId(model.id);
  const isDeprecated = model.isDeprecated === true;
  const content = (
    <div className="flex min-w-0 flex-1 items-center justify-between gap-3">
      <LocalModelLabel
        model={model.id}
        label={label}
        title={title}
        className="min-w-0 flex-1"
      />
      <div className="flex shrink-0 items-center gap-2 text-[11px]">
        <LocalModelBackendBadge model={model.id} />
        {model.mode !== "realtime" && <ModelModeBadge mode={model.mode} />}
        {!model.isDownloaded && sizeLabel && (
          <span className="text-muted-foreground font-mono">{sizeLabel}</span>
        )}
      </div>
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
            "opacity-0 group-hover:opacity-100",
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
        label={displayModelLabel(model.id, model.displayName)}
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
    const resultPromise =
      String(model).startsWith("soniqo-") || String(model).startsWith("onnx-")
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
        aria-label={
          platform() === "windows"
            ? t`Show in File Explorer`
            : t`Show in Finder`
        }
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
