use search_cache::{Cache, SearchRequest};
use std::path::{Path, PathBuf};

fn seed(vault: &Path, id: &str, title: &str, note: &str) {
    let dir = vault.join("sessions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("_meta.json"), serde_json::json!({ "id": id, "title": title, "created_at": "2026-01-01T00:00:00Z", "started_at": null, "ended_at": null, "tags": [] }).to_string()).unwrap();
    std::fs::write(dir.join("notes.md"), note).unwrap();
}
fn search(cache: &Cache, query: &str) -> Vec<String> {
    cache
        .search(SearchRequest {
            query: query.into(),
            collection: None,
            filters: Default::default(),
            limit: 100,
            options: Default::default(),
        })
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.document.id)
        .collect()
}
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dst.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
        }
    }
}
struct Replica {
    vault: PathBuf,
    local: PathBuf,
}

#[test]
fn every_desktop_cli_pair_bootstraps_and_converges_after_offline_edits() {
    let root = tempfile::tempdir().unwrap();
    let replicas: Vec<_> = ["desktop-a", "desktop-b", "cli-a", "cli-b"]
        .iter()
        .map(|name| {
            let vault = root.path().join(name).join("vault");
            std::fs::create_dir_all(&vault).unwrap();
            Replica {
                vault,
                local: root.path().join(name).join("local"),
            }
        })
        .collect();
    for producer in &replicas {
        seed(&producer.vault, "shared", "Planning", "original marker");
        let mut cache = Cache::open_at(&producer.vault, &producer.local).unwrap();
        cache.refresh(false).unwrap();
        cache.publish().unwrap();
        drop(cache);
        for consumer in &replicas {
            if producer.vault == consumer.vault {
                continue;
            }
            let local = tempfile::tempdir().unwrap();
            let vault = tempfile::tempdir().unwrap();
            // Snapshot arrives first. Search/list readiness must not read sources.
            copy_tree(
                &producer.vault.join(".loofah-cache"),
                &vault.path().join(".loofah-cache"),
            );
            let mut reader = Cache::open_at(vault.path(), local.path()).unwrap();
            assert!(reader.bootstrapped);
            assert_eq!(search(&reader, "marker"), ["shared"]);
            assert_eq!(reader.headers().len(), 1);
            assert_eq!(reader.maintain(8, false).unwrap().removed.len(), 0);
            seed(
                vault.path(),
                "shared",
                "Planning",
                "winning replica content",
            );
            seed(vault.path(), "independent", "Offline", "independent marker");
            reader.refresh(true).unwrap();
            assert_eq!(search(&reader, "winning"), ["shared"]);
            assert_eq!(search(&reader, "independent"), ["independent"]);
            // A foreign, older publication cannot replace a healthy local cache.
            drop(reader);
            let reader = Cache::open_at(vault.path(), local.path()).unwrap();
            assert_eq!(search(&reader, "winning"), ["shared"]);
        }
    }
}

#[test]
fn unchanged_verification_does_not_reindex_and_same_stat_edits_are_repaired() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    for id in ["a", "b", "c"] {
        seed(vault.path(), id, "Title", "alpha");
    }
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    assert!(cache.refresh(true).unwrap().updated.is_empty());
    let note = vault.path().join("sessions/b/notes.md");
    let before =
        filetime::FileTime::from_last_modification_time(&std::fs::metadata(&note).unwrap());
    std::fs::write(&note, "bravo").unwrap();
    filetime::set_file_mtime(&note, before).unwrap();
    drop(cache);
    for _ in 0..8 {
        let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
        let report = cache.maintain(1, false).unwrap();
        assert!(report.updated.iter().all(|id| id == "b"));
    }
    let cache = Cache::open_at(vault.path(), local.path()).unwrap();
    assert_eq!(search(&cache, "bravo"), ["b"]);
}

