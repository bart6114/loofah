use hypr_loofah_protocol::{
    AgentExport, AgentMeeting, AgentPage, AgentSearchPage, AgentTranscript, ApplicationError,
    Change, ErrorCode, GetMeetingInput, GetMeetingTranscriptInput, ListMeetingsInput,
    RenderedTranscriptSegment, SearchMeetingsInput,
};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn agent_list(
        &self,
        input: ListMeetingsInput,
    ) -> Result<AgentPage, ApplicationError> {
        self.agent_ready()?;
        if input.query.as_ref().is_some_and(|query| query.len() > 8192)
            || input
                .tags
                .as_ref()
                .is_some_and(|tags| tags.len() > 64 || tags.iter().any(|tag| tag.len() > 4096))
            || (input
                .query
                .as_ref()
                .is_some_and(|query| !query.trim().is_empty())
                && input.offset.unwrap_or(0) > 10_000)
        {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Agent query exceeds the request cost limit",
            ));
        }
        let cache = self
            .search
            .cache()
            .await
            .map_err(|_| error(ErrorCode::Unavailable, "Search is not ready"))?;
        self.finish_agent_projection(hypr_agent_access::list_meetings(&cache, input).await)
    }

    pub async fn agent_search(
        &self,
        input: SearchMeetingsInput,
    ) -> Result<AgentSearchPage, ApplicationError> {
        self.agent_ready()?;
        if input.query.len() > 8192 || input.offset.unwrap_or(0) > 10_000 {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Agent query exceeds the request cost limit",
            ));
        }
        let cache = self
            .search
            .cache()
            .await
            .map_err(|_| error(ErrorCode::Unavailable, "Search is not ready"))?;
        self.finish_agent_projection(hypr_agent_access::search_meetings(&cache, input).await)
    }

    pub async fn agent_meeting(&self, session_id: &str) -> Result<AgentMeeting, ApplicationError> {
        self.validate(session_id)?;
        let input = GetMeetingInput {
            meeting_id: session_id.into(),
        };
        self.agent_snapshot(session_id, move |vault| async move {
            hypr_agent_access::get_meeting(&vault, input).await
        })
        .await
    }

    pub async fn agent_transcript(
        &self,
        session_id: &str,
    ) -> Result<AgentTranscript, ApplicationError> {
        self.validate(session_id)?;
        let input = GetMeetingTranscriptInput {
            meeting_id: session_id.into(),
        };
        self.agent_snapshot(session_id, move |vault| async move {
            hypr_agent_access::get_meeting_transcript(&vault, input).await
        })
        .await
    }

    pub async fn agent_export(&self, session_id: &str) -> Result<AgentExport, ApplicationError> {
        self.validate(session_id)?;
        let id = session_id.to_owned();
        self.agent_snapshot(session_id, move |vault| async move {
            hypr_agent_access::get_meeting_export(&vault, id).await
        })
        .await
    }

    pub async fn agent_segments(
        &self,
        session_id: &str,
    ) -> Result<Vec<RenderedTranscriptSegment>, ApplicationError> {
        self.validate(session_id)?;
        let id = session_id.to_owned();
        self.agent_snapshot(session_id, move |vault| async move {
            hypr_agent_access::get_meeting_export_segments(&vault, id).await
        })
        .await
    }

    async fn agent_snapshot<T, F>(
        &self,
        session_id: &str,
        operation: impl FnOnce(std::path::PathBuf) -> F + Send + 'static,
    ) -> Result<T, ApplicationError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = hypr_agent_access::Result<T>> + Send + 'static,
    {
        let store = self.store.clone();
        let root = self.document_operation_root();
        let id = session_id.to_owned();
        // Recovery and blocking readers keep one snapshot lock even after cancellation.
        tokio::spawn(async move {
            let _snapshot = store
                .lock_recovered_snapshot(root.as_deref(), &id)
                .await
                .map_err(|cause| storage_error(cause, false))?;
            operation(store.vault_base().to_owned())
                .await
                .map_err(agent_error)
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "agent snapshot worker failed");
            error(ErrorCode::Internal, "Cannot read canonical session content")
        })?
    }

    fn agent_ready(&self) -> Result<(), ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))
    }

    fn finish_agent_projection<T>(
        &self,
        result: hypr_agent_access::Result<T>,
    ) -> Result<T, ApplicationError> {
        // Fresh agent reads can publish the search projection before the background worker sees it.
        self.events.publish(Change::SearchReady);
        result.map_err(agent_error)
    }
}

