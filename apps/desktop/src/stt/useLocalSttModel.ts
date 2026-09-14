import {
  queryOptions,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";

import {
  commands as localSttCommands,
  events as localSttEvents,
  type LocalModel,
  type DownloadStatus,
} from "@hypr/plugin-local-stt";

export const localSttKeys = {
  all: ["local-stt"] as const,
  models: () => [...localSttKeys.all, "model"] as const,
  model: (model: LocalModel) => [...localSttKeys.models(), model] as const,
  modelDownloaded: (model: LocalModel) =>
    [...localSttKeys.model(model), "downloaded"] as const,
  modelDownloading: (model: LocalModel) =>
    [...localSttKeys.model(model), "downloading"] as const,
};

export const localSttQueries = {
  server: (model: LocalModel | null) =>
    queryOptions({
      queryKey: ["local-stt-server", model],
      enabled: !!model,
      refetchInterval: 1000,
      queryFn: async () => {
        if (!model) return null;
        const result = await localSttCommands.getServerForModel(model);
        if (result.status === "error") throw new Error(result.error);
        return result.data;
      },
    }),
  supportedModels: () =>
    queryOptions({
      queryKey: [...localSttKeys.all, "supported-models"] as const,
      queryFn: () => localSttCommands.listSupportedModels(),
      staleTime: Infinity,
      select: (result) => {
        if (result.status === "error") {
          throw new Error(result.error);
        }
        return result.data;
      },
    }),
  isDownloaded: (model: LocalModel) =>
    queryOptions({
      refetchOnMount: "always",
      queryKey: localSttKeys.modelDownloaded(model),
      queryFn: () => localSttCommands.isModelDownloaded(model),
      select: (result) => {
        if (result.status === "error") {
          throw new Error(result.error);
        }
        return result.data;
      },
    }),
  isDownloading: (model: LocalModel) =>
    queryOptions({
      refetchOnMount: "always",
      queryKey: localSttKeys.modelDownloading(model),
      queryFn: () => localSttCommands.isModelDownloading(model),
      select: (result) => {
        if (result.status === "error") {
          throw new Error(result.error);
        }
        return result.data;
      },
    }),
};

export function useLocalModelDownload(
  model: LocalModel,
  onDownloadComplete?: (model: LocalModel) => void,
) {
  const queryClient = useQueryClient();
  const [progress, setProgress] = useState<number>(0);
  const [isStarting, setIsStarting] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const isDownloaded = useQuery(localSttQueries.isDownloaded(model));
  const isDownloading = useQuery(localSttQueries.isDownloading(model));
  const refetchDownloaded = isDownloaded.refetch;

  const showProgress =
    !isDownloaded.data && (isStarting || (isDownloading.data ?? false));

  useEffect(() => {
    if (isDownloading.data) {
      setIsStarting(false);
    }
  }, [isDownloading.data]);

  useEffect(() => {
    const unlisten = localSttEvents.downloadProgressPayload.listen((event) => {
      if (event.payload.model === model) {
        updateLocalModelDownloadQueries(
          queryClient,
          model,
          event.payload.status,
        );
        const { status } = event.payload;
        if (typeof status === "object" && "failed" in status) {
          setErrorMessage(status.failed);
          setIsStarting(false);
          setProgress(0);
        } else if (status === "completed") {
          setErrorMessage(null);
          setProgress(100);
        } else if (typeof status === "object" && "downloading" in status) {
          setErrorMessage(null);
          setProgress(Math.max(0, Math.min(100, status.downloading)));
        }
      }
    });

    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [model, queryClient]);

  useEffect(() => {
    if (isDownloaded.data && progress > 0) {
      setProgress(0);
      onDownloadComplete?.(model);
    }
  }, [isDownloaded.data, model, onDownloadComplete, progress]);

  const handleDownload = useCallback(() => {
    if (isDownloaded.data || isDownloading.data || isStarting) {
      return;
    }
    setErrorMessage(null);
    setIsStarting(true);
    setProgress(0);
    void localSttCommands.downloadModel(model).then((result) => {
      if (result.status === "error") {
        setErrorMessage(result.error);
        setIsStarting(false);
      }
    });
  }, [isDownloaded.data, isDownloading.data, isStarting, model]);

  const handleCancel = useCallback(() => {
    void localSttCommands.cancelDownload(model).finally(() => {
      void queryClient.invalidateQueries({
        queryKey: localSttKeys.model(model),
      });
    });
    setIsStarting(false);
    setProgress(0);
  }, [model, queryClient]);

  const handleDelete = useCallback(() => {
    void localSttCommands.deleteModel(model).then((result) => {
      if (result.status === "ok") {
        void refetchDownloaded();
      }
    });
  }, [model, refetchDownloaded]);

  return {
    progress,
    hasError: errorMessage !== null,
    errorMessage,
    isDownloaded: isDownloaded.data ?? false,
    isDownloadedLoading: isDownloaded.isLoading,
    showProgress,
    handleDownload,
    handleCancel,
    handleDelete,
  };
}

export function updateLocalModelDownloadQueries(
  client: QueryClient,
  model: LocalModel,
  status: DownloadStatus,
) {
  const downloading = typeof status === "object" && "downloading" in status;
  client.setQueryData(localSttKeys.modelDownloading(model), {
    status: "ok",
    data: downloading,
  });
  if (!downloading) {
    void client.invalidateQueries({
      queryKey: localSttKeys.modelDownloaded(model),
    });
    void client.invalidateQueries({ queryKey: ["local-stt-server", model] });
  }
}