#[test]
fn corrupt_and_incomplete_snapshots_are_ignored_and_owned_retention_is_bounded() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "durable");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    let pack = cache.publish().unwrap();
    cache.publish().unwrap();
    cache.publish().unwrap();
    assert_eq!(
        std::fs::read_dir(pack.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|e| e == "pack"))
            .count(),
        2
    );
    let foreign = vault.path().join(".loofah-cache/search-v1/foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("z-corrupt.pack"), b"broken").unwrap();
    std::fs::write(foreign.join("interrupted.tmp"), b"unfinished").unwrap();
    cache.publish().unwrap();
    assert!(foreign.join("z-corrupt.pack").exists());
    let other = tempfile::tempdir().unwrap();
    let imported = Cache::open_at(vault.path(), other.path()).unwrap();
    assert!(imported.bootstrapped);
    assert_eq!(search(&imported, "durable"), ["a"]);
}

#[test]
fn temporary_invalid_sources_keep_entries_and_confirmed_deletion_removes_them() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "durable");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    std::fs::write(vault.path().join("sessions/a/_meta.json"), "{").unwrap();
    cache.mark("a").unwrap();
    assert!(cache.refresh(false).unwrap().pending > 0);
    assert_eq!(search(&cache, "durable"), ["a"]);
    drop(cache);
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    seed(vault.path(), "a", "Title", "recovered");
    cache.refresh(false).unwrap();
    assert_eq!(search(&cache, "recovered"), ["a"]);
    std::fs::remove_dir_all(vault.path().join("sessions/a")).unwrap();
    cache.refresh(false).unwrap();
    assert!(search(&cache, "recovered").is_empty());
}

#[test]
fn attachments_are_never_indexed_and_a_few_edits_only_replace_affected_sessions() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    for id in ["a", "b", "c"] {
        seed(vault.path(), id, "Title", "original");
    }
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    std::fs::write(vault.path().join("sessions/a/unknown.md"), "secret").unwrap();
    std::fs::create_dir_all(vault.path().join("sessions/a/attachments")).unwrap();
    std::fs::write(
        vault.path().join("sessions/a/attachments/notes.md"),
        "secret",
    )
    .unwrap();
    seed(vault.path(), "b", "Title", "changed");
    cache.mark("b").unwrap();
    let report = cache.refresh(false).unwrap();
    assert_eq!(report.updated, ["b"]);
    assert!(search(&cache, "secret").is_empty());
}

#[test]
fn unicode_phrase_queries_and_title_boost_are_shared() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Café planning", "notes");
    seed(vault.path(), "b", "Other", "café planning");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    assert_eq!(search(&cache, "café \"planning\"")[0], "a");
    assert_eq!(search(&cache, "caf").len(), 2);
}

#[test]
fn corrupt_local_generation_recovers_without_deleting_the_previous_generation() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "recovery");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    drop(cache);
    let previous = std::fs::read_to_string(local.path().join("active")).unwrap();
    std::fs::write(local.path().join(&previous).join("meta.json"), b"broken").unwrap();
    let cache = Cache::open_at(vault.path(), local.path()).unwrap();
    assert!(cache.bootstrapped);
    assert_eq!(search(&cache, "recovery"), ["a"]);
    assert!(local.path().join(previous).exists());
}

#[test]
fn foreground_reader_remains_usable_while_maintenance_holds_the_writer() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "responsive");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    let reader = cache.reader();
    let locked = std::sync::Mutex::new(cache);
    let _guard = locked.lock().unwrap();
    assert_eq!(
        reader
            .search(SearchRequest {
                query: "responsive".into(),
                collection: None,
                filters: Default::default(),
                limit: 10,
                options: Default::default()
            })
            .unwrap()
            .count,
        1
    );
}

