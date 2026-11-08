use super::*;

fn meta(id: &str, date: &str, title: &str, tags: &[&str]) -> SessionMeta {
    serde_json::from_value(serde_json::json!({"id":id,"title":title,"created_at":date,"tags":tags,"folder":"raw/folder","author":" Raw Author ","future_metadata":{"secret":"PRIVATE"}})).unwrap()
}
fn query(view: TimelineView, limit: u32) -> TimelineQuery {
    TimelineQuery {
        view,
        limit,
        cursor: None,
        expected_revision: None,
    }
}
fn store() -> (tempfile::TempDir, SessionStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().into());
    (dir, store)
}

#[test]
fn pages_over_400_identical_dates_preserve_title_utf16_and_legacy_stable_ties() {
    let (_dir, store) = store();
    for i in (0..451).rev() {
        store.index_upsert_meta(&meta(
            &format!("id-{i:04}"),
            "2026-11-08T12:00:00Z",
            "same",
            &[],
        ));
    }
    let mut input = query(
        TimelineView::Date {
            before_ms: TIMELINE_DATE_LIMIT,
        },
        200,
    );
    let mut ids = Vec::new();
    let mut sizes = Vec::new();
    loop {
        let page = store.indexed_timeline_headers(&input).unwrap();
        assert_eq!(page.total_count, 451);
        sizes.push(page.items.len());
        ids.extend(page.items.into_iter().map(|row| row.header.id));
        match page.next_cursor {
            Some(cursor) => input.cursor = Some(cursor),
            None => break,
        }
    }
    assert_eq!(sizes, vec![200, 200, 51]);
    assert_eq!(
        ids,
        (0..451).map(|i| format!("id-{i:04}")).collect::<Vec<_>>()
    );
    store.index_upsert_meta(&meta(
        "supplementary",
        "2026-11-08T12:00:00Z",
        "\u{10000}",
        &[],
    ));
    store.index_upsert_meta(&meta("bmp", "2026-11-08T12:00:00Z", "\u{e000}", &[]));
    let last = store
        .indexed_timeline_headers(&query(
            TimelineView::Date {
                before_ms: TIMELINE_DATE_LIMIT,
            },
            200,
        ))
        .unwrap();
    assert!(last.items.iter().all(|row| row.header.title == "same"));
    let a = meta("a", "x", "\u{10000}", &[]);
    let b = meta("b", "x", "\u{e000}", &[]);
    assert!(
        Candidate {
            meta: &a,
            timestamp: None,
            oldest_first: false
        } < Candidate {
            meta: &b,
            timestamp: None,
            oldest_first: false
        }
    );
}

