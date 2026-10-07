use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

type Registry = Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>;

#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptOperations(Registry);

pub(crate) struct TranscriptOperation {
    registry: Registry,
    id: String,
    lock: Arc<AsyncMutex<()>>,
    guard: Option<OwnedMutexGuard<()>>,
}

impl TranscriptOperations {
    pub async fn lock(&self, id: &str) -> TranscriptOperation {
        let lock = {
            let mut registry = self.0.lock().unwrap();
            let lock = registry.get(id).and_then(Weak::upgrade).unwrap_or_default();
            registry.insert(id.to_owned(), Arc::downgrade(&lock));
            lock
        };
        let mut operation = TranscriptOperation {
            registry: self.0.clone(),
            id: id.to_owned(),
            lock,
            guard: None,
        };
        operation.guard = Some(operation.lock.clone().lock_owned().await);
        operation
    }
}

impl Drop for TranscriptOperation {
    fn drop(&mut self) {
        self.guard.take();
        let mut registry = self.registry.lock().unwrap();
        if Arc::strong_count(&self.lock) == 1 {
            registry.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn guards_release_registry_entries_including_cancelled_waiters() {
        let operations = TranscriptOperations::default();
        for i in 0..100 {
            let id = i.to_string();
            let guard = operations.lock(&id).await;
            let pending = operations.lock(&id);
            tokio::pin!(pending);
            tokio::select! {
                biased;
                _ = &mut pending => panic!("operation was not serialized"),
                _ = tokio::task::yield_now() => {}
            }
            drop(guard);
        }
        assert!(operations.0.lock().unwrap().is_empty());
    }
}
