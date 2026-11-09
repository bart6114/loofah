use super::*;

const LOCAL: &str = "00000000-0000-0000-0000-000000000000";

async fn local_terminal(context: &Arc<VaultContext>, id: &str) -> TranscriptionJobSnapshot {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let snapshot = context
                .transcription_job(LOCAL.into(), id.into())
                .await
                .unwrap();
            if snapshot.state.is_terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn local_principal_survives_submit_result_replay_and_journal_reopen() {
    let mut fixture = Fixture::new(false).await;
    let context = fixture.context();
    assert!(
        context
            .list_transcription_jobs(LOCAL.into(), Default::default())
            .await
            .unwrap()
            .jobs
            .is_empty()
    );
    let submitted = context
        .submit_transcription_job(LOCAL.into(), REQUEST.into(), request())
        .await
        .unwrap();
    let done = local_terminal(&context, &submitted.id).await;
    assert_eq!(done.state, TranscriptionJobState::Succeeded);
    let result = context
        .transcription_job_result(LOCAL.into(), done.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.transcript.user_id, LOCAL);
    assert_eq!(result.transcript.words.len(), 1);
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 1);
    let canonical = std::fs::read(fixture.transcript()).unwrap();
    drop(context);
    fixture.runtime.take().unwrap().shutdown().await.unwrap();
    fixture.runtime = Some(
        open(
            fixture.root.path(),
            fixture.engine.clone(),
            fixture.auth.clone(),
            TranscriptionLimits::default(),
        )
        .await,
    );
    let context = fixture.context();
    assert_eq!(
        context
            .list_transcription_jobs(LOCAL.into(), Default::default())
            .await
            .unwrap()
            .jobs,
        vec![done.clone()]
    );
    assert_eq!(
        context
            .transcription_job(LOCAL.into(), done.id.clone())
            .await
            .unwrap(),
        done
    );
    assert_eq!(
        context
            .transcription_job_result(LOCAL.into(), done.id.clone())
            .await
            .unwrap(),
        Some(result)
    );
    assert_eq!(
        context
            .submit_transcription_job(LOCAL.into(), REQUEST.into(), request())
            .await
            .unwrap(),
        done
    );
    assert!(
        context
            .list_transcription_jobs(MEMBER.into(), Default::default())
            .await
            .unwrap()
            .jobs
            .is_empty()
    );
    assert_eq!(
        context
            .transcription_job(MEMBER.into(), done.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 1);
    assert_eq!(std::fs::read(fixture.transcript()).unwrap(), canonical);
    drop(context);
    fixture.finish().await;
}

#[tokio::test]
async fn local_principal_does_not_relax_operation_ids_or_accept_noncanonical_principals() {
    let fixture = Fixture::new(false).await;
    let context = fixture.context();
    for principal in [
        String::new(),
        MEMBER.to_uppercase(),
        MEMBER.replace('-', ""),
        format!("{{{MEMBER}}}"),
        LOCAL.replace('-', ""),
    ] {
        assert_eq!(
            context
                .list_transcription_jobs(principal.clone(), Default::default())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(
            context
                .submit_transcription_job(principal, REQUEST.into(), request())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    assert_eq!(
        context
            .submit_transcription_job(LOCAL.into(), LOCAL.into(), request())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .transcription_job(LOCAL.into(), LOCAL.into())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .transcription_job_result(LOCAL.into(), LOCAL.into())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .cancel_transcription_job(LOCAL.into(), LOCAL.into())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .resume_transcription_result(LOCAL.into(), LOCAL.into())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .list_transcription_jobs(
                LOCAL.into(),
                ListTranscriptionJobsInput {
                    limit: Some(50),
                    after: Some(LOCAL.into())
                }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 0);
    assert!(!fixture.transcript().exists());
    drop(context);
    fixture.finish().await;
}

#[tokio::test]
async fn local_principal_still_requires_original_host_authorization() {
    let fixture = Fixture::new(false).await;
    fixture.auth.allowed.store(false, Ordering::Release);
    let context = fixture.context();
    let submitted = context
        .submit_transcription_job(LOCAL.into(), REQUEST.into(), request())
        .await
        .unwrap();
    let done = local_terminal(&context, &submitted.id).await;
    assert_eq!(done.state, TranscriptionJobState::Failed);
    assert_eq!(done.failure, Some(TranscriptionFailure::AccessDenied));
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 0);
    assert!(!fixture.transcript().exists());
    drop(context);
    fixture.finish().await;
}