#[test]
#[ignore = "performance measurement on a disposable 1000-session vault"]
fn measure_cache_startup_and_incremental_maintenance() {
    let vault = match std::env::var_os("LOOFAH_BENCHMARK_ROOT") {
        Some(root) => tempfile::Builder::new()
            .prefix("loofah-search-bench-")
            .tempdir_in(root)
            .unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    let local = tempfile::tempdir().unwrap();
    for n in 0..1000 {
        seed(
            vault.path(),
            &format!("session-{n:04}"),
            "Planning",
            &"transcript words ".repeat(300),
        );
    }
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    let build = cache.refresh(false).unwrap();
    let pack = cache.publish().unwrap();
    drop(cache);
    let started = std::time::Instant::now();
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    let ready = started.elapsed();
    assert_eq!(cache.headers().len(), 1000);
    let warm = cache.maintain(16, false).unwrap();
    let imported = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let imported_cache = Cache::open_at(vault.path(), imported.path()).unwrap();
    let import = started.elapsed();
    assert_eq!(imported_cache.headers().len(), 1000);
    let mut build = build;
    let build_updates = build.updated.len();
    build.updated.clear();
    eprintln!(
        "vault={}, cached readiness={ready:?}, import={import:?}, pack_bytes={}, build_updates={build_updates}, build={}, warm={}",
        vault.path().display(),
        std::fs::metadata(pack).unwrap().len(),
        serde_json::to_string(&build).unwrap(),
        serde_json::to_string(&warm).unwrap()
    );
    if std::env::var_os("LOOFAH_BENCHMARK_KEEP").is_some() {
        eprintln!("retained benchmark vault={}", vault.keep().display());
    }
}

#[test]
#[ignore = "requires LOOFAH_BENCHMARK_VAULT pointing at a disposable synced vault"]
fn measure_existing_cloud_vault() {
    let vault = PathBuf::from(std::env::var_os("LOOFAH_BENCHMARK_VAULT").unwrap());
    let local = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let mut cache = Cache::open_at(&vault, local.path()).unwrap();
    let import = started.elapsed();
    assert!(cache.bootstrapped);
    let sessions = cache.headers().len();
    for phase in ["first", "warm"] {
        let report = cache.refresh(true).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.updated.is_empty());
        eprintln!(
            "phase={phase} sessions={sessions} import={import:?} maintenance={}",
            serde_json::to_string(&report).unwrap()
        );
    }
    drop(cache);
    let started = std::time::Instant::now();
    let cache = Cache::open_at(&vault, local.path()).unwrap();
    eprintln!(
        "cached readiness={:?} headers={}",
        started.elapsed(),
        cache.headers().len()
    );
}

#[test]
fn four_independent_writers_converge_after_interleaved_delivery_and_missed_publications() {
    let root = tempfile::tempdir().unwrap();
    let mut replicas = Vec::new();
    for name in ["desktop-a", "desktop-b", "cli-a", "cli-b"] {
        let vault = root.path().join(name).join("vault");
        let local = root.path().join(name).join("cache");
        seed(&vault, "shared", "Planning", "original");
        seed(&vault, "deleted", "Obsolete", "obsolete");
        let mut cache = Cache::open_at(&vault, &local).unwrap();
        cache.refresh(false).unwrap();
        replicas.push((vault, local, cache));
    }
    // Each producer edits offline, including conflicting edits to the same file.
    for (n, (vault, _, cache)) in replicas.iter_mut().enumerate() {
        seed(vault, "shared", "Planning", &format!("conflict winner{n}"));
        seed(
            vault,
            &format!("independent-{n}"),
            "Independent",
            &format!("token{n}"),
        );
        cache.refresh(false).unwrap();
        if n % 2 == 0 {
            cache.publish().unwrap();
        }
    }
    // Deliver one stale publication before source files and another after them.
    let publications = root.path().join("publications");
    copy_tree(&replicas[0].0.join(".loofah-cache"), &publications);
    copy_tree(&publications, &replicas[1].0.join(".loofah-cache"));
    for (vault, _, cache) in &mut replicas {
        seed(vault, "shared", "Planning", "conflict winner3");
        for n in 0..4 {
            seed(
                vault,
                &format!("independent-{n}"),
                "Independent",
                &format!("token{n}"),
            );
        }
        std::fs::remove_dir_all(vault.join("sessions/deleted")).unwrap();
        for _ in 0..5 {
            cache.maintain(2, false).unwrap();
        }
    }
    copy_tree(&publications, &replicas[3].0.join(".loofah-cache"));
    for (vault, local, mut cache) in replicas {
        cache.refresh(true).unwrap();
        assert_eq!(search(&cache, "winner3"), ["shared"]);
        assert!(search(&cache, "obsolete").is_empty());
        assert_eq!(cache.headers().len(), 5);
        drop(cache);
        let cache = Cache::open_at(&vault, &local).unwrap();
        for n in 0..4 {
            assert_eq!(
                search(&cache, &format!("token{n}")),
                [format!("independent-{n}")]
            );
        }
    }
}

