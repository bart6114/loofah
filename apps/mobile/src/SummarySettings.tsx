import { useForm, useStore } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import { useEffect, useState } from "react";

import { listModels, PROVIDERS } from "@hypr/ai-providers";
import {
  CORE_TRANSCRIPTION_LANGUAGE_CODES,
  getBaseLanguageDisplayName,
} from "@hypr/utils";

import { errorMessage, type Snapshot, useCommand } from "./api";

function isLoopbackUrl(value: string) {
  try {
    return ["localhost", "127.0.0.1", "[::1]"].includes(
      new URL(value).hostname,
    );
  } catch {
    return false;
  }
}

export function SummarySettings({
  snapshot,
  onDirty,
}: {
  snapshot: Snapshot;
  onDirty?: (dirty: boolean) => void;
}) {
  const save = useCommand("mobile_save_settings");
  const [defaults, setDefaults] = useState({
    summaryProvider: snapshot.settings.summary_provider,
    summaryModel: snapshot.settings.summary_model,
    summaryBaseUrl: snapshot.settings.summary_base_url,
    summaryLanguage: snapshot.settings.summary_language,
    apiKey: "",
  });
  const form = useForm({
    defaultValues: defaults,
    onSubmit: async ({ value }) => {
      await save.mutateAsync({
        ...value,
        summaryModel: value.summaryModel.trim(),
        summaryBaseUrl: value.summaryBaseUrl.trim(),
        apiKey: value.apiKey.trim() || null,
      });
      const saved = { ...value, apiKey: "" };
      setDefaults(saved);
      form.reset(saved);
    },
  });
  const isDirty = useStore(form.store, (state) => state.isDirty);
  const providerId = useStore(
    form.store,
    (state) => state.values.summaryProvider,
  );
  const baseUrl = useStore(form.store, (state) => state.values.summaryBaseUrl);
  const provider = PROVIDERS.find((candidate) => candidate.id === providerId);
  const supported = providerId === "none" || !!provider?.mobileSupported;
  const localServer = providerId === "lmstudio" || providerId === "ollama";
  const hasApiKey =
    snapshot.settings.providers[providerId]?.has_api_key ||
    (snapshot.settings.summary_provider === providerId &&
      snapshot.settings.has_api_key);
  const models = useMutation({
    mutationFn: async () => {
      const values = form.state.values;
      const selected = PROVIDERS.find(
        (candidate) => candidate.id === values.summaryProvider,
      );
      if (!selected?.mobileSupported)
        throw new Error("Choose a supported provider.");
      let apiKey = values.apiKey.trim();
      try {
        if (!apiKey && hasApiKey) {
          apiKey =
            (await invoke<string | null>("mobile_provider_api_key", {
              providerId: values.summaryProvider,
            })) ?? "";
        }
        return await listModels({
          providerId: selected.id,
          baseUrl: values.summaryBaseUrl.trim(),
          apiKey,
          fetch: tauriFetch,
        });
      } catch (failure) {
        let message = errorMessage(failure);
        if (apiKey) {
          for (const secret of [apiKey, encodeURIComponent(apiKey)]) {
            message = message.split(secret).join("[redacted]");
          }
        }
        throw new Error(message.slice(0, 1500));
      }
    },
  });
  useEffect(() => {
    onDirty?.(isDirty);
    return () => onDirty?.(false);
  }, [isDirty, onDirty]);
  useEffect(() => {
    if (!isDirty) {
      const synced = {
        summaryProvider: snapshot.settings.summary_provider,
        summaryModel: snapshot.settings.summary_model,
        summaryBaseUrl: snapshot.settings.summary_base_url,
        summaryLanguage: snapshot.settings.summary_language,
        apiKey: "",
      };
      setDefaults(synced);
      form.reset(synced);
      models.reset();
    }
  }, [
    snapshot.settings.summary_provider,
    snapshot.settings.summary_model,
    snapshot.settings.summary_base_url,
    snapshot.settings.summary_language,
  ]);
  return (
    <section id="summary-settings" className="card">
      <h2>Summaries</h2>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit().catch(() => {});
        }}
      >
        <fieldset
          style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
          disabled={save.isPending || models.isPending}
        >
          <form.Field name="summaryProvider">
            {(field) => (
              <label>
                Provider
                <select
                  value={field.state.value}
                  onChange={(event) => {
                    const id = event.target.value;
                    field.handleChange(id);
                    const next = PROVIDERS.find(
                      (candidate) => candidate.id === id,
                    );
                    const suggestedBaseUrl =
                      id === snapshot.settings.summary_provider
                        ? snapshot.settings.summary_base_url
                        : snapshot.settings.providers[id]?.base_url ||
                          next?.baseUrl ||
                          "";
                    form.setFieldValue(
                      "summaryBaseUrl",
                      (id === "lmstudio" || id === "ollama") &&
                        id !== snapshot.settings.summary_provider &&
                        isLoopbackUrl(suggestedBaseUrl)
                        ? ""
                        : suggestedBaseUrl,
                    );
                    form.setFieldValue(
                      "summaryModel",
                      id === snapshot.settings.summary_provider
                        ? snapshot.settings.summary_model
                        : "",
                    );
                    form.setFieldValue("apiKey", "");
                    models.reset();
                  }}
                >
                  <option value="none">Off</option>
                  {PROVIDERS.filter(
                    (candidate) => candidate.mobileSupported,
                  ).map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.displayName}
                    </option>
                  ))}
                  {!supported && (
                    <option value={providerId} disabled>
                      {providerId === "chatgpt_subscription"
                        ? "ChatGPT subscription"
                        : providerId}{" "}
                      · unavailable on iPhone
                    </option>
                  )}
                </select>
              </label>
            )}
          </form.Field>
          {!supported ? (
            <p className="notice">
              {providerId === "chatgpt_subscription"
                ? "ChatGPT subscription sign-in is supported on Mac. Choose an API provider or a reachable LM Studio or Ollama server to create summaries on iPhone."
                : "This synced provider is unavailable on iPhone. Choose another provider to create summaries."}
            </p>
          ) : (
            providerId !== "none" && (
              <>
                <p className="notice">
                  By saving {provider?.displayName}, you allow notes and
                  transcripts to be sent to this provider when you create a
                  summary. Your API key is stored in the iPhone Keychain.
                </p>
                {localServer && (
                  <>
                    <p className="muted">
                      {provider?.displayName} runs on your Mac or another
                      computer. Enter a server address your iPhone can reach.
                      This connection requires that server to be running;
                      summaries do not run offline on iPhone.
                    </p>
                    {isLoopbackUrl(baseUrl) && (
                      <p className="notice">
                        This address points to your iPhone. Use the server’s
                        address on your Wi-Fi network instead.
                      </p>
                    )}
                  </>
                )}
                <form.Field name="summaryBaseUrl">
                  {(field) => (
                    <label>
                      Server URL
                      <input
                        required
                        type="url"
                        value={field.state.value}
                        placeholder={
                          localServer
                            ? "http://192.168.1.10:1234/v1"
                            : "https://your-provider.example/v1"
                        }
                        onChange={(event) => {
                          field.handleChange(event.target.value);
                          models.reset();
                        }}
                      />
                    </label>
                  )}
                </form.Field>
                <form.Field name="apiKey">
                  {(field) => (
                    <label>
                      {provider?.displayName} API key
                      {localServer ? " (optional)" : ""}
                      <input
                        type="password"
                        autoComplete="off"
                        value={field.state.value}
                        placeholder={
                          hasApiKey
                            ? "Key saved · leave blank to keep"
                            : "Enter an API key"
                        }
                        onChange={(event) => {
                          field.handleChange(event.target.value);
                          models.reset();
                        }}
                      />
                    </label>
                  )}
                </form.Field>
                <button
                  type="button"
                  disabled={
                    !baseUrl.trim() || (localServer && isLoopbackUrl(baseUrl))
                  }
                  onClick={() => models.mutate()}
                >
                  {models.isPending ? "Loading models…" : "Load models"}
                </button>
                {models.error && (
                  <p className="error" role="alert">
                    {errorMessage(models.error)} You can enter a model ID below.
                  </p>
                )}
                <form.Field name="summaryModel">
                  {(field) => (
                    <>
                      {!!models.data?.models.length && (
                        <label>
                          Available models
                          <select
                            value={
                              models.data.models.includes(field.state.value)
                                ? field.state.value
                                : ""
                            }
                            onChange={(event) =>
                              field.handleChange(event.target.value)
                            }
                          >
                            <option value="" disabled>
                              Choose a model
                            </option>
                            {models.data.models.map((model) => (
                              <option key={model} value={model}>
                                {model}
                              </option>
                            ))}
                          </select>
                        </label>
                      )}
                      <label>
                        Model ID
                        <input
                          required
                          value={field.state.value}
                          placeholder={
                            providerId === "openrouter"
                              ? "provider/model"
                              : "Model or deployment ID"
                          }
                          onChange={(event) =>
                            field.handleChange(event.target.value)
                          }
                        />
                      </label>
                    </>
                  )}
                </form.Field>
                {models.isSuccess && !models.data?.models.length && (
                  <p className="muted">
                    This endpoint did not list any models. Enter a model or
                    deployment ID.
                  </p>
                )}
              </>
            )
          )}
          <form.Field name="summaryLanguage">
            {(field) => (
              <label>
                Summary language
                <select
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                >
                  {CORE_TRANSCRIPTION_LANGUAGE_CODES.map((code) => (
                    <option key={code} value={code}>
                      {getBaseLanguageDisplayName(code)}
                    </option>
                  ))}
                  {!CORE_TRANSCRIPTION_LANGUAGE_CODES.some(
                    (code) => code === field.state.value,
                  ) && (
                    <option value={field.state.value}>
                      {getBaseLanguageDisplayName(field.state.value)} (
                      {field.state.value})
                    </option>
                  )}
                </select>
              </label>
            )}
          </form.Field>
          <button
            type="submit"
            disabled={!supported || (localServer && isLoopbackUrl(baseUrl))}
          >
            {save.isPending ? "Saving…" : "Save summary settings"}
          </button>
        </fieldset>
        {save.error && (
          <p className="error" role="alert">
            {errorMessage(save.error)}
          </p>
        )}
        {save.isSuccess && (
          <p className="status" role="status">
            Settings saved
          </p>
        )}
      </form>
    </section>
  );
}
