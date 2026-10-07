use serde::{Deserialize, Serialize};

use super::{SessionStore, StoreError, WriteGuard, paths};

/// One tag, file-canonical in the vault-root `tags.json`. The id is the normalized
/// (lowercased) name itself — unlike people's lossy slug, two names normalizing
/// identically are by definition the same tag, so no collision suffixing is needed.
/// Sessions keep storing raw tag strings in `_meta.json` and degrade gracefully if
/// `tags.json` disappears.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TagItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct TagsFile {
    #[serde(default)]
    tags: Vec<TagItem>,
}

/// Trim, strip a leading `#`, lowercase. `None` when nothing is left — the strict
/// charset filter stays on the frontend; this is only what file-level dedupe needs.
fn normalize_tag_name(raw: &str) -> Option<String> {
    hypr_vault_read::normalize_tag_name(raw)
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct TagContext {
    pub available: Vec<String>,
    pub attached: Vec<String>,
    pub dismissed: Vec<String>,
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
pub struct ScoredTagSuggestion {
    pub name: String,
    pub confidence: f64,
}

fn normalized_tags(tags: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut tags: Vec<_> = tags
        .into_iter()
        .filter_map(|tag| normalize_tag_name(&tag))
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

fn is_suggestible_tag(name: &str) -> bool {
    static NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\A[\p{L}\p{N}_][\p{L}\p{N}_-]*(?:/[\p{L}\p{N}_][\p{L}\p{N}_-]*)*\z")
            .unwrap()
    });
    name.encode_utf16().count() <= 120 && !name.contains("import") && NAME.is_match(name)
}

pub(crate) fn apply_suggested_tags(
    meta: &mut super::SessionMeta,
    suggestions: Vec<ScoredTagSuggestion>,
    auto_apply_high_confidence_tags: bool,
) -> Vec<String> {
    let dismissed = normalized_tags(
        meta.tag_suggestions
            .as_ref()
            .map(|state| state.dismissed.clone())
            .unwrap_or_default(),
    );
    let attached = normalized_tags(meta.tags.clone());
    let mut candidates: Vec<(String, f64)> = Vec::new();
    for suggestion in suggestions {
        if !suggestion.confidence.is_finite() || !(0.0..=1.0).contains(&suggestion.confidence) {
            continue;
        }
        let Some(name) = normalize_tag_name(&suggestion.name) else {
            continue;
        };
        if !is_suggestible_tag(&name) || attached.contains(&name) || dismissed.contains(&name) {
            continue;
        }
        if let Some((_, confidence)) = candidates
            .iter_mut()
            .find(|(candidate, _)| *candidate == name)
        {
            *confidence = (*confidence).max(suggestion.confidence);
        } else if candidates.len() < 3 {
            candidates.push((name, suggestion.confidence));
        }
    }
    let mut items = Vec::new();
    let mut auto_applied = Vec::new();
    for (name, confidence) in candidates.into_iter().take(3) {
        if auto_apply_high_confidence_tags && confidence > 0.85 {
            auto_applied.push(name.clone());
            meta.tags.push(name);
        } else {
            items.push(name);
        }
    }
    if !auto_applied.is_empty() {
        meta.tags.sort();
    }
    meta.tag_suggestions = Some(super::TagSuggestionState { items, dismissed });
    auto_applied
}

impl SessionStore {
    pub async fn session_tag_context(&self, session_id: &str) -> Result<TagContext, StoreError> {
        let meta = self
            .read_meta(session_id)
            .await?
            .ok_or_else(|| StoreError::Io(format!("session {session_id} has no _meta.json")))?;
        let registry = self.list_tags().await?;
        let mut names: Vec<_> = registry.into_iter().map(|tag| tag.name).collect();
        {
            let index = self.index.read().unwrap();
            names.extend(
                index
                    .sessions
                    .values()
                    .flat_map(|entry| entry.meta.tags.iter().cloned()),
            );
        }
        names.extend(meta.tags.iter().cloned());
        Ok(TagContext {
            available: normalized_tags(names)
                .into_iter()
                .filter(|name| is_suggestible_tag(name))
                .collect(),
            attached: normalized_tags(meta.tags),
            dismissed: normalized_tags(
                meta.tag_suggestions
                    .map(|state| state.dismissed)
                    .unwrap_or_default(),
            ),
        })
    }

    /// A missing `tags.json` is an empty registry, and an unparseable one must never
    /// take the typeahead down with it — sessions keep their raw tag strings either way.
    async fn read_tags(&self) -> Result<Vec<TagItem>, StoreError> {
        let path = self.vault_base.join(paths::tags_path());
        tokio::task::spawn_blocking(move || {
            let raw = match std::fs::read(&path) {
                Ok(raw) => raw,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
                Err(e) => {
                    tracing::warn!("failed to read tags.json; treating as empty: {e}");
                    return Vec::new();
                }
            };
            match serde_json::from_slice::<TagsFile>(&raw) {
                Ok(file) => file.tags,
                Err(e) => {
                    tracing::warn!("failed to parse tags.json; treating as empty: {e}");
                    Vec::new()
                }
            }
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))
    }

    pub async fn list_tags(&self) -> Result<Vec<TagItem>, StoreError> {
        let mut tags = self.read_tags().await?;
        tags.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(tags)
    }

    /// Registry is append-only: an existing tag is returned untouched, otherwise it is
    /// created under its normalized name. The guard spans read and write: `tags.json`
    /// is whole-file rewritten, so two concurrent ensures without it could drop each
    /// other's entry.
    pub async fn ensure_tag(&self, name: &str) -> Result<TagItem, StoreError> {
        let guard = self.lock_writes().await;
        self.ensure_tag_locked(&guard, name).await
    }

    pub(crate) async fn ensure_tag_locked(
        &self,
        guard: &WriteGuard<'_>,
        name: &str,
    ) -> Result<TagItem, StoreError> {
        let Some(normalized) = normalize_tag_name(name) else {
            return Err(StoreError::Io("tag name cannot be empty".to_string()));
        };

        let mut tags = self.read_tags().await?;
        if let Some(existing) = tags.iter().find(|t| t.id == normalized) {
            return Ok(existing.clone());
        }

        let tag = TagItem {
            id: normalized.clone(),
            name: normalized,
        };
        tags.push(tag.clone());

        let bytes = serde_json::to_vec_pretty(&TagsFile { tags })
            .map_err(|e| StoreError::Serialize(e.to_string()))?;
        self.write_file_locked(guard, paths::tags_path(), bytes)
            .await?;

        self.index_upsert_tag(&tag);
        self.notify_index_changed(super::IndexEntity::Tags, vec![tag.id.clone()]);
        Ok(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(name: &str, confidence: f64) -> ScoredTagSuggestion {
        ScoredTagSuggestion {
            name: name.into(),
            confidence,
        }
    }

    #[tokio::test]
    async fn ensure_tag_creates_file_lazily_and_reuses_case_insensitively() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        assert!(!vault.path().join("tags.json").exists());

        let created = store.ensure_tag("Project-X").await.unwrap();
        assert_eq!(created.id, "project-x");
        assert_eq!(created.name, "project-x");
        assert!(vault.path().join("tags.json").exists());

        let mtime_after_create = std::fs::metadata(vault.path().join("tags.json"))
            .unwrap()
            .modified()
            .unwrap();

        let reused = store.ensure_tag("#PROJECT-x").await.unwrap();
        assert_eq!(reused, created);
        let mtime_after_reuse = std::fs::metadata(vault.path().join("tags.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(mtime_after_create, mtime_after_reuse);

        let listed = store.list_tags().await.unwrap();
        assert_eq!(listed, vec![created]);
    }

    #[tokio::test]
    async fn ensure_tag_rejects_empty_names() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        assert!(store.ensure_tag("   ").await.is_err());
        assert!(store.ensure_tag("#").await.is_err());
        assert!(!vault.path().join("tags.json").exists());
    }

    #[tokio::test]
    async fn concurrent_ensures_of_same_name_yield_one_tag() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        let (a, b) = tokio::join!(store.ensure_tag("Hiring"), store.ensure_tag("hiring"));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.id, b.id);

        let listed = store.list_tags().await.unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn unparseable_tags_file_is_treated_as_empty() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::write(vault.path().join("tags.json"), b"{not json").unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        assert_eq!(store.list_tags().await.unwrap(), vec![]);

        let created = store.ensure_tag("standup").await.unwrap();
        assert_eq!(created.id, "standup");
        assert_eq!(store.list_tags().await.unwrap(), vec![created]);
    }

    #[tokio::test]
    async fn list_tags_sorts_alphabetically() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());

        store.ensure_tag("zebra").await.unwrap();
        store.ensure_tag("alpha").await.unwrap();
        store.ensure_tag("hiring").await.unwrap();

        let names: Vec<String> = store
            .list_tags()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["alpha", "hiring", "zebra"]);
    }

    fn meta(id: &str, tags: &[&str]) -> super::super::SessionMeta {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": "Session", "started_at": null, "ended_at": null,
            "created_at": "2026-09-01T00:00:00Z", "tags": tags
        }))
        .unwrap()
    }

    async fn generate(
        store: &SessionStore,
        markdown: &str,
        expected: &str,
        tags: &[&str],
    ) -> Result<(), StoreError> {
        store
            .update_summary_with_suggestions(
                "s1",
                markdown,
                Some(expected),
                true,
                Some(tags.iter().map(|tag| scored(tag, 0.5)).collect()),
                false,
            )
            .await
    }

    #[tokio::test]
    async fn generated_tags_are_filtered_and_explicitly_resolved_across_regeneration() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        store
            .write_meta(&meta("s1", &[" #Attached ", "Imported"]))
            .await
            .unwrap();
        store.ensure_summary("s1").await.unwrap();
        generate(
            &store,
            "First",
            "",
            &[
                "attached", " #New ", "NEW", "imported", "Ignore", "Third", "Fourth",
            ],
        )
        .await
        .unwrap();
        let before = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(before.tags, vec![" #Attached ", "Imported"]);
        assert_eq!(
            before.tag_suggestions.unwrap().items,
            vec!["new", "ignore", "third"]
        );
        assert!(store.list_tags().await.unwrap().is_empty());
        assert!(store.accept_tag_suggestion("s1", "#NEW").await.unwrap());
        assert!(store.dismiss_tag_suggestion("s1", "Ignore").await.unwrap());
        assert!(!store.accept_tag_suggestion("s1", "missing").await.unwrap());
        assert_eq!(store.list_tags().await.unwrap()[0].name, "new");
        generate(
            &store,
            "Second",
            "First",
            &["new", "IGNORE", "Third", "Novel"],
        )
        .await
        .unwrap();
        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert!(after.tags.contains(&"new".to_string()));
        let state = after.tag_suggestions.unwrap();
        assert_eq!(state.items, vec!["third", "novel"]);
        assert_eq!(state.dismissed, vec!["ignore"]);
        store
            .update_generated_summary("s1", "Third", Some("Second"))
            .await
            .unwrap();
        assert_eq!(
            store
                .read_meta("s1")
                .await
                .unwrap()
                .unwrap()
                .tag_suggestions,
            Some(state)
        );
        generate(&store, "Fourth", "Third", &[]).await.unwrap();
        let state = store
            .read_meta("s1")
            .await
            .unwrap()
            .unwrap()
            .tag_suggestions
            .unwrap();
        assert!(state.items.is_empty());
        assert_eq!(state.dismissed, vec!["ignore"]);
    }

    #[tokio::test]
    async fn stale_and_deleted_summaries_cannot_change_suggestions() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        store.write_meta(&meta("s1", &[])).await.unwrap();
        store.ensure_summary("s1").await.unwrap();
        generate(&store, "Saved", "", &["keep"]).await.unwrap();
        for markdown in ["Stale", "Saved"] {
            assert!(matches!(
                generate(&store, markdown, "", &["wrong"]).await,
                Err(StoreError::Conflict(_))
            ));
        }
        store.delete_summary("s1").await.unwrap();
        assert!(generate(&store, "Late", "Saved", &["wrong"]).await.is_err());
        assert!(store.read_summary("s1").await.unwrap().is_none());
        assert_eq!(
            store
                .read_meta("s1")
                .await
                .unwrap()
                .unwrap()
                .tag_suggestions
                .unwrap()
                .items,
            vec!["keep"]
        );
        store.delete_session("s1").await.unwrap();
        assert!(generate(&store, "Late", "Saved", &["wrong"]).await.is_err());
        assert!(store.read_meta("s1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn scored_suggestions_respect_threshold_setting_and_prior_decisions() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        let mut session = meta("s1", &["Attached"]);
        session.tag_suggestions = Some(super::super::TagSuggestionState {
            items: vec![],
            dismissed: vec!["Dismissed".into()],
        });
        store.write_meta(&session).await.unwrap();
        store.ensure_summary("s1").await.unwrap();
        store
            .update_summary_with_suggestions(
                "s1",
                "First",
                Some(""),
                false,
                Some(vec![
                    scored("Attached", 0.99),
                    scored("Dismissed", 0.99),
                    scored("Boundary", 0.85),
                    scored("Auto", 0.5),
                    scored("Auto", 0.8501),
                    scored("Fourth", 0.5),
                    scored("Fifth", 0.99),
                ]),
                true,
            )
            .await
            .unwrap();
        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(after.tags, vec!["Attached", "auto"]);
        let state = after.tag_suggestions.unwrap();
        assert_eq!(state.items, vec!["boundary", "fourth"]);
        assert_eq!(state.dismissed, vec!["dismissed"]);
        assert_eq!(store.list_tags().await.unwrap()[0].name, "auto");

        store
            .update_summary_with_suggestions(
                "s1",
                "Second",
                Some("First"),
                false,
                Some(vec![scored("Disabled", 1.0)]),
                false,
            )
            .await
            .unwrap();
        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(after.tags, vec!["Attached", "auto"]);
        assert_eq!(after.tag_suggestions.unwrap().items, vec!["disabled"]);
        assert_eq!(store.list_tags().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn invalid_scores_and_stale_summary_do_not_attach_or_register_tags() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        store.write_meta(&meta("s1", &[])).await.unwrap();
        store.ensure_summary("s1").await.unwrap();
        store
            .update_summary_with_suggestions(
                "s1",
                "Saved",
                Some(""),
                false,
                Some(vec![
                    scored("nan", f64::NAN),
                    scored("infinite", f64::INFINITY),
                    scored("negative", -0.1),
                    scored("excess", 1.1),
                    scored("valid", 0.8501),
                ]),
                true,
            )
            .await
            .unwrap();
        let after = store.read_meta("s1").await.unwrap().unwrap();
        assert_eq!(after.tags, vec!["valid"]);
        assert!(after.tag_suggestions.unwrap().items.is_empty());
        assert_eq!(store.list_tags().await.unwrap().len(), 1);

        assert!(matches!(
            store
                .update_summary_with_suggestions(
                    "s1",
                    "Stale",
                    Some(""),
                    false,
                    Some(vec![scored("must-not-register", 0.99)]),
                    true,
                )
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            store.read_meta("s1").await.unwrap().unwrap().tags,
            vec!["valid"]
        );
        assert_eq!(store.list_tags().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tag_context_unions_registry_and_sessions_without_import_candidates() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        let mut session = meta("s1", &[" #Beta ", "Imported"]);
        session.tag_suggestions = Some(super::super::TagSuggestionState {
            items: vec![],
            dismissed: vec![" #Ignored ".into(), "ignored".into()],
        });
        store.write_meta(&session).await.unwrap();
        store
            .write_meta(&meta("s2", &["Alpha", "beta", "project/import-test"]))
            .await
            .unwrap();
        store.ensure_tag("Registry").await.unwrap();
        store.ensure_tag("Imported").await.unwrap();
        let context = store.session_tag_context("s1").await.unwrap();
        assert_eq!(context.available, vec!["alpha", "beta", "registry"]);
        assert_eq!(context.attached, vec!["beta", "imported"]);
        assert_eq!(context.dismissed, vec!["ignored"]);
    }

    #[tokio::test]
    async fn enhanced_doc_suggestions_follow_the_same_save_and_conflict_rules() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        store.write_meta(&meta("s1", &[])).await.unwrap();
        store
            .write_enhanced_doc(&super::super::EnhancedDoc {
                id: "doc".into(),
                session_id: "s1".into(),
                kind: "template_output".into(),
                title: "Doc".into(),
                template_id: String::new(),
                sort_order: 0,
                markdown: "Original".into(),
            })
            .await
            .unwrap();
        let patch = super::super::EnhancedDocPatch {
            markdown: Some("Generated".into()),
            expected_markdown: Some("Original".into()),
            suggested_tags: Some(vec![scored("Novel", 0.5)]),
            reconcile_tasks: Some(true),
            ..Default::default()
        };
        store
            .update_enhanced_doc("s1", "doc", patch.clone())
            .await
            .unwrap();
        assert!(matches!(
            store.update_enhanced_doc("s1", "doc", patch.clone()).await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            store
                .read_meta("s1")
                .await
                .unwrap()
                .unwrap()
                .tag_suggestions
                .unwrap()
                .items,
            vec!["novel"]
        );
        store.delete_enhanced_doc("s1", "doc").await.unwrap();
        assert!(store.update_enhanced_doc("s1", "doc", patch).await.is_err());
        assert_eq!(
            store
                .read_meta("s1")
                .await
                .unwrap()
                .unwrap()
                .tag_suggestions
                .unwrap()
                .items,
            vec!["novel"]
        );
    }

    #[tokio::test]
    async fn enhanced_doc_auto_applies_only_after_a_successful_guard() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_path_buf());
        store.write_meta(&meta("s1", &[])).await.unwrap();
        store
            .write_enhanced_doc(&super::super::EnhancedDoc {
                id: "doc".into(),
                session_id: "s1".into(),
                kind: "template_output".into(),
                title: "Doc".into(),
                template_id: String::new(),
                sort_order: 0,
                markdown: "Original".into(),
            })
            .await
            .unwrap();
        let patch = super::super::EnhancedDocPatch {
            markdown: Some("Updated".into()),
            expected_markdown: Some("Original".into()),
            suggested_tags: Some(vec![scored("enhanced", 0.99)]),
            ..Default::default()
        };
        store
            .update_enhanced_doc_with_auto_apply("s1", "doc", patch.clone(), true)
            .await
            .unwrap();
        assert!(matches!(
            store
                .update_enhanced_doc_with_auto_apply("s1", "doc", patch, true)
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            store.read_meta("s1").await.unwrap().unwrap().tags,
            vec!["enhanced"]
        );
        assert_eq!(store.list_tags().await.unwrap().len(), 1);
    }

    #[test]
    fn suggestions_reject_invalid_names_and_preserve_hierarchy() {
        let mut session = meta("s1", &[]);
        apply_suggested_tags(
            &mut session,
            [
                "two words",
                "a//b",
                "-bad",
                "punctuation!",
                "\u{0345}",
                " #Project/Étude-2 ",
                "project/étude-2",
                "_valid/123",
            ]
            .into_iter()
            .map(|name| scored(name, 0.5))
            .chain(std::iter::once(ScoredTagSuggestion {
                name: "x".repeat(121),
                confidence: 0.5,
            }))
            .collect(),
            false,
        );
        assert_eq!(
            session.tag_suggestions.unwrap().items,
            vec!["project/étude-2", "_valid/123"]
        );
    }
}
