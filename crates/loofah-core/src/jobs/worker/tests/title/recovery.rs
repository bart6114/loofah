use std::io::Write;

use crate::jobs::{
    configured::{
        ConfiguredSummaryGenerator, LlmSelection, ProcessingAdmission, SummaryModel, SummaryModels,
    },
    generation::{ModelChunk, ModelRequest, ModelStream, TextModel},
    source::SummaryIdentity,
};

use super::*;

struct CountedModel {
    path: PathBuf,
}

impl TextModel for CountedModel {
    fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        Box::pin(async move {
            let title = request.max_output_tokens == 128;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .unwrap();
            writeln!(file, "{}", if title { "title" } else { "body" }).unwrap();
            file.sync_all().unwrap();
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelChunk::Text(
                    if title { "Paid title" } else { BODY }.into(),
                )),
                Ok(ModelChunk::Finish { truncated: false }),
            ])) as ModelStream)
        })
    }
}

pub(super) struct Models(pub(super) Arc<dyn TextModel>);

impl SummaryModels for Models {
    fn resolve(
        &self,
        _: LlmSelection,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryModel, ProcessingJobFailure>> {
        Box::pin(async move {
            Ok(SummaryModel {
                model: self.0.clone(),
                include_images: false,
            })
        })
    }
}

async fn context(root: &Path, create: bool) -> Arc<VaultContext> {
    let vault = root.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let context = Arc::new(
        VaultContext::new(
            "crash-vault".into(),
            Arc::new(SessionStore::new(vault.clone())),
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.join("operations")),
    );
    if create {
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap();
        context
            .store
            .create_note_session(&meta, "Source notes.")
            .await
            .unwrap();
        context.store.ensure_summary("note").await.unwrap();
        context
            .settings
            .set_values(std::collections::HashMap::from([
                ("current_llm_provider".into(), serde_json::json!("custom")),
                (
                    "current_llm_model".into(),
                    serde_json::json!("counted-model"),
                ),
            ]))
            .await
            .unwrap();
    }
    context
        .store
        .recover_document_writes(&context.document_operation_root().unwrap())
        .await
        .unwrap();
    context
}

fn generator(context: &Arc<VaultContext>, root: &Path) -> Arc<dyn JobGenerator> {
    Arc::new(ConfiguredSummaryGenerator::new(
        context,
        SummaryIdentity::LocalOwner,
        Arc::new(Models(Arc::new(CountedModel {
            path: root.join("paid-calls"),
        }))),
        ProcessingAdmission::new(1, 0).unwrap(),
        true,
    ))
}

#[tokio::test]
#[ignore = "subprocess crash harness"]
async fn title_crash_child() {
    let Some(root) = std::env::var_os("LOOFAH_TITLE_TEST_ROOT").map(PathBuf::from) else {
        return;
    };
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    let context = context(&root, true).await;
    let journal = JobJournal::open(root.join("jobs"), context.events.clone())
        .await
        .unwrap();
    let baseline = context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let job = journal
        .submit(
            "member-one".into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                only_if_empty: false,
                session_id: "note".into(),
                target: DocumentTarget::Summary,
                expected_revision: baseline.revision,
                title_policy: TitlePolicy::GenerateIfEmpty,
            },
        )
        .await
        .unwrap();
    let mut file = std::fs::File::create(root.join("job-id")).unwrap();
    file.write_all(job.id.as_bytes()).unwrap();
    file.sync_all().unwrap();
    let worker = JobWorker::start(
        context.clone(),
        journal.clone(),
        generator(&context, &root),
        Arc::new(Authorizer {
            allowed: Arc::new(RwLock::new(true)),
        }),
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while !journal
            .get(job.id.clone())
            .await
            .unwrap()
            .unwrap()
            .state
            .is_terminal()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    worker.shutdown().await.unwrap();
    panic!("requested crash checkpoint was not reached");
}

#[tokio::test]
async fn actual_crash_reopen_preserves_confirmed_bodies_candidates_and_paid_call_counts() {
    for point in [
        "body_saved",
        "body_confirmed",
        "title_generating",
        "title_generated",
        "title_candidate",
        "title_applying",
        "title_applied",
        "title_finished",
    ] {
        let root = tempfile::tempdir().unwrap();
        let location = root.path().to_owned();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "jobs::worker::tests::title::recovery::title_crash_child",
                ])
                .env("LOOFAH_TITLE_TEST_ROOT", &location)
                .env("LOOFAH_TITLE_CRASH_STAGE", point)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(
            output.status.code(),
            Some(91),
            "{point}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let before = std::fs::read(root.path().join("paid-calls")).unwrap();
        let counts = String::from_utf8(before.clone()).unwrap();
        assert_eq!(
            counts.lines().filter(|line| *line == "body").count(),
            1,
            "{point}"
        );
        assert_eq!(
            counts.lines().filter(|line| *line == "title").count(),
            usize::from(!matches!(
                point,
                "body_saved" | "body_confirmed" | "title_generating"
            )),
            "{point}"
        );
        let context = context(root.path(), false).await;
        let journal = JobJournal::open(root.path().join("jobs"), context.events.clone())
            .await
            .unwrap();
        let id = std::fs::read_to_string(root.path().join("job-id")).unwrap();
        let job = journal.get(id.clone()).await.unwrap().unwrap();
        assert_eq!(
            job.state,
            if point == "body_saved" {
                ProcessingJobState::Interrupted
            } else {
                ProcessingJobState::Succeeded
            },
            "{point}"
        );
        let result = journal
            .owned_result("member-one".into(), id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.markdown, BODY);
        if point == "body_saved" {
            assert!(result.title.is_none());
        } else {
            let title = result.title.unwrap();
            assert_eq!(
                title.status,
                if point == "title_finished" {
                    ProcessingTitleStatus::Applied
                } else if matches!(point, "title_applying" | "title_applied") {
                    ProcessingTitleStatus::Unconfirmed
                } else {
                    ProcessingTitleStatus::Interrupted
                },
                "{point}"
            );
            assert_eq!(
                title.candidate.is_some(),
                matches!(
                    point,
                    "title_candidate" | "title_applying" | "title_applied" | "title_finished"
                ),
                "{point}"
            );
        }
        let worker = JobWorker::start(
            context.clone(),
            journal.clone(),
            generator(&context, root.path()),
            Arc::new(Authorizer {
                allowed: Arc::new(RwLock::new(true)),
            }),
        )
        .await
        .unwrap();
        worker.shutdown().await.unwrap();
        assert_eq!(
            std::fs::read(root.path().join("paid-calls")).unwrap(),
            before,
            "{point}: generation replayed"
        );
        let title = context
            .store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .title;
        assert_eq!(
            title,
            if matches!(point, "title_applied" | "title_finished") {
                "Paid title"
            } else {
                ""
            },
            "{point}"
        );
        assert_eq!(journal.get(id).await.unwrap().unwrap().state, job.state);
    }
}