fn agent_error(cause: hypr_agent_access::Error) -> ApplicationError {
    use hypr_agent_access::Error;
    match cause {
        Error::NotFound(_) => error(ErrorCode::NotFound, "Session not found"),
        Error::InvalidInput(reason)
        | Error::Cache(hypr_search_cache::Error::InvalidInput(reason)) => {
            error(ErrorCode::Validation, &reason)
        }
        Error::Cache(
            hypr_search_cache::Error::NotReady
            | hypr_search_cache::Error::Busy
            | hypr_search_cache::Error::Changed,
        ) => error(
            ErrorCode::Unavailable,
            "Search projection is unavailable; retry later",
        ),
        cause => {
            tracing::error!(%cause, "agent read failed");
            error(ErrorCode::Internal, "Cannot read canonical session content")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::{EnhancedDoc, SessionStore, TaskInput};
    use serde_json::json;
    use std::sync::Arc;

    async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Arc<VaultContext>) {
        let vault = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let context = Arc::new(
            VaultContext::new(
                "agent".into(),
                Arc::new(SessionStore::new(vault.path().into())),
                StartupState::new(Some(vault.path())),
                Arc::new(ConfigState::load_or_default(vault.path())),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(runtime.path().join("operations")),
        );
        let meta = serde_json::from_value(json!({"id":"note","title":"0","created_at":"2026-11-05T10:00:00Z","tags":["project"],"author":"tester","skill":"verification"})).unwrap();
        context.store.create_note_session(&meta, "0").await.unwrap();
        context.store.ensure_summary("note").await.unwrap();
        context
            .store
            .update_generated_summary("note", "Summary text\n\n- [ ] Call Alice", Some(""))
            .await
            .unwrap();
        context
            .store
            .write_enhanced_doc(&EnhancedDoc {
                id: "output".into(),
                session_id: "note".into(),
                kind: "template_output".into(),
                title: "Report".into(),
                template_id: "report".into(),
                sort_order: 2,
                markdown: "Template output".into(),
            })
            .await
            .unwrap();
        context
            .store
            .replace_tasks(
                "session_raw_note",
                "note",
                vec![TaskInput {
                    id: "task".into(),
                    source_order: 0,
                    status: "done".into(),
                    text: "Checked action".into(),
                    body: json!([]),
                    due_at: "2026-12-01".into(),
                }],
            )
            .await
            .unwrap();
        let transcript = serde_json::from_value(json!({"id":"transcript","session_id":"note","words":[{"text":"First","start_ms":0,"end_ms":100,"channel":0},{"text":"last.","start_ms":110,"end_ms":200,"channel":0}]})).unwrap();
        context
            .store
            .write_transcript("note", transcript)
            .await
            .unwrap();
        std::fs::write(
            vault.path().join("sessions/note/foreign.md"),
            "ATTACHMENT CANARY",
        )
        .unwrap();
        std::fs::write(
            vault.path().join("sessions/note/.hidden.md"),
            "HIDDEN CANARY",
        )
        .unwrap();
        (vault, runtime, context)
    }

    #[tokio::test]
    async fn canonical_agent_models_and_complete_transcripts_match_existing_cli_readers() {
        let (vault, _runtime, context) = fixture().await;
        assert_eq!(
            context.agent_meeting("note").await.unwrap(),
            hypr_agent_access::get_meeting(
                vault.path(),
                GetMeetingInput {
                    meeting_id: "note".into()
                }
            )
            .await
            .unwrap()
        );
        assert_eq!(
            context.agent_transcript("note").await.unwrap(),
            hypr_agent_access::get_meeting_transcript(
                vault.path(),
                GetMeetingTranscriptInput {
                    meeting_id: "note".into()
                }
            )
            .await
            .unwrap()
        );
        let export = context.agent_export("note").await.unwrap();
        assert_eq!(
            export,
            hypr_agent_access::get_meeting_export(vault.path(), "note".into())
                .await
                .unwrap()
        );
        assert_eq!(export.transcripts[0].words.len(), 2);
        assert!(export.transcripts[0].text.ends_with("First last."));
        let segments = serde_json::to_value(context.agent_segments("note").await.unwrap()).unwrap();
        assert_eq!(
            segments,
            serde_json::to_value(
                hypr_agent_access::get_meeting_export_segments(vault.path(), "note".into())
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        let json = serde_json::to_string(&export).unwrap();
        assert!(!json.contains("CANARY"));
        assert!(!json.contains(&vault.path().to_string_lossy().to_string()));
    }

    #[tokio::test]
    async fn cancelling_an_admitted_projection_retains_its_lock_until_file_reads_finish() {
        let (_vault, _runtime, context) = fixture().await;
        let (started, admitted) = tokio::sync::oneshot::channel();
        let (finish, completed) = tokio::sync::oneshot::channel();
        let reader = context.clone();
        let task = tokio::spawn(async move {
            reader
                .agent_snapshot("note", move |_vault| async move {
                    started.send(()).unwrap();
                    completed.await.unwrap();
                    Ok(())
                })
                .await
        });
        admitted.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let writer = context.store.lock_snapshot();
        tokio::pin!(writer);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut writer)
                .await
                .is_err()
        );
        finish.send(()).unwrap();
        let _guard = tokio::time::timeout(std::time::Duration::from_secs(2), writer)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn composite_agent_reads_do_not_mix_a_note_with_another_title_revision() {
        let (_vault, _runtime, context) = fixture().await;
        let writer = context.clone();
        let task = tokio::spawn(async move {
            for number in 1..=30 {
                let current = writer.read_note("note").await.unwrap();
                let text = number.to_string();
                writer
                    .save_note(
                        "note",
                        hypr_loofah_protocol::SaveNoteRequest {
                            markdown: text.clone(),
                            title: Some(text),
                            expected_revision: current.revision,
                        },
                    )
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
        });
        for _ in 0..30 {
            let meeting = context.agent_meeting("note").await.unwrap();
            assert_eq!(meeting.title, meeting.note.unwrap().markdown);
        }
        task.await.unwrap();
    }

    #[tokio::test]
    async fn fresh_agent_queries_publish_search_readiness_and_keep_existing_pagination_filters() {
        let (vault, runtime, context) = fixture().await;
        context
            .search
            .initialize(&runtime.path().join("cache"), vault.path())
            .await
            .unwrap();
        let meta = serde_json::from_value(json!({"id":"second","title":"Unique zephyr","created_at":"2026-11-06T10:00:00Z","tags":["project"]})).unwrap();
        context
            .store
            .create_note_session(&meta, "Content")
            .await
            .unwrap();
        let mut events = context.events.subscribe(None);
        assert_eq!(events.recv().await.unwrap().change, Change::ResyncRequired);
        let input = ListMeetingsInput {
            query: Some("zephyr".into()),
            limit: Some(1),
            tags: Some(vec!["PROJECT".into()]),
            ..Default::default()
        };
        let page = context.agent_list(input.clone()).await.unwrap();
        assert_eq!(page.meetings.len(), 1);
        assert_eq!(page.meetings[0].id, "second");
        assert_eq!(
            page,
            hypr_agent_access::list_meetings(&context.search.cache().await.unwrap(), input)
                .await
                .unwrap()
        );
        assert_eq!(events.recv().await.unwrap().change, Change::SearchReady);
        let result = context
            .agent_search(SearchMeetingsInput {
                query: "zephyr".into(),
                limit: Some(1),
                offset: None,
            })
            .await
            .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].session_id, "second");
        let empty = context
            .agent_list(ListMeetingsInput {
                offset: Some(u32::MAX),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(empty.meetings.is_empty());
    }

    #[tokio::test]
    async fn agent_reads_reject_missing_sessions_paths_and_unbounded_query_cost_without_leaking_paths()
     {
        let (_vault, _runtime, context) = fixture().await;
        for id in ["../outside", ".hidden", "CON", "a/b", "a\\b"] {
            assert_eq!(
                context.agent_meeting(id).await.unwrap_err().code,
                ErrorCode::Validation
            );
        }
        assert_eq!(
            context.agent_export("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            context.agent_transcript("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            context.agent_segments("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            context
                .agent_search(SearchMeetingsInput {
                    query: "term".into(),
                    offset: Some(u32::MAX),
                    limit: Some(u32::MAX)
                })
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        assert_eq!(
            context
                .agent_list(ListMeetingsInput {
                    query: Some("term".into()),
                    offset: Some(u32::MAX),
                    ..Default::default()
                })
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        context.store.set_startup_pending(true);
        assert_eq!(
            context.agent_meeting("note").await.unwrap_err().code,
            ErrorCode::Unavailable
        );
    }
}
