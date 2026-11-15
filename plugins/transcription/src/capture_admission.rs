use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub struct CaptureAdmission {
    gate: tokio::sync::Mutex<()>,
    dictation: Arc<AtomicBool>,
}

impl CaptureAdmission {
    pub async fn lock(&self) -> CaptureAdmissionGuard<'_> {
        CaptureAdmissionGuard {
            _guard: self.gate.lock().await,
            dictation: &self.dictation,
        }
    }
}

pub struct CaptureAdmissionGuard<'a> {
    _guard: tokio::sync::MutexGuard<'a, ()>,
    dictation: &'a Arc<AtomicBool>,
}

impl CaptureAdmissionGuard<'_> {
    pub fn is_dictating(&self) -> bool {
        self.dictation.load(Ordering::SeqCst)
    }

    pub fn reserve_dictation(&self) -> Option<DictationLease> {
        if self.dictation.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(DictationLease(self.dictation.clone()))
    }
}

pub struct DictationLease(Arc<AtomicBool>);

impl Drop for DictationLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn meeting_start_waits_for_dictation_admission_and_cannot_enter_until_drain() {
        let admission = Arc::new(CaptureAdmission::default());
        let checking_meeting_snapshot = admission.lock().await;
        let contender = admission.clone();
        let (entered, mut entry) = tokio::sync::oneshot::channel();
        let meeting = tokio::spawn(async move {
            let guard = contender.lock().await;
            entered.send(guard.is_dictating()).unwrap();
        });
        tokio::task::yield_now().await;
        assert!(matches!(
            entry.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        let dictation = checking_meeting_snapshot.reserve_dictation().unwrap();
        drop(checking_meeting_snapshot);
        assert!(entry.await.unwrap());
        meeting.await.unwrap();
        assert!(admission.lock().await.is_dictating());
        drop(dictation);
        assert!(!admission.lock().await.is_dictating());
    }

    #[tokio::test]
    async fn dictation_checks_meeting_snapshot_only_after_meeting_start_finishes() {
        let admission = Arc::new(CaptureAdmission::default());
        let meeting_start = admission.lock().await;
        let meeting_started = Arc::new(AtomicBool::new(false));
        let contender = admission.clone();
        let started = meeting_started.clone();
        let dictation = tokio::spawn(async move {
            let _guard = contender.lock().await;
            started.load(Ordering::SeqCst)
        });
        meeting_started.store(true, Ordering::SeqCst);
        drop(meeting_start);
        assert!(dictation.await.unwrap());
    }
}
