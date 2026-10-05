use super::*;
use crate::{SearchFilters, SearchOptions};

fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Cache) {
    let global = tempfile::tempdir().unwrap();
    let vault = tempfile::tempdir().unwrap();
    let cache = Cache::new(global.path(), vault.path()).unwrap();
    (global, vault, cache)
}

fn session(vault: &Path, id: &str, title: &str, note: &str, tags: &[&str]) {
    let dir = vault.join("sessions").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("_meta.json"), serde_json::json!({"id":id,"title":title,"tags":tags,"created_at":"2026-09-01T10:00:00Z","started_at":null,"ended_at":null}).to_string()).unwrap();
    fs::write(dir.join("notes.md"), note).unwrap();
}

fn query(text: &str, limit: usize) -> SearchRequest {
    SearchRequest {
        query: text.into(),
        collection: None,
        filters: SearchFilters::default(),
        limit,
        options: SearchOptions {
            snippets: Some(true),
            ..Default::default()
        },
    }
}

#[test]
fn missing_cache_needs_explicit_initialization_and_empty_vault_is_ready() {
    let (_global, _vault, cache) = fixture();
    assert!(matches!(cache.check(), Err(Error::NotReady)));
    assert!(!cache.path().exists());
    assert!(matches!(
        cache.search_fresh(query("hello", 10)),
        Err(Error::NotReady)
    ));
    let report = cache.initialize(&mut |_| {}).unwrap();
    assert_eq!(report.sessions, 0);
    assert!(cache.status().ready);
    assert!(
        cache
            .search_fresh(query("hello", 10))
            .unwrap()
            .hits
            .is_empty()
    );
}

#[test]
fn incremental_reconciliation_adds_edits_and_deletes_only_changed_sessions() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Planning", "first", &[]);
    session(vault.path(), "b", "Review", "other", &[]);
    assert_eq!(cache.initialize(&mut |_| {}).unwrap().added, 2);
    let pointer = fs::read(cache.path().join("CURRENT")).unwrap();
    let (index, _) = cache.open().unwrap();
    let opstamp = index.load_metas().unwrap().opstamp;
    let report = cache.reconcile(&mut |_| {}).unwrap();
    assert_eq!((report.added, report.updated, report.deleted), (0, 0, 0));
    assert_eq!(index.load_metas().unwrap().opstamp, opstamp);
    assert_eq!(fs::read(cache.path().join("CURRENT")).unwrap(), pointer);
    fs::write(vault.path().join("sessions/a/notes.md"), "third").unwrap();
    session(vault.path(), "c", "New", "created", &[]);
    fs::remove_dir_all(vault.path().join("sessions/b")).unwrap();
    let report = cache.reconcile(&mut |_| {}).unwrap();
    assert_eq!((report.added, report.updated, report.deleted), (1, 1, 1));
    assert_eq!(
        cache.search(query("third", 10)).unwrap().hits[0]
            .document
            .id,
        "a"
    );
    assert!(cache.search(query("other", 10)).unwrap().hits.is_empty());
}

#[test]
fn atomic_replacement_and_same_size_same_mtime_edits_are_detected() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "first", &[]);
    cache.initialize(&mut |_| {}).unwrap();
    let path = vault.path().join("sessions/a/notes.md");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, "third").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(cache.reconcile(&mut |_| {}).unwrap().updated, 1);
    let replacement = path.with_file_name(".replacement");
    fs::write(&replacement, "third").unwrap();
    File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::rename(replacement, path).unwrap();
    assert_eq!(cache.reconcile(&mut |_| {}).unwrap().updated, 1);
}

#[test]
fn external_edits_are_visible_before_search_and_filtered_lists() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Old", "none", &["work"]);
    cache.initialize(&mut |_| {}).unwrap();
    session(vault.path(), "b", "New", "zebra", &["work", "hiring"]);
    assert_eq!(
        cache.search_fresh(query("zebra", 10)).unwrap().hits[0]
            .document
            .id,
        "b"
    );
    fs::write(vault.path().join("sessions/a/notes.md"), "zebra").unwrap();
    let (page, total) = cache
        .list_fresh(Some("zebra"), &["HIRING".into()], false, 0, 1)
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(page[0].id, "b");
    let (page, total) = cache.list_fresh(Some("zebra"), &[], false, 1, 1).unwrap();
    assert_eq!(total, 2);
    assert_eq!(page.len(), 1);
}

