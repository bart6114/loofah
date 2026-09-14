import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";

import type { LocalModel } from "@hypr/plugin-local-stt";

import { localSttQueries } from "./useLocalSttModel";

import { type ProviderId } from "~/settings/ai/stt/shared";
import { useConfigValues } from "~/shared/config";
import { isFmtrLocalSttModel } from "~/stt/capabilities";

// STT is on-device only: the connection is always a local server the
// `local-stt` plugin spins up for the selected model. There is no cloud or
// generic baseUrl+apiKey provider left to connect to.
export const useSTTConnection = () => {
  const { current_stt_provider, current_stt_model } = useConfigValues([
    "current_stt_provider",
    "current_stt_model",
  ] as const) as {
    current_stt_provider: ProviderId | undefined;
    current_stt_model: string | undefined;
  };

  const localModel = isFmtrLocalSttModel(
    current_stt_provider,
    current_stt_model,
  )
    ? current_stt_model
    : null;
  const isLocalModel = !!localModel;

  const downloaded = useQuery({
    ...localSttQueries.isDownloaded(localModel as LocalModel),
    enabled: !!localModel,
  });
  const server = useQuery(localSttQueries.server(localModel));
  const data = useMemo(() => {
    if (!localModel || !current_stt_provider || downloaded.data === undefined)
      return null;
    if (!downloaded.data)
      return { status: "not_downloaded" as const, connection: null };
    return {
      status: server.data?.status ?? "loading",
      connection:
        server.data?.status === "ready" && server.data.url
          ? {
              provider: current_stt_provider,
              model: localModel,
              baseUrl: server.data.url,
              apiKey: "",
            }
          : null,
    };
  }, [current_stt_provider, downloaded.data, localModel, server.data]);
  const local = { ...server, data };
  const connection = data?.connection ?? null;

  return {
    conn: connection,
    local,
    isLocalModel,
  };
};
