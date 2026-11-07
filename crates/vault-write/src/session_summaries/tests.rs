use super::*;
use crate::index::SessionEntry;
use serde_json::json;

fn meta(id: &str, title: &str, day: u32) -> SessionMeta {
    serde_json::from_value(json!({"id":id,"title":title,"created_at":format!("2026-01-{day:02}T00:00:00Z"),"tags":[],"future":"PRIVATE META"})).unwrap()
}
fn ids(rows: &[SessionSummary]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_str()).collect()
}

#[test]
fn queries_are_bounded_over_the_full_header_set_without_returning_content_or_extras() {
    let vault = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    for i in 0..2000 {
        let id = format!("note-{i:04}");
        let title = if i == 3 { "Åland planning" } else { "Other" };
        let item = meta(&id, title, 1 + i % 28);
        store.index.write().unwrap().sessions.insert(
            id,
            SessionEntry {
                meta: item,
                note_markdown: Some("PRIVATE NOTE BODY".into()),
            },
        );
    }
    let rows = store
        .indexed_session_summaries(&SessionSummaryQuery::Title {
            query: "  åLAND  ".into(),
            limit: 50,
        })
        .unwrap();
    assert_eq!(ids(&rows), vec!["note-0003"]);
    let value = serde_json::to_value(&rows).unwrap();
    assert_eq!(value[0].as_object().unwrap().len(), 3);
    assert!(!value.to_string().contains("PRIVATE"));
    let rows = store
        .indexed_session_summaries(&SessionSummaryQuery::List {
            limit: 55,
            order: SessionSummaryOrder::NewestFirst,
            titled_only: false,
        })
        .unwrap();
    assert_eq!(rows.len(), 55);
    for pair in rows.windows(2) {
        assert!(
            pair[0].created_at > pair[1].created_at
                || pair[0].created_at == pair[1].created_at && pair[0].id < pair[1].id
        );
    }
}

#[test]
fn list_order_ties_and_titled_filter_are_applied_before_the_limit() {
    let vault = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    for item in [
        meta("empty", "", 1),
        meta("b", "Second", 3),
        meta("a", "First", 3),
        meta("older", "Older", 2),
    ] {
        store.index.write().unwrap().sessions.insert(
            item.id.clone(),
            SessionEntry {
                meta: item,
                note_markdown: None,
            },
        );
    }
    let query = |order, titled_only| SessionSummaryQuery::List {
        limit: 2,
        order,
        titled_only,
    };
    assert_eq!(
        ids(&store
            .indexed_session_summaries(&query(SessionSummaryOrder::OldestFirst, true))
            .unwrap()),
        vec!["older", "a"]
    );
    assert_eq!(
        ids(&store
            .indexed_session_summaries(&query(SessionSummaryOrder::NewestFirst, false))
            .unwrap()),
        vec!["a", "b"]
    );
    assert_eq!(
        ids(&store
            .indexed_session_summaries(&query(SessionSummaryOrder::OldestFirst, false))
            .unwrap()),
        vec!["empty", "older"]
    );
}

#[tokio::test]
async fn id_queries_use_canonical_metadata_preserve_requested_order_and_ignore_unknown_files() {
    let vault = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    for item in [
        meta("a", "Old A", 1),
        meta("b", "B", 2),
        meta("gone", "Deleted", 3),
    ] {
        store
            .create_note_session(&item, "PRIVATE NOTE BODY")
            .await
            .unwrap();
    }
    let path = vault.path().join("sessions/a");
    std::fs::write(
        path.join("_meta.json"),
        serde_json::to_vec(&meta("a", "Canonical A", 4)).unwrap(),
    )
    .unwrap();
    std::fs::write(path.join("user.md"), "ATTACHMENT CANARY").unwrap();
    std::fs::write(path.join(".hidden"), "HIDDEN CANARY").unwrap();
    std::fs::remove_file(vault.path().join("sessions/gone/_meta.json")).unwrap();
    assert!(store.session_get("gone").is_some());
    let requested = ["b", "gone", "a", "b", "missing"].map(str::to_string);
    let rows = store
        .canonical_session_summaries(None, &requested)
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec!["b", "a"]);
    assert_eq!(rows[1].title, "Canonical A");
    assert_eq!(
        std::fs::read_to_string(path.join("user.md")).unwrap(),
        "ATTACHMENT CANARY"
    );
    assert_eq!(
        std::fs::read_to_string(path.join(".hidden")).unwrap(),
        "HIDDEN CANARY"
    );
    assert!(!vault.path().join("sessions/missing").exists());
}

#[test]
fn invalid_inputs_readiness_and_response_budgets_are_enforced_before_copying_large_fields() {
    let vault = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    for query in [
        SessionSummaryQuery::List {
            limit: 201,
            order: SessionSummaryOrder::NewestFirst,
            titled_only: false,
        },
        SessionSummaryQuery::Title {
            query: " ".into(),
            limit: 1,
        },
        SessionSummaryQuery::Ids {
            ids: vec!["../escape".into()],
        },
    ] {
        assert!(store.indexed_session_summaries(&query).is_err());
    }
    let item = meta("large", &"x".repeat(SESSION_SUMMARY_TEXT_LIMIT + 1), 1);
    store.index.write().unwrap().sessions.insert(
        "large".into(),
        SessionEntry {
            meta: item,
            note_markdown: None,
        },
    );
    let query = SessionSummaryQuery::List {
        limit: 1,
        order: SessionSummaryOrder::OldestFirst,
        titled_only: false,
    };
    assert!(matches!(
        store.indexed_session_summaries(&query),
        Err(StoreError::ResourceLimit(_))
    ));
    store.set_startup_pending(true);
    assert!(store.indexed_session_summaries(&query).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn canonical_id_reads_cannot_follow_another_vaults_session_directory() {
    let vault = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let source = SessionStore::new(other.path().into());
    source
        .create_note_session(&meta("note", "FOREIGN CANARY", 1), "")
        .await
        .unwrap();
    std::fs::create_dir(vault.path().join("sessions")).unwrap();
    std::os::unix::fs::symlink(
        other.path().join("sessions/note"),
        vault.path().join("sessions/note"),
    )
    .unwrap();
    let store = SessionStore::new(vault.path().into());
    assert!(
        store
            .canonical_session_summaries(None, &["note".into()])
            .await
            .is_err()
    );
}