#[test]
fn offsets_date_only_pre_epoch_undated_and_client_cutoff_are_explicit() {
    let (_dir, store) = store();
    for (id, date) in [
        ("a", "2024-03-31T03:00:00+02:00"),
        ("b", "2024-03-31T01:00:00Z"),
        ("future", "2024-04-01"),
        ("old", "1969-12-31T23:59:59Z"),
        ("invalid", "not a date"),
        ("naive", "2024-03-31T01:00:00"),
        ("locale", "03/31/2024"),
        ("empty", ""),
    ] {
        store.index_upsert_meta(&meta(id, date, "same", &[]));
    }
    let cutoff = timestamp("2024-04-01T00:00:00Z").unwrap();
    let dated = store
        .indexed_timeline_headers(&query(TimelineView::Date { before_ms: cutoff }, 200))
        .unwrap();
    assert_eq!(
        dated
            .items
            .iter()
            .map(|r| r.header.id.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a", "old"]
    );
    assert_eq!(dated.future_count, 1);
    assert_eq!(dated.undated_count, 4);
    assert_eq!(dated.items[0].timestamp_ms, dated.items[1].timestamp_ms);
    let undated = store
        .indexed_timeline_headers(&query(TimelineView::Undated {}, 200))
        .unwrap();
    assert_eq!(undated.total_count, 4);
    let all = store
        .indexed_timeline_headers(&query(TimelineView::Untagged {}, 200))
        .unwrap();
    assert_eq!(all.total_count, 8);
    assert_eq!(all.items.last().unwrap().header.id, "old");
    assert_eq!(timestamp("2024-03-31"), timestamp("2024-03-31T00:00:00Z"));
    assert!(timestamp("2016-12-31T23:59:60Z").is_none());
}

#[test]
fn complete_hierarchical_facets_dedupe_subtrees_preserve_raw_labels_and_direct_pages() {
    let (_dir, store) = store();
    for (id, tags) in [
        ("one", vec!["a", "/a//b/", "a/b", "a/c"]),
        ("two", vec!["a/b"]),
        ("three", vec!["Untagged"]),
        ("four", vec!["///"]),
        ("five", vec![" A / b "]),
    ] {
        store.index_upsert_meta(&meta(id, "invalid", id, &tags));
    }
    let facets = store
        .indexed_timeline_facets(&TimelineFacetsQuery {
            expected_revision: None,
        })
        .unwrap();
    let a = facets.nodes.iter().find(|n| n.path == "a").unwrap();
    assert_eq!((a.direct_count, a.subtree_count), (1, 2));
    let b = facets.nodes.iter().find(|n| n.path == "a/b").unwrap();
    assert_eq!((b.direct_count, b.subtree_count), (2, 2));
    assert_eq!(b.parent_path.as_deref(), Some("a"));
    assert_eq!(facets.untagged_count, 1);
    assert!(facets.nodes.iter().any(|n| n.label == " b "));
    assert_eq!(
        store
            .indexed_timeline_headers(&query(TimelineView::Tag { path: "a".into() }, 200))
            .unwrap()
            .total_count,
        1
    );
    let named = store
        .indexed_timeline_headers(&query(
            TimelineView::Tag {
                path: "Untagged".into(),
            },
            200,
        ))
        .unwrap();
    assert_eq!(named.items[0].header.id, "three");
    let untagged = store
        .indexed_timeline_headers(&query(TimelineView::Untagged {}, 200))
        .unwrap();
    assert_eq!(untagged.items[0].header.id, "four");
    assert_eq!(named.items[0].header.created_at, "invalid");
}

#[test]
fn cursors_bind_view_limit_cutoff_and_projection_revision_while_body_edits_are_stable() {
    let (_dir, store) = store();
    for id in ["a", "b", "c"] {
        store.index_upsert_meta(&meta(id, "2026-01-01", "same", &[]));
    }
    let mut input = query(TimelineView::Untagged {}, 1);
    let first = store.indexed_timeline_headers(&input).unwrap();
    input.cursor = first.next_cursor.clone();
    store.index_set_note("a", Some("body changed".into()));
    assert_eq!(
        store.indexed_timeline_headers(&input).unwrap().revision,
        first.revision
    );
    for view in [
        TimelineView::Undated {},
        TimelineView::Date { before_ms: 0 },
        TimelineView::Tag { path: "a".into() },
    ] {
        let mut other = input.clone();
        other.view = view;
        assert!(matches!(
            store.indexed_timeline_headers(&other),
            Err(StoreError::ResourceLimit(_))
        ));
    }
    let mut other = input.clone();
    other.limit = 2;
    assert!(store.indexed_timeline_headers(&other).is_err());
    store.index_upsert_meta(&meta("a", "2026-01-01", "new title", &[]));
    assert!(matches!(
        store.indexed_timeline_headers(&input),
        Err(StoreError::Conflict(_))
    ));
    input.cursor = None;
    input.expected_revision = Some(first.revision.clone());
    assert!(matches!(
        store.indexed_timeline_headers(&input),
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store.indexed_timeline_facets(&TimelineFacetsQuery {
            expected_revision: Some(first.revision)
        }),
        Err(StoreError::Conflict(_))
    ));
}

#[tokio::test]
async fn rebuild_only_invalidates_when_projected_headers_change_and_reads_never_touch_canonical_files()
 {
    let (dir, store) = store();
    let mut original = meta("one", "2026-01-01", "title", &[]);
    store
        .create_note_session(&original, "PRIVATE BODY")
        .await
        .unwrap();
    let input = query(TimelineView::Untagged {}, 200);
    let first = store.indexed_timeline_headers(&input).unwrap();
    store.rebuild_index().await.unwrap();
    assert_eq!(
        store.indexed_timeline_headers(&input).unwrap().revision,
        first.revision
    );
    let file = dir.path().join("sessions/one/_meta.json");
    original.author = Some("new author".into());
    std::fs::write(&file, serde_json::to_vec(&original).unwrap()).unwrap();
    store.refresh_session("one").await.unwrap();
    assert_ne!(
        store.indexed_timeline_headers(&input).unwrap().revision,
        first.revision
    );
    let stable = store.indexed_timeline_headers(&input).unwrap().revision;
    std::fs::write(&file, b"{invalid metadata").unwrap();
    assert!(store.refresh_session("one").await.is_err());
    assert_eq!(
        store.indexed_timeline_headers(&input).unwrap().revision,
        stable
    );
    std::fs::remove_dir_all(dir.path().join("sessions")).unwrap();
    let page = store.indexed_timeline_headers(&input).unwrap();
    assert_eq!(page.items.len(), 1);
    let wire = serde_json::to_string(&page).unwrap();
    assert!(!wire.contains("PRIVATE"));
    assert!(!wire.contains("future_metadata"));
    assert!(!wire.contains(&dir.path().to_string_lossy().to_string()));
    assert!(
        store
            .indexed_timeline_facets(&TimelineFacetsQuery {
                expected_revision: None
            })
            .is_ok()
    );
}

#[test]
fn complete_facets_and_actual_serialized_pages_fail_explicitly_at_bounds() {
    let (_dir, store) = store();
    for i in 0..=TIMELINE_FACET_LIMIT {
        store.index_upsert_meta(&meta(
            &format!("id{i}"),
            "2026-01-01",
            "",
            &[&format!("tag{i}")],
        ));
    }
    assert!(matches!(
        store.indexed_timeline_facets(&TimelineFacetsQuery {
            expected_revision: None
        }),
        Err(StoreError::ResourceLimit(_))
    ));
    let (_dir, store) = self::store();
    store.index_upsert_meta(&meta("one", "2026-01-01", &"\u{0001}".repeat(200_000), &[]));
    assert!(matches!(
        store.indexed_timeline_headers(&query(TimelineView::Untagged {}, 200)),
        Err(StoreError::ResourceLimit(_))
    ));
}

#[test]
fn all_header_projection_changes_invalidate_and_noop_updates_stay_stable() {
    let (_dir, store) = store();
    let mut current = meta("one", "2026-01-01", "title", &[]);
    store.index_upsert_meta(&current);
    let input = query(TimelineView::Untagged {}, 200);
    let mut rev = store.indexed_timeline_headers(&input).unwrap().revision;
    for field in 0..5 {
        match field {
            0 => current.title = "changed".into(),
            1 => current.created_at = "2026-01-02".into(),
            2 => current.folder = Some("new".into()),
            3 => current.author = Some("person".into()),
            _ => current.tags = vec!["tag".into()],
        };
        store.index_upsert_meta(&current);
        let next = store
            .indexed_timeline_facets(&TimelineFacetsQuery {
                expected_revision: None,
            })
            .unwrap()
            .revision;
        assert_ne!(rev, next);
        rev = next;
    }
    store.index_upsert_meta(&current);
    assert_eq!(
        store
            .indexed_timeline_facets(&TimelineFacetsQuery {
                expected_revision: None
            })
            .unwrap()
            .revision,
        rev
    );
    let summary = crate::index::TranscriptSummary {
        metadata: crate::index::SessionTranscriptMetadata::default(),
        transcript_ids: vec!["transcript".into()],
        has_words: true,
        word_count: 1,
        content_hash: 0,
    };
    store.index_set_transcript_summary("one", summary.clone());
    let next = store
        .indexed_timeline_facets(&TimelineFacetsQuery {
            expected_revision: None,
        })
        .unwrap()
        .revision;
    assert_ne!(next, rev);
    rev = next;
    store.index_set_transcript_summary("one", summary);
    assert_eq!(
        store
            .indexed_timeline_facets(&TimelineFacetsQuery {
                expected_revision: None
            })
            .unwrap()
            .revision,
        rev
    );
    store.index_remove_session("one");
    assert_ne!(
        store
            .indexed_timeline_facets(&TimelineFacetsQuery {
                expected_revision: None
            })
            .unwrap()
            .revision,
        rev
    );
}

#[test]
fn upcoming_pages_find_nearest_among_hundreds_with_exclusive_now_and_cutoff() {
    let (_dir, store) = store();
    let now = timestamp("2026-01-01T12:00:00Z").unwrap();
    for i in 0..451 {
        let date = chrono::DateTime::from_timestamp_millis(now + 300_001 + i)
            .unwrap()
            .to_rfc3339();
        store.index_upsert_meta(&meta(&format!("distant{i:03}"), &date, "same", &[]));
    }
    for (id, offset) in [("now", 0), ("nearest", 1), ("exact-five-minutes", 300_000)] {
        let date = chrono::DateTime::from_timestamp_millis(now + offset)
            .unwrap()
            .to_rfc3339();
        store.index_upsert_meta(&meta(id, &date, "same", &[]));
    }
    let mut input = query(
        TimelineView::Upcoming {
            after_ms: now,
            before_ms: Some(now + 300_001),
        },
        1,
    );
    let first = store.indexed_timeline_headers(&input).unwrap();
    assert_eq!(first.total_count, 2);
    assert_eq!(first.items[0].header.id, "nearest");
    assert_eq!(first.future_count, 0);
    input.cursor = first.next_cursor;
    let second = store.indexed_timeline_headers(&input).unwrap();
    assert_eq!(second.items[0].header.id, "exact-five-minutes");
    assert!(second.next_cursor.is_none());
    let mut all = query(
        TimelineView::Upcoming {
            after_ms: now,
            before_ms: None,
        },
        200,
    );
    let mut ids = Vec::new();
    loop {
        let page = store.indexed_timeline_headers(&all).unwrap();
        assert_eq!(page.total_count, 453);
        ids.extend(page.items.into_iter().map(|row| row.header.id));
        match page.next_cursor {
            Some(cursor) => all.cursor = Some(cursor),
            None => break,
        }
    }
    assert_eq!(ids.len(), 453);
    assert_eq!(ids[0], "nearest");
    assert_eq!(ids[452], "distant450");
    let past = store
        .indexed_timeline_headers(&query(TimelineView::Date { before_ms: now + 1 }, 200))
        .unwrap();
    assert_eq!(past.items.len(), 1);
    assert_eq!(past.items[0].header.id, "now");
    assert_eq!(past.future_count, 453);
    input.cursor = None;
    input.view = TimelineView::Upcoming {
        after_ms: now,
        before_ms: Some(now + 300_000),
    };
    assert_eq!(
        store.indexed_timeline_headers(&input).unwrap().total_count,
        1
    );
}

#[test]
fn empty_tag_entries_consume_scan_and_projection_admission_without_normalizing_raw_values() {
    let item = meta("one", "2026-01-01", "", &["", ""]);
    let mut budget = 64;
    assert!(matches!(
        tag_paths(&item, &mut budget),
        Err(StoreError::ResourceLimit(_))
    ));
    let mut budget = 128;
    assert!(tag_paths(&item, &mut budget).unwrap().is_empty());
    let (_dir, store) = store();
    let mut item = item;
    item.tags = vec![String::new(); TIMELINE_RESPONSE_LIMIT / 8 + 1];
    store.index_upsert_meta(&item);
    assert!(matches!(
        store.indexed_timeline_headers(&query(TimelineView::Untagged {}, 200)),
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(store.session_list_headers()[0].tags, item.tags);
}
