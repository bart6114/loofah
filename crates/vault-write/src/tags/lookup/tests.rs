use super::*;
use serde_json::json;

fn header(store: &SessionStore, id: &str, tags: Vec<String>) {
    store.index_upsert_meta(
        &serde_json::from_value(
            json!({"id":id,"title":"","created_at":"2026-11-08T00:00:00Z","tags":tags}),
        )
        .unwrap(),
    );
}

#[tokio::test]
async fn union_matches_existing_typeahead_order_dedupe_exclusions_and_raw_names() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().into());
    std::fs::write(
        root.path().join("tags.json"),
        json!({"future":{"keep":true},"tags":[
            {"id":"registry","name":"registry"},{"id":"duplicate","name":"duplicate"},
            {"id":"other-id","name":"duplicate"},{"id":"import","name":"import/audio"},
            {"id":"upper","name":"Project"},{"id":"empty","name":""}
        ]})
        .to_string(),
    )
    .unwrap();
    header(
        &store,
        "one",
        vec![
            "hand/edited".into(),
            "duplicate".into(),
            "project".into(),
            "#raw".into(),
            "😀".into(),
            "\u{e000}".into(),
        ],
    );
    assert_eq!(
        store
            .tag_suggestions("", &["duplicate".into()], 8)
            .await
            .unwrap(),
        [
            "",
            "#raw",
            "Project",
            "hand/edited",
            "import/audio",
            "project",
            "registry",
            "😀"
        ]
    );
    assert_eq!(
        store.tag_suggestions("# PRO", &[], 8).await.unwrap(),
        ["project"]
    );
    assert_eq!(
        store
            .tag_suggestions("", &["Project".into(), "project".into()], 1)
            .await
            .unwrap(),
        [""]
    );
    assert_eq!(
        store.tag_suggestions("IMPORT", &[], 8).await.unwrap(),
        ["import/audio"]
    );
}

#[tokio::test]
async fn normalization_matches_js_hash_trim_bom_and_nel_behavior() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().into());
    header(
        &store,
        "one",
        vec![
            "project".into(),
            "#project".into(),
            "\u{85}project\u{85}".into(),
            "##project".into(),
        ],
    );
    assert_eq!(
        store
            .tag_suggestions("\u{feff}PROJECT\u{feff}", &[], 8)
            .await
            .unwrap(),
        ["##project", "#project", "project", "\u{85}project\u{85}"]
    );
    assert_eq!(
        store
            .tag_suggestions("\u{85}PROJECT\u{85}", &[], 8)
            .await
            .unwrap(),
        ["\u{85}project\u{85}"]
    );
    assert_eq!(
        store.tag_suggestions(" #PROJECT ", &[], 8).await.unwrap(),
        ["##project", "#project"]
    );
    assert_eq!(
        store.tag_suggestions("##PROJECT", &[], 8).await.unwrap(),
        ["##project", "#project"]
    );
}

#[tokio::test]
async fn searches_the_whole_header_index_before_limiting_and_returns_at_most_the_requested_names() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().into());
    for n in 0..12_000 {
        header(&store, &format!("note-{n}"), vec![format!("z-{n:05}")]);
    }
    header(
        &store,
        "late",
        vec!["needle-after-all-pages".into(), "a-first".into()],
    );
    assert_eq!(
        store.tag_suggestions("needle", &[], 8).await.unwrap(),
        ["needle-after-all-pages"]
    );
    let names = store
        .tag_suggestions("", &["a-first".into()], 8)
        .await
        .unwrap();
    assert_eq!(names.len(), 8);
    assert_eq!(names[0], "needle-after-all-pages");
    assert_eq!(names[7], "z-00006");
}

#[tokio::test]
async fn missing_or_invalid_registry_keeps_in_use_tags_and_future_refreshes_are_canonical() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().into());
    header(&store, "note", vec!["header".into()]);
    assert_eq!(store.tag_suggestions("", &[], 8).await.unwrap(), ["header"]);
    std::fs::write(root.path().join("tags.json"), "not json").unwrap();
    assert_eq!(store.tag_suggestions("", &[], 8).await.unwrap(), ["header"]);
    std::fs::write(
        root.path().join("tags.json"),
        r#"{"tags":[{"id":"fresh","name":"fresh"}]}"#,
    )
    .unwrap();
    assert_eq!(
        store.tag_suggestions("", &[], 8).await.unwrap(),
        ["fresh", "header"]
    );
    store.index_remove_session("note");
    assert_eq!(store.tag_suggestions("", &[], 8).await.unwrap(), ["fresh"]);
}

#[tokio::test]
async fn bounds_fail_explicitly_instead_of_truncating_sources_or_oversize_results() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().into());
    assert!(matches!(
        store.tag_suggestions(&"x".repeat(4097), &[], 8).await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert!(matches!(
        store.tag_suggestions("", &vec!["x".into(); 257], 8).await,
        Err(StoreError::ResourceLimit(_))
    ));
    for limit in [0, 201] {
        assert!(matches!(
            store.tag_suggestions("", &[], limit).await,
            Err(StoreError::ResourceLimit(_))
        ));
    }
    let large = "x".repeat(MAX_RESULT + 1);
    header(&store, "large", vec![large.clone()]);
    assert!(
        store
            .tag_suggestions("", &[large], 8)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store.tag_suggestions("", &[], 8).await,
        Err(StoreError::ResourceLimit(_))
    ));
    store.set_startup_pending(true);
    assert!(store.tag_suggestions("", &[], 8).await.is_err());
}