#[test]
fn desktop_and_cli_share_matching_ranking_and_session_snippets() {
    let (_global, vault, cache) = fixture();
    session(
        vault.path(),
        "a",
        "Résumé planning",
        "Budget decisions",
        &[],
    );
    session(vault.path(), "b", "Other", "Résumé planning budget", &[]);
    cache.initialize(&mut |_| {}).unwrap();
    for text in [
        "RESUME",
        "résum",
        "\"Résumé planning\"",
        "budget planning",
        "résumé \"planning\"",
    ] {
        let desktop = cache.search(query(text, 10)).unwrap();
        let cli = cache.search_fresh(query(text, 10)).unwrap();
        assert_eq!(
            serde_json::to_value(desktop).unwrap(),
            serde_json::to_value(cli).unwrap()
        );
    }
    let result = cache.search(query("resume", 10)).unwrap();
    assert_eq!(result.hits[0].document.id, "a");
    assert!(
        !result.hits[0]
            .title_snippet
            .as_ref()
            .unwrap()
            .highlights
            .is_empty()
    );
    assert!(cache.search(query("sume", 10)).unwrap().hits.is_empty());
    assert!(cache.search(query("budegt", 10)).unwrap().hits.is_empty());
    assert!(
        cache
            .search(query("\"planning resume\"", 10))
            .unwrap()
            .hits
            .is_empty()
    );
}

#[test]
fn equal_scores_have_stable_id_order_before_pagination() {
    let (_global, vault, cache) = fixture();
    for id in ["z", "c", "a", "b"] {
        session(vault.path(), id, "Identical", "zebra", &[]);
    }
    cache.initialize(&mut |_| {}).unwrap();
    let result = cache.search(query("zebra", 2)).unwrap();
    assert_eq!(result.count, 4);
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.document.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    for _ in 0..3 {
        assert_eq!(
            cache.search(query("zebra", 2)).unwrap().hits[1].document.id,
            "b"
        );
    }
}

#[test]
fn known_documents_and_legacy_note_are_indexed_attachments_and_transients_are_ignored() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "note", &[]);
    let dir = vault.path().join("sessions/a");
    fs::rename(dir.join("notes.md"), dir.join("_memo.md")).unwrap();
    fs::write(dir.join("summary.md"), "summaryword").unwrap();
    fs::create_dir(dir.join("enhanced")).unwrap();
    fs::write(
        dir.join("enhanced/output.md"),
        "---\nkind: template_output\n---\n\ntemplateword",
    )
    .unwrap();
    fs::write(dir.join("transcript.json"), serde_json::json!({"transcripts":[{"id":"t","session_id":"a","started_at":0.0,"words":[{"text":"transcriptword","start_ms":0.0,"end_ms":100.0,"channel":0.0}]}]}).to_string()).unwrap();
    cache.initialize(&mut |_| {}).unwrap();
    for name in [
        "unknown.md",
        ".hidden",
        "audio_mic.wav",
        "audio.mp3.tmp",
        "enhanced/.hidden.md",
    ] {
        fs::write(dir.join(name), "secretword").unwrap();
    }
    assert_eq!(cache.reconcile(&mut |_| {}).unwrap().updated, 0);
    assert!(
        cache
            .search(query("secretword", 10))
            .unwrap()
            .hits
            .is_empty()
    );
    for text in ["note", "summaryword", "templateword", "transcriptword"] {
        assert_eq!(
            cache.search(query(text, 10)).unwrap().hits[0].document.id,
            "a"
        );
    }
    fs::remove_file(dir.join("enhanced/output.md")).unwrap();
    assert!(
        cache
            .search_fresh(query("templateword", 10))
            .unwrap()
            .hits
            .is_empty()
    );
}

