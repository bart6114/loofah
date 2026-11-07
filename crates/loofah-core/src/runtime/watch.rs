use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use hypr_vault_write::StoreError;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{Notify, mpsc};

use crate::vault_watch::{ChangeSource, WatchAction, classify_event};

pub(super) struct FileWatcher(Option<RecommendedWatcher>);

impl Drop for FileWatcher {
    fn drop(&mut self) {
        let Some(watcher) = self.0.take() else {
            return;
        };
        // fseventsd's unregister RPC can stall for a minute. The closed input owns
        // no vault/store, so OS cleanup must not hold canonical flush or shutdown open.
        #[cfg(target_os = "macos")]
        if let Err(error) = std::thread::Builder::new()
            .name("loofah-watch-cleanup".into())
            .spawn(move || drop(watcher))
        {
            tracing::warn!(%error, "could not schedule native watch cleanup");
        }
        #[cfg(not(target_os = "macos"))]
        drop(watcher);
    }
}

pub(super) fn start(root: &Path) -> Result<(FileWatcher, ChangeSource), StoreError> {
    let root = root.to_path_buf();
    let watched = root.clone();
    let (tx, paths) = mpsc::channel(256);
    let overflow = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Notify::new());
    let lost = overflow.clone();
    let signal = wake.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<Event>| {
            let rescan = || {
                lost.store(true, Ordering::Release);
                signal.notify_one();
            };
            let event = match result {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(%error, "vault filesystem watch requires rescan");
                    rescan();
                    return;
                }
            };
            if event.need_rescan() || matches!(event.kind, EventKind::Any | EventKind::Other) {
                rescan();
                return;
            }
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            for path in event.paths {
                let Some(relative) = path.strip_prefix(&root).ok().and_then(Path::to_str) else {
                    continue;
                };
                if relative.is_empty() {
                    rescan();
                    continue;
                }
                if classify_event(relative, false) == WatchAction::Ignore {
                    continue;
                }
                match tx.try_send(relative.to_owned()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => rescan(),
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                }
            }
        },
        Config::default().with_follow_symlinks(false),
    )
    .map_err(|error| {
        StoreError::Io(format!("cannot initialize vault filesystem watch: {error}"))
    })?;
    watcher
        .watch(&watched, RecursiveMode::Recursive)
        .map_err(|error| StoreError::Io(format!("cannot watch vault: {error}")))?;
    Ok((
        FileWatcher(Some(watcher)),
        ChangeSource::Bounded {
            paths,
            overflow,
            wake,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::VaultRuntime;
    use hypr_loofah_protocol::{ArtifactChange, ArtifactTarget, Change};
    use hypr_vault_write::{SessionMeta, SessionStore};
    use std::time::Duration;

    #[tokio::test]
    async fn headless_runtime_watches_owned_files_refreshes_notes_and_stops_without_a_desktop() {
        let vault = tempfile::tempdir().unwrap();
        let runtime_files = tempfile::tempdir().unwrap();
        let seed = SessionStore::new(vault.path().into());
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({"id":"note","title":"Watched","created_at":"2026-11-06T19:00:00Z","tags":[]})).unwrap();
        seed.create_note_session(&meta, "Initial note")
            .await
            .unwrap();
        let attachments = vault.path().join("sessions/note/attachments");
        std::fs::create_dir(&attachments).unwrap();
        let image = attachments.join("image.svg");
        std::fs::write(&image, "<svg/>").unwrap();
        let runtime = VaultRuntime::open(
            "vault".into(),
            vault.path(),
            &runtime_files.path().join("cache"),
            &runtime_files.path().join("operations"),
        )
        .await
        .unwrap();
        let context = runtime.context.clone();
        let mut events = context.events.subscribe(None);
        std::fs::write(&image, "<svg><!--external change--></svg>").unwrap();
        let expected = ArtifactChange {
            session_id: "note".into(),
            target: ArtifactTarget::Attachment {
                attachment_id: "image.svg".into(),
            },
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Change::IndexChanged {
                    artifacts: Some(details),
                    ..
                } = events.recv().await.unwrap().change
                    && details.contains(&expected)
                {
                    break;
                }
            }
        })
        .await
        .expect("native watcher must publish exact attachment details");
        std::fs::write(vault.path().join("sessions/note/notes.md"), "External note").unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if context
                    .store
                    .session_get("note")
                    .unwrap()
                    .note_markdown
                    .as_deref()
                    == Some("External note")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("external note must refresh its in-memory projection");
        runtime.shutdown().await.unwrap();
        let mut stopped = context.store.subscribe_index_changes();
        std::fs::write(
            vault.path().join("sessions/note/notes.md"),
            "After shutdown",
        )
        .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            stopped.try_recv().is_err(),
            "stopped reconciliation must not ingest late native events"
        );
        assert!(start(&vault.path().join("missing")).is_err());
        assert_eq!(
            std::fs::read_to_string(image).unwrap(),
            "<svg><!--external change--></svg>"
        );
    }
}