#[test]
fn lost_history_restarts_reconciliation_and_durable_repairs_survive_restarts() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "before");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    cache.mark("a").unwrap();
    drop(cache);
    let state_path = local.path().join("maintenance.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    state["event_cursor"] = serde_json::json!(u64::MAX);
    state["scan_cursor"] = serde_json::json!("zzzz");
    std::fs::write(state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    seed(vault.path(), "a", "Title", "repaired after restart");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    assert_eq!(search(&cache, "repaired"), ["a"]);
}

#[test]
fn incompatible_and_truncated_completed_packs_do_not_bootstrap() {
    use std::io::Read;
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "canonical");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    let pack = cache.publish().unwrap();
    let bytes = std::fs::read(pack).unwrap();
    drop(cache);
    let replica = tempfile::tempdir().unwrap();
    let directory = replica.path().join(".loofah-cache/search-v1/publisher");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("truncated.pack"), &bytes[..bytes.len() / 2]).unwrap();
    let mut input = tar::Archive::new(bytes.as_slice());
    let mut output =
        tar::Builder::new(std::fs::File::create(directory.join("incompatible.pack")).unwrap());
    for entry in input.entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().into_owned();
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).unwrap();
        if name == Path::new("manifest.json") {
            let mut value: serde_json::Value = serde_json::from_slice(&contents).unwrap();
            value["format"] = serde_json::json!(999);
            contents = serde_json::to_vec(&value).unwrap();
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        output
            .append_data(&mut header, name, contents.as_slice())
            .unwrap();
    }
    output.finish().unwrap();
    let other = tempfile::tempdir().unwrap();
    let cache = Cache::open_at(replica.path(), other.path()).unwrap();
    assert!(!cache.bootstrapped);
    assert!(cache.headers().is_empty());
}

#[test]
fn a_single_missing_membership_observation_keeps_cached_sessions() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "durable");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    let hidden = vault.path().join("in-transit");
    std::fs::rename(vault.path().join("sessions/a"), &hidden).unwrap();
    cache.mark("a").unwrap();
    assert!(cache.maintain(8, false).unwrap().removed.is_empty());
    assert_eq!(search(&cache, "durable"), ["a"]);
    std::fs::rename(hidden, vault.path().join("sessions/a")).unwrap();
    cache.refresh(false).unwrap();
    assert_eq!(search(&cache, "durable"), ["a"]);
}

#[test]
fn late_delete_events_and_never_indexed_sessions_do_not_leave_permanent_repairs() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "removed");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    std::fs::remove_dir_all(vault.path().join("sessions/a")).unwrap();
    cache.refresh(false).unwrap();
    assert!(cache.headers().is_empty());
    cache.mark("a").unwrap();
    cache.mark("never-indexed").unwrap();
    let report = cache.refresh(false).unwrap();
    assert!(report.removed.is_empty());
    assert_eq!(report.pending, 0);
}

#[test]
fn missing_local_segments_recover_from_a_snapshot_before_reading_sources() {
    let vault = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    seed(vault.path(), "a", "Title", "recovery");
    let mut cache = Cache::open_at(vault.path(), local.path()).unwrap();
    cache.refresh(false).unwrap();
    drop(cache);
    let active = std::fs::read_to_string(local.path().join("active")).unwrap();
    let segment = std::fs::read_dir(local.path().join(active))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "store"))
        .unwrap();
    std::fs::remove_file(segment).unwrap();
    std::fs::rename(vault.path().join("sessions"), vault.path().join("offline")).unwrap();
    let cache = Cache::open_at(vault.path(), local.path()).unwrap();
    assert!(cache.bootstrapped);
    assert_eq!(search(&cache, "recovery"), ["a"]);
}