#[test]
fn unreadable_content_does_not_delete_or_advance_fingerprints() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "original", &[]);
    cache.initialize(&mut |_| {}).unwrap();
    fs::write(vault.path().join("sessions/a/transcript.json"), "invalid").unwrap();
    let error = cache.search_fresh(query("original", 10)).unwrap_err();
    assert!(error.to_string().contains("session a:"));
    assert!(error.to_string().contains("transcript.json"));
    assert_eq!(cache.search(query("original", 10)).unwrap().count, 1);
    assert!(cache.open().unwrap().1.sources != cache.sources().unwrap());
    fs::remove_file(vault.path().join("sessions/a/transcript.json")).unwrap();
    cache.reconcile(&mut |_| {}).unwrap();
    assert!(cache.status().ready);
}

#[test]
fn incomplete_or_corrupt_cache_is_rebuilt_by_init_only() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "content", &[]);
    fs::create_dir_all(cache.path()).unwrap();
    fs::write(cache.path().join("CURRENT"), "../escape").unwrap();
    assert!(matches!(cache.check(), Err(Error::Corrupt(_))));
    cache.initialize(&mut |_| {}).unwrap();
    let meta_path = cache
        .path()
        .join("generations")
        .join(fs::read_to_string(cache.path().join("CURRENT")).unwrap())
        .join("meta.json");
    let mut metas: serde_json::Value =
        serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    let mut payload: serde_json::Value =
        serde_json::from_str(metas["payload"].as_str().unwrap()).unwrap();
    payload["schema_version"] = serde_json::json!(0);
    metas["payload"] = serde_json::json!(payload.to_string());
    fs::write(&meta_path, metas.to_string()).unwrap();
    assert!(matches!(cache.check(), Err(Error::NotReady)));
    cache.initialize(&mut |_| {}).unwrap();
    assert_eq!(cache.search(query("content", 10)).unwrap().count, 1);
}

#[test]
fn interrupted_build_never_publishes_ready_cache() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "content", &[]);
    let result = std::panic::catch_unwind(|| {
        cache.initialize(&mut |event| {
            if event.phase == "commit" {
                panic!("simulated crash");
            }
        })
    });
    assert!(result.is_err());
    assert!(!cache.status().ready);
    assert!(!cache.path().join("CURRENT").exists());
    cache.initialize(&mut |_| {}).unwrap();
    assert!(cache.status().ready);
}

#[test]
fn edit_during_reconciliation_remains_pending() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "old", &[]);
    cache.initialize(&mut |_| {}).unwrap();
    fs::write(vault.path().join("sessions/a/notes.md"), "first edit").unwrap();
    let result = cache.reconcile(&mut |event| {
        if event.phase == "index" {
            fs::write(vault.path().join("sessions/a/notes.md"), "later edit").unwrap();
        }
    });
    assert!(matches!(result, Err(Error::Changed)));
    assert_eq!(cache.search_fresh(query("later", 10)).unwrap().count, 1);
}

#[test]
fn cache_is_shared_between_clients_and_writers_release_the_lock() {
    let (global, vault, first) = fixture();
    session(vault.path(), "a", "Title", "old", &[]);
    first.initialize(&mut |_| {}).unwrap();
    let second = Cache::new(global.path(), vault.path()).unwrap();
    fs::write(vault.path().join("sessions/a/notes.md"), "newcontent").unwrap();
    let other = second.clone();
    let worker = std::thread::spawn(move || other.reconcile(&mut |_| {}).unwrap());
    first.reconcile(&mut |_| {}).unwrap();
    worker.join().unwrap();
    assert_eq!(first.search(query("newcontent", 10)).unwrap().count, 1);
    let _writer = first.lock(true, &mut |_| {}).unwrap();
    let mut events = Vec::new();
    assert!(matches!(
        second.lock_file(
            "writer.lock",
            true,
            Duration::from_millis(110),
            &mut |event| events.push(event.phase)
        ),
        Err(Error::Busy)
    ));
    assert!(events.contains(&"waiting"));
    // Desktop readers can query the committed generation while a reconciler owns the writer lock.
    assert_eq!(second.search(query("newcontent", 10)).unwrap().count, 1);
}

#[test]
fn registry_includes_unused_tags_and_refreshes_external_changes() {
    let (_global, vault, cache) = fixture();
    cache.initialize(&mut |_| {}).unwrap();
    fs::write(
        vault.path().join("tags.json"),
        serde_json::json!({"tags":[{"id":"unused","name":"Unused"}]}).to_string(),
    )
    .unwrap();
    let tags = cache.tags_fresh().unwrap();
    assert_eq!(tags[0].id, "unused");
    assert_eq!(cache.reconcile(&mut |_| {}).unwrap().updated, 0);
    fs::write(vault.path().join("tags.json"), "malformed").unwrap();
    assert!(cache.tags_fresh().is_err());
}

