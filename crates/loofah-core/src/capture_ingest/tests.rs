use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_loofah_protocol::{CaptureFormat, CaptureIdentity, CaptureTarget};

fn context(root: &std::path::Path) -> Arc<VaultContext> {
    let file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("lease"))
        .unwrap();
    file.try_lock().unwrap();
    let mut context = VaultContext::new(
        "vault".into(),
        Arc::new(hypr_vault_write::SessionStore::new(root.into())),
        StartupState::new(Some(root)),
        Arc::new(ConfigState::load_or_default(root)),
        Arc::new(SearchService::default()),
    )
    .with_operation_storage(root.join("operations"));
    context.lease = Some(file);
    context
        .enable_capture_ingestion(
            "https://capture.example/".into(),
            CaptureSpoolLimits::default(),
        )
        .unwrap();
    Arc::new(context)
}
fn input() -> CaptureIngestRequest {
    CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: CaptureScope {
                server: "https://capture.example/".into(),
                member_id: "member".into(),
                vault_id: "vault".into(),
            },
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            },
        },
        format: CaptureFormat::stereo(),
        first_admitted_audio_observed_at_ms: 1234,
    }
}

#[tokio::test]
async fn capture_admission_owns_the_real_vault_lease_after_its_caller_is_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = context(&root);
    let weak = Arc::downgrade(&context);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let declaration = input();
    let caller = tokio::spawn({
        let context = context.clone();
        let entered = entered.clone();
        let release = release.clone();
        let input = declaration.clone();
        async move {
            context
                .begin_capture_authorized("member".into(), input, move || async move {
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(15), entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(context);
    let contender = std::fs::File::options()
        .read(true)
        .write(true)
        .open(root.join("lease"))
        .unwrap();
    assert!(contender.try_lock().is_err());
    assert!(weak.upgrade().is_some());
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while weak.upgrade().is_some() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    contender.try_lock().unwrap();
    drop(contender);
    let spool = CaptureSpool::open(
        root.join("operations/captures"),
        CaptureSpoolLimits::default(),
    )
    .await
    .unwrap();
    let snapshot = spool
        .inventory(declaration.identity.scope)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        snapshot.identity.capture_id,
        declaration.identity.capture_id
    );
    assert_eq!(snapshot.first_admitted_audio_observed_at_ms, Some(1234));
    assert!(snapshot.publication.is_none());
    assert_eq!(snapshot.chunk_count, 0);
}

#[tokio::test]
async fn authorization_is_rechecked_after_storage_initialization_and_denial_admits_no_capture() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let context = context(&root);
    let declaration = input();
    let cause = context
        .begin_capture_authorized("member".into(), declaration.clone(), || async {
            Err::<(), _>(error(ErrorCode::Unauthenticated, "Expired token"))
        })
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::Unauthenticated);
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    let settings = context.capture_ingestion.get().unwrap();
    let spool = settings.spool.get().unwrap();
    assert!(
        spool
            .inventory(declaration.identity.scope.clone())
            .await
            .unwrap()
            .is_empty()
    );
    let mut wrong = declaration.clone();
    wrong.identity.scope.member_id = "other".into();
    let cause = context
        .begin_capture_authorized("member".into(), wrong, || async { Ok(()) })
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::Validation);
    assert!(
        spool
            .inventory(declaration.identity.scope.clone())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        context
            .begin_capture_authorized("member".into(), declaration.clone(), || async { Ok(()) })
            .await
            .unwrap()
            .identity,
        declaration.identity
    );
    let cause = context
        .capture_status_authorized("other".into(), declaration.identity.capture_id, || async {
            Ok(())
        })
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::NotFound);
}
