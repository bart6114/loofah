use std::{fs::File, io::Read, path::Path};

use hypr_loofah_protocol::ProcessingJobFailure;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

const MAX_CATALOG_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Deserialize)]
struct Catalog {
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    slug: String,
    #[serde(default)]
    input_modalities: Vec<String>,
}

pub(crate) async fn supports_images<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    model: &str,
    cancel: CancellationToken,
) -> Result<bool, ProcessingJobFailure> {
    if cancel.is_cancelled() {
        return Err(ProcessingJobFailure::Interrupted);
    }
    let catalog =
        super::model_catalog_path(app).map_err(|_| ProcessingJobFailure::ProviderUnavailable)?;
    inspect(model.to_owned(), cancel, move || read_catalog(&catalog)).await
}

async fn inspect(
    model: String,
    cancel: CancellationToken,
    read: impl FnOnce() -> Result<Catalog, ProcessingJobFailure> + Send + 'static,
) -> Result<bool, ProcessingJobFailure> {
    if cancel.is_cancelled() {
        return Err(ProcessingJobFailure::Interrupted);
    }
    // Keep the original owned read through cancellation; the shared generator retains its
    // model admission and Context until this preparation has actually drained.
    let catalog = tokio::task::spawn_blocking(read)
        .await
        .map_err(|_| ProcessingJobFailure::ProviderUnavailable)?;
    if cancel.is_cancelled() {
        return Err(ProcessingJobFailure::Interrupted);
    }
    Ok(catalog?.models.iter().any(|entry| {
        entry.slug == model && entry.input_modalities.iter().any(|value| value == "image")
    }))
}

fn read_catalog(path: &Path) -> Result<Catalog, ProcessingJobFailure> {
    let unavailable = || ProcessingJobFailure::ProviderUnavailable;
    let file = File::open(path).map_err(|_| unavailable())?;
    if !file.metadata().map_err(|_| unavailable())?.is_file() {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(unavailable());
    }
    let catalog: Catalog = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if catalog.models.len() > 512 {
        return Err(unavailable());
    }
    let mut seen = std::collections::HashSet::new();
    if catalog
        .models
        .iter()
        .any(|entry| entry.slug.is_empty() || !seen.insert(&entry.slug))
    {
        return Err(unavailable());
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscription_image_input_uses_exact_bundled_modalities_not_names() {
        for (model, expected) in [
            ("text-vision-name", false),
            ("plain-name", true),
            ("missing", false),
            ("no-modalities", false),
        ] {
            let catalog: Catalog = serde_json::from_value(serde_json::json!({"models":[
                {"slug":"text-vision-name","input_modalities":["text"]},
                {"slug":"plain-name","input_modalities":["text","image"]},
                {"slug":"no-modalities"}
            ]}))
            .unwrap();
            assert_eq!(
                inspect(model.into(), CancellationToken::new(), move || Ok(catalog))
                    .await
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn catalog_loading_is_bounded_and_missing_corrupt_or_duplicate_data_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("models.json");
        assert!(read_catalog(&path).is_err());
        for bytes in [
            b"invalid".to_vec(),
            vec![b' '; MAX_CATALOG_BYTES as usize + 1],
            br#"{"models":[{"slug":"same"},{"slug":"same","input_modalities":["image"]}]}"#
                .to_vec(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(read_catalog(&path).is_err());
        }
        std::fs::write(
            &path,
            br#"{"models":[{"slug":"model","input_modalities":["image"]}]}"#,
        )
        .unwrap();
        assert_eq!(read_catalog(&path).unwrap().models[0].slug, "model");
    }

    #[tokio::test]
    async fn cancelled_catalog_discovery_skips_new_reads_and_drains_admitted_reads() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            inspect("m".into(), cancel, || panic!("must not read")).await,
            Err(ProcessingJobFailure::Interrupted)
        );
        let cancel = CancellationToken::new();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let stopped = cancel.clone();
        let mut task = tokio::spawn(inspect("m".into(), cancel, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(Catalog { models: vec![] })
        }));
        started_rx.await.unwrap();
        stopped.cancel();
        assert!(futures_util::poll!(&mut task).is_pending());
        release_tx.send(()).unwrap();
        assert_eq!(task.await.unwrap(), Err(ProcessingJobFailure::Interrupted));
    }
}
