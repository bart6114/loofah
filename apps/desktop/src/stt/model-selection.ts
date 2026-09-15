type ModelEntry = {
  id: string;
  isDownloaded?: boolean;
};

type PreferredProviderModelOptions = {
  allowSavedModelWithoutChoices?: boolean;
  keepUnavailableSavedModel?: boolean;
};

// STT is on-device only: there are no external/hosted provider models left to
// default to or migrate aliases for.
export function getDefaultSttModel(_provider?: string | null) {
  return undefined;
}

export function normalizeStoredSttModel(
  _provider: string | undefined,
  model: string | undefined,
) {
  return model;
}

export function getPreferredProviderModel(
  savedModel: string | undefined,
  models: ModelEntry[],
  options?: PreferredProviderModelOptions,
) {
  const equivalentModel = savedModel?.replace(
    /^(soniqo|onnx)-parakeet-/,
    "parakeet-",
  );
  const platformChoice = models.find(
    (model) =>
      model.id.replace(/^(soniqo|onnx)-parakeet-/, "parakeet-") ===
      equivalentModel,
  );
  if (
    platformChoice &&
    (options?.keepUnavailableSavedModel ||
      platformChoice.isDownloaded !== false)
  ) {
    return platformChoice.id;
  }

  const selectableModels = models.filter((model) => model.isDownloaded ?? true);

  if (
    options?.keepUnavailableSavedModel &&
    savedModel &&
    models.some((model) => model.id === savedModel)
  ) {
    return savedModel;
  }

  if (savedModel && selectableModels.some((model) => model.id === savedModel)) {
    return savedModel;
  }

  if (selectableModels.length > 0) {
    return selectableModels[0].id;
  }

  if (options?.allowSavedModelWithoutChoices) {
    return savedModel ?? "";
  }

  return "";
}