#[test]
fn growing_recording_does_not_reindex_unchanged_text() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "content", &[]);
    let audio = vault.path().join("sessions/a/audio.wav");
    fs::write(&audio, b"initial audio").unwrap();
    cache.initialize(&mut |_| {}).unwrap();
    fs::write(&audio, b"audio that grew during recording").unwrap();
    assert_eq!(cache.reconcile(&mut |_| {}).unwrap().updated, 0);
    assert_eq!(
        cache
            .search_fresh(query("content", usize::MAX))
            .unwrap()
            .count,
        1
    );
}

#[test]
#[ignore = "production performance workload"]
fn large_vault_with_concurrent_recording_writes() {
    let (_global, vault, cache) = fixture();
    let content = "Planning decisions budgets launch milestones. ".repeat(200);
    for n in 0..2000 {
        session(
            vault.path(),
            &format!("session-{n:05}"),
            &format!("Planning {n}"),
            &content,
            &["work"],
        );
    }
    let audio = vault.path().join("sessions/session-00000/audio.wav");
    fs::write(&audio, b"recording").unwrap();
    let started = Instant::now();
    cache.initialize(&mut |_| {}).unwrap();
    println!("Initial 2000-session cache: {:?}", started.elapsed());
    let recording = std::thread::spawn(move || {
        let mut file = File::options().append(true).open(audio).unwrap();
        for _ in 0..50 {
            file.write_all(&[0; 4096]).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        file.sync_all().unwrap();
    });
    let started = Instant::now();
    for _ in 0..5 {
        assert_eq!(
            cache.search_fresh(query("planning", 20)).unwrap().count,
            2000
        );
    }
    println!(
        "Five fresh searches during recording writes: {:?}",
        started.elapsed()
    );
    fs::write(
        vault.path().join("sessions/session-00001/notes.md"),
        "newchangedword",
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(cache.reconcile(&mut |_| {}).unwrap().updated, 1);
    println!("Incremental update of one session: {:?}", started.elapsed());
    recording.join().unwrap();
    assert!(
        fs::metadata(vault.path().join("sessions/session-00000/audio.wav"))
            .unwrap()
            .len()
            > 200_000
    );
}

#[test]
fn scoped_reconciliation_leaves_unrelated_corrupt_sessions_pending() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Planning", "first", &[]);
    session(vault.path(), "b", "Review", "other", &[]);
    cache.initialize(&mut |_| {}).unwrap();
    fs::write(vault.path().join("sessions/b/_meta.json"), "broken").unwrap();
    fs::write(vault.path().join("sessions/a/notes.md"), "updated").unwrap();
    assert_eq!(cache.reconcile_sessions(&["a".into()]).unwrap().updated, 1);
    assert_eq!(cache.search(query("updated", 10)).unwrap().count, 1);
    assert!(cache.search_fresh(query("updated", 10)).is_err());
    fs::remove_dir_all(vault.path().join("sessions/a")).unwrap();
    assert_eq!(cache.reconcile_sessions(&["a".into()]).unwrap().deleted, 1);
    assert_eq!(cache.search(query("updated", 10)).unwrap().count, 0);
}

#[test]
fn init_repairs_damaged_segment_contents_even_when_the_manifest_is_unchanged() {
    let (_global, vault, cache) = fixture();
    session(vault.path(), "a", "Title", "content", &[]);
    cache.initialize(&mut |_| {}).unwrap();
    let (index, _) = cache.open().unwrap();
    let generation = cache
        .path()
        .join("generations")
        .join(fs::read_to_string(cache.path().join("CURRENT")).unwrap());
    let path = fs::read_dir(&generation)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "store")
        })
        .unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert!(!index.validate_checksum().unwrap().is_empty());
    drop(index);
    cache.initialize(&mut |_| {}).unwrap();
    assert_eq!(cache.search_fresh(query("content", 10)).unwrap().count, 1);
}
