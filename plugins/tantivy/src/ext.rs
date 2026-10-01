use std::collections::BTreeSet;

use tantivy::collector::{Count, TopDocs};
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, PhraseQuery, Query, TermQuery,
};
use tantivy::schema::{Facet, Field, IndexRecordOption};
use tantivy::snippet::SnippetGenerator;
use tantivy::tokenizer::TextAnalyzer;
use tantivy::{DateTime, Index, ReloadPolicy, Searcher, TantivyDocument, Term};
use tauri_plugin_settings::SettingsPluginExt;

use crate::query::build_created_at_range_query;
use crate::schema::{extract_search_document, get_fields};
use crate::tokenizer::register_tokenizers;
use crate::{
    CollectionConfig, CollectionIndex, HighlightRange, IndexState, SearchDocument, SearchHit,
    SearchRequest, SearchResult, Snippet,
};

pub fn detect_language(text: &str) -> hypr_language::Language {
    hypr_language::detect(text)
}

fn parse_query_parts(query: &str) -> (Vec<&str>, Vec<&str>) {
    let mut phrases = Vec::new();
    let mut regular_terms = Vec::new();
    let mut in_quote = false;
    let mut quote_start = 0;
    let mut current_start = 0;

    let chars: Vec<char> = query.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '"' {
            if in_quote {
                let phrase = &query[quote_start..i];
                if !phrase.trim().is_empty() {
                    phrases.push(phrase.trim());
                }
                in_quote = false;
                current_start = i + 1;
            } else {
                let before = &query[current_start..i];
                for term in before.split_whitespace() {
                    if !term.is_empty() {
                        regular_terms.push(term);
                    }
                }
                in_quote = true;
                quote_start = i + 1;
            }
        }
        i += 1;
    }

    if in_quote {
        let phrase = &query[quote_start..];
        if !phrase.trim().is_empty() {
            phrases.push(phrase.trim());
        }
    } else {
        let remaining = &query[current_start..];
        for term in remaining.split_whitespace() {
            if !term.is_empty() {
                regular_terms.push(term);
            }
        }
    }

    (phrases, regular_terms)
}

// Title boost factor (3x) to match Orama's title:3, content:1 behavior
const TITLE_BOOST: f32 = 3.0;
const MAX_PREFIX_EXPANSIONS: usize = 64;

fn analyze_tokens(analyzer: &mut TextAnalyzer, text: &str) -> Vec<String> {
    let mut stream = analyzer.token_stream(text);
    let mut tokens = Vec::new();
    while let Some(token) = stream.next() {
        tokens.push(token.text.clone());
    }
    tokens
}

/// Expand a typed word-prefix into the concrete terms present in the index for
/// `field`. Expanding to real `TermQuery`s (instead of an automaton-based prefix
/// query) lets the snippet generator see the matched whole words, so they get
/// highlighted in results.
fn expand_prefix_terms(searcher: &Searcher, field: Field, prefix: &str) -> Vec<String> {
    let mut expanded = BTreeSet::new();
    'segments: for segment in searcher.segment_readers() {
        let Ok(inverted) = segment.inverted_index(field) else {
            continue;
        };
        let Ok(mut stream) = inverted.terms().range().ge(prefix.as_bytes()).into_stream() else {
            continue;
        };
        while stream.advance() {
            if !stream.key().starts_with(prefix.as_bytes()) {
                break;
            }
            if let Ok(term) = std::str::from_utf8(stream.key()) {
                expanded.insert(term.to_string());
            }
            if expanded.len() >= MAX_PREFIX_EXPANSIONS {
                break 'segments;
            }
        }
    }
    expanded.into_iter().collect()
}

fn text_term_query(field: Field, text: &str) -> Box<dyn Query> {
    Box::new(TermQuery::new(
        Term::from_field_text(field, text),
        IndexRecordOption::WithFreqs,
    ))
}

fn union_query(mut queries: Vec<Box<dyn Query>>) -> Box<dyn Query> {
    if queries.len() == 1 {
        queries.pop().unwrap()
    } else {
        Box::new(BooleanQuery::union(queries))
    }
}

/// Match in title (boosted) or content.
fn either_field_clause(
    title_query: Box<dyn Query>,
    content_query: Box<dyn Query>,
) -> Box<dyn Query> {
    Box::new(BooleanQuery::new(vec![
        (
            Occur::Should,
            Box::new(BoostQuery::new(title_query, TITLE_BOOST)) as Box<dyn Query>,
        ),
        (Occur::Should, content_query),
    ]))
}

pub struct Tantivy<'a, R: tauri::Runtime, M: tauri::Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> Tantivy<'a, R, M> {
    pub async fn register_collection(&self, config: CollectionConfig) -> Result<(), crate::Error> {
        // The search index is a rebuildable cache, not user data: it lives in the
        // OS app-data dir (global_base), not the vault, so vault sync backends
        // (e.g. Google Drive) never see it churn.
        let global_base = self.manager.app_handle().settings().global_base()?;
        let index_path = global_base.join(&config.path).into_std_path_buf();
        let version_path = index_path.join("schema_version");

        std::fs::create_dir_all(&index_path)?;

        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        if guard.collections.contains_key(&config.name) {
            tracing::debug!("Collection '{}' already registered", config.name);
            return Ok(());
        }

        let schema = (config.schema_builder)();

        let needs_reindex = if index_path.join("meta.json").exists() {
            let stored_version = std::fs::read_to_string(&version_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(0);
            stored_version != config.schema_version
        } else {
            false
        };

        let index = if index_path.join("meta.json").exists() && !needs_reindex {
            Index::open_in_dir(&index_path)?
        } else {
            if needs_reindex {
                tracing::debug!(
                    "Schema version changed for collection '{}', re-creating index",
                    config.name
                );
                std::fs::remove_dir_all(&index_path)?;
                std::fs::create_dir_all(&index_path)?;
            }
            Index::create_in_dir(&index_path, schema.clone())?
        };

        std::fs::write(&version_path, config.schema_version.to_string())?;

        register_tokenizers(&index);

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let writer = index.writer(50_000_000)?;

        let collection_index = CollectionIndex {
            schema,
            index,
            reader,
            writer,
        };

        guard
            .collections
            .insert(config.name.clone(), collection_index);

        tracing::debug!(
            "Tantivy collection '{}' registered at {:?} (version: {})",
            config.name,
            index_path,
            config.schema_version
        );
        Ok(())
    }

    fn get_collection_name(collection: Option<String>) -> String {
        collection.unwrap_or_else(|| "default".to_string())
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResult, crate::Error> {
        let collection_name = Self::get_collection_name(request.collection);
        let state = self.manager.state::<IndexState>();
        let guard = state.inner.read().await;

        let collection_index = guard
            .collections
            .get(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let index = &collection_index.index;
        let reader = &collection_index.reader;

        let fields = get_fields(schema);
        let searcher = reader.searcher();

        let use_fuzzy = request.options.fuzzy.unwrap_or(false);
        let phrase_slop = request.options.phrase_slop.unwrap_or(0);
        let has_query = !request.query.trim().is_empty();

        let mut combined_query: Box<dyn Query> = if !has_query {
            Box::new(AllQuery)
        } else {
            // Query terms must go through the same analyzer as the indexed text
            // (lowercase + ascii folding), or cased/accented queries match nothing.
            // Title and content share one tokenizer, so analyzing once suffices.
            let mut analyzer = index.tokenizer_for_field(fields.content)?;
            let (phrases, regular_terms) = parse_query_parts(&request.query);

            let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

            // Quoted phrases: exact word sequence in title or content.
            for phrase in &phrases {
                let words = analyze_tokens(&mut analyzer, phrase);
                match words.as_slice() {
                    [] => {}
                    [word] => clauses.push((
                        Occur::Must,
                        either_field_clause(
                            text_term_query(fields.title, word),
                            text_term_query(fields.content, word),
                        ),
                    )),
                    words => {
                        let phrase_query = |field| {
                            let terms = words
                                .iter()
                                .map(|word| Term::from_field_text(field, word))
                                .collect();
                            let mut query = PhraseQuery::new(terms);
                            query.set_slop(phrase_slop);
                            Box::new(query) as Box<dyn Query>
                        };
                        clauses.push((
                            Occur::Must,
                            either_field_clause(
                                phrase_query(fields.title),
                                phrase_query(fields.content),
                            ),
                        ));
                    }
                }
            }

            let tokens: Vec<String> = regular_terms
                .iter()
                .flat_map(|term| analyze_tokens(&mut analyzer, term))
                .collect();
            // The trailing term is likely still being typed unless the query ends
            // with whitespace or a closing quote: also match it as a word prefix.
            let last_token_is_prefix = !request
                .query
                .ends_with(|c: char| c.is_whitespace() || c == '"');

            for (i, token) in tokens.iter().enumerate() {
                let is_prefix = last_token_is_prefix && i + 1 == tokens.len();
                let mut title_variants = vec![text_term_query(fields.title, token)];
                let mut content_variants = vec![text_term_query(fields.content, token)];

                if is_prefix {
                    for expanded in expand_prefix_terms(&searcher, fields.title, token) {
                        if expanded != *token {
                            title_variants.push(text_term_query(fields.title, &expanded));
                        }
                    }
                    for expanded in expand_prefix_terms(&searcher, fields.content, token) {
                        if expanded != *token {
                            content_variants.push(text_term_query(fields.content, &expanded));
                        }
                    }
                }
                if use_fuzzy {
                    let distance = request.options.distance.unwrap_or(1);
                    title_variants.push(Box::new(FuzzyTermQuery::new(
                        Term::from_field_text(fields.title, token),
                        distance,
                        true,
                    )));
                    content_variants.push(Box::new(FuzzyTermQuery::new(
                        Term::from_field_text(fields.content, token),
                        distance,
                        true,
                    )));
                }

                // Every term must appear (in either field) for a document to match.
                clauses.push((
                    Occur::Must,
                    either_field_clause(union_query(title_variants), union_query(content_variants)),
                ));
            }

            if clauses.is_empty() {
                Box::new(AllQuery)
            } else {
                Box::new(BooleanQuery::new(clauses))
            }
        };

        // Apply created_at filter
        if let Some(ref created_at_filter) = request.filters.created_at {
            let range_query = build_created_at_range_query(fields.created_at, created_at_filter);
            if let Some(rq) = range_query {
                combined_query = Box::new(BooleanQuery::new(vec![
                    (Occur::Must, combined_query),
                    (Occur::Must, rq),
                ]));
            }
        }

        // Apply doc_type filter
        if let Some(ref doc_type) = request.filters.doc_type {
            let doc_type_term = Term::from_field_text(fields.doc_type, doc_type);
            let doc_type_query = TermQuery::new(doc_type_term, IndexRecordOption::Basic);
            combined_query = Box::new(BooleanQuery::new(vec![
                (Occur::Must, combined_query),
                (Occur::Must, Box::new(doc_type_query)),
            ]));
        }

        // Apply facet filter
        if let Some(ref facet_path) = request.filters.facet
            && let Ok(facet) = Facet::from_text(facet_path)
        {
            let facet_term = Term::from_facet(fields.facets, &facet);
            let facet_query = TermQuery::new(facet_term, IndexRecordOption::Basic);
            combined_query = Box::new(BooleanQuery::new(vec![
                (Occur::Must, combined_query),
                (Occur::Must, Box::new(facet_query)),
            ]));
        }

        // Use tuple collector to get both top docs and total count
        let (top_docs, count) = searcher.search(
            &combined_query,
            &(TopDocs::with_limit(request.limit), Count),
        )?;

        let generate_snippets = request.options.snippets.unwrap_or(false);
        let snippet_max_chars = request.options.snippet_max_chars.unwrap_or(150);

        let (title_snippet_gen, content_snippet_gen) = if generate_snippets {
            let mut title_gen =
                SnippetGenerator::create(&searcher, &*combined_query, fields.title)?;
            title_gen.set_max_num_chars(snippet_max_chars);

            let mut content_gen =
                SnippetGenerator::create(&searcher, &*combined_query, fields.content)?;
            content_gen.set_max_num_chars(snippet_max_chars);

            (Some(title_gen), Some(content_gen))
        } else {
            (None, None)
        };

        let mut hits = Vec::new();
        for (score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;

            if let Some(search_doc) = extract_search_document(schema, &fields, &retrieved_doc) {
                let title_snippet = title_snippet_gen.as_ref().map(|generator| {
                    let snippet = generator.snippet_from_doc(&retrieved_doc);
                    Snippet {
                        fragment: snippet.fragment().to_string(),
                        highlights: snippet
                            .highlighted()
                            .iter()
                            .map(|range| HighlightRange {
                                start: range.start,
                                end: range.end,
                            })
                            .collect(),
                    }
                });

                let content_snippet = content_snippet_gen.as_ref().map(|generator| {
                    let snippet = generator.snippet_from_doc(&retrieved_doc);
                    Snippet {
                        fragment: snippet.fragment().to_string(),
                        highlights: snippet
                            .highlighted()
                            .iter()
                            .map(|range| HighlightRange {
                                start: range.start,
                                end: range.end,
                            })
                            .collect(),
                    }
                });

                hits.push(SearchHit {
                    score,
                    document: search_doc,
                    title_snippet,
                    content_snippet,
                });
            }
        }

        Ok(SearchResult { hits, count })
    }

    pub async fn reindex(&self, collection: Option<String>) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;

        writer.delete_all_documents()?;

        let fields = get_fields(schema);

        writer.commit()?;

        tracing::debug!(
            "Reindex completed for collection '{}'. Index cleared and ready for new documents. Fields: {:?}",
            collection_name,
            fields.id
        );

        Ok(())
    }

    pub async fn add_document(
        &self,
        collection: Option<String>,
        document: SearchDocument,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let mut doc = TantivyDocument::new();
        doc.add_text(fields.id, &document.id);
        doc.add_text(fields.doc_type, &document.doc_type);
        doc.add_text(fields.language, document.language.as_deref().unwrap_or(""));
        doc.add_text(fields.title, &document.title);
        doc.add_text(fields.content, &document.content);
        doc.add_date(
            fields.created_at,
            DateTime::from_timestamp_millis(document.created_at),
        );

        for facet_path in &document.facets {
            if let Ok(facet) = Facet::from_text(facet_path) {
                doc.add_facet(fields.facets, facet);
            }
        }

        writer.add_document(doc)?;
        writer.commit()?;

        tracing::debug!(
            "Added document '{}' to collection '{}'",
            document.id,
            collection_name
        );

        Ok(())
    }

    pub async fn update_document(
        &self,
        collection: Option<String>,
        document: SearchDocument,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let id_term = Term::from_field_text(fields.id, &document.id);
        writer.delete_term(id_term);

        let mut doc = TantivyDocument::new();
        doc.add_text(fields.id, &document.id);
        doc.add_text(fields.doc_type, &document.doc_type);
        doc.add_text(fields.language, document.language.as_deref().unwrap_or(""));
        doc.add_text(fields.title, &document.title);
        doc.add_text(fields.content, &document.content);
        doc.add_date(
            fields.created_at,
            DateTime::from_timestamp_millis(document.created_at),
        );

        for facet_path in &document.facets {
            if let Ok(facet) = Facet::from_text(facet_path) {
                doc.add_facet(fields.facets, facet);
            }
        }

        writer.add_document(doc)?;
        writer.commit()?;

        tracing::debug!(
            "Updated document '{}' in collection '{}'",
            document.id,
            collection_name
        );

        Ok(())
    }

    pub async fn update_documents(
        &self,
        collection: Option<String>,
        documents: Vec<SearchDocument>,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let count = documents.len();

        for document in documents {
            let id_term = Term::from_field_text(fields.id, &document.id);
            writer.delete_term(id_term);

            let mut doc = TantivyDocument::new();
            doc.add_text(fields.id, &document.id);
            doc.add_text(fields.doc_type, &document.doc_type);
            doc.add_text(fields.language, document.language.as_deref().unwrap_or(""));
            doc.add_text(fields.title, &document.title);
            doc.add_text(fields.content, &document.content);
            doc.add_date(
                fields.created_at,
                DateTime::from_timestamp_millis(document.created_at),
            );

            for facet_path in &document.facets {
                if let Ok(facet) = Facet::from_text(facet_path) {
                    doc.add_facet(fields.facets, facet);
                }
            }

            writer.add_document(doc)?;
        }

        writer.commit()?;

        tracing::debug!(
            "Updated {} documents in collection '{}'",
            count,
            collection_name
        );

        Ok(())
    }

    pub async fn remove_document(
        &self,
        collection: Option<String>,
        id: String,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let id_term = Term::from_field_text(fields.id, &id);
        writer.delete_term(id_term);
        writer.commit()?;

        tracing::debug!(
            "Removed document '{}' from collection '{}'",
            id,
            collection_name
        );

        Ok(())
    }
}

pub trait TantivyPluginExt<R: tauri::Runtime> {
    fn tantivy(&self) -> Tantivy<'_, R, Self>
    where
        Self: tauri::Manager<R> + Sized;
}

impl<R: tauri::Runtime, T: tauri::Manager<R>> TantivyPluginExt<R> for T {
    fn tantivy(&self) -> Tantivy<'_, R, Self>
    where
        Self: Sized,
    {
        Tantivy {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::get_tokenizer_name_for_language;
    use crate::{CollectionIndex, IndexState, SearchDocument, SearchFilters, SearchRequest};
    use tauri::Manager;

    #[test]
    fn test_detect_language_tokenizer_integration() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let lang = detect_language(text);
        let tokenizer_name = get_tokenizer_name_for_language(&lang);
        assert_eq!(tokenizer_name, "lang_en");
    }

    async fn harness() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(IndexState::default());

        let schema = crate::build_schema();
        let index = tantivy::Index::create_in_ram(schema.clone());
        register_tokenizers(&index);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .unwrap();
        let writer = index.writer(50_000_000).unwrap();
        app.state::<IndexState>()
            .inner
            .write()
            .await
            .collections
            .insert(
                "default".to_string(),
                CollectionIndex {
                    schema,
                    index,
                    reader,
                    writer,
                },
            );
        app
    }

    fn doc(id: &str, title: &str, content: &str) -> SearchDocument {
        SearchDocument {
            id: id.to_string(),
            doc_type: "session".to_string(),
            language: None,
            title: title.to_string(),
            content: content.to_string(),
            created_at: 0,
            facets: Vec::new(),
        }
    }

    async fn search(
        app: &tauri::App<tauri::test::MockRuntime>,
        query: &str,
    ) -> crate::SearchResult {
        {
            let state = app.state::<IndexState>();
            let guard = state.inner.read().await;
            guard.collections["default"].reader.reload().unwrap();
        }
        app.tantivy()
            .search(SearchRequest {
                query: query.to_string(),
                collection: None,
                filters: SearchFilters::default(),
                limit: 10,
                options: crate::SearchOptions {
                    snippets: Some(true),
                    ..Default::default()
                },
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn matches_whole_words_not_letter_fragments() {
        let app = harness().await;
        app.tantivy()
            .add_document(
                None,
                doc(
                    "s1",
                    "M&A process",
                    "dat David zijn werk vastleggen uiteraard",
                ),
            )
            .await
            .unwrap();
        app.tantivy()
            .add_document(None, doc("s2", "tim debrief", "de mailkooning is geworden"))
            .await
            .unwrap();

        // The ngram-era regression: "david" matched every document containing
        // the letters d/a/v/i and highlighted 1-3 char fragments everywhere.
        let result = search(&app, "david").await;
        assert_eq!(result.count, 1);
        assert_eq!(result.hits[0].document.id, "s1");

        // Casing and accents normalize through the analyzer.
        assert_eq!(search(&app, "David").await.count, 1);

        // Subsequence/infix letters must not match.
        assert_eq!(search(&app, "avid").await.count, 0);
        assert_eq!(search(&app, "xyz").await.count, 0);

        // Multi-term queries require every word.
        assert_eq!(search(&app, "david vastleggen").await.count, 1);
        assert_eq!(search(&app, "david mailkooning").await.count, 0);
    }

    #[tokio::test]
    async fn trailing_term_matches_as_prefix_with_whole_word_highlight() {
        let app = harness().await;
        app.tantivy()
            .add_document(None, doc("s1", "debrief", "dat David zijn werk"))
            .await
            .unwrap();

        let result = search(&app, "dav").await;
        assert_eq!(result.count, 1, "search-as-you-type prefix must match");

        let snippet = result.hits[0].content_snippet.as_ref().unwrap();
        let highlighted: Vec<&str> = snippet
            .highlights
            .iter()
            .map(|range| &snippet.fragment[range.start..range.end])
            .collect();
        assert_eq!(
            highlighted,
            vec!["David"],
            "the whole matched word is highlighted, nothing else"
        );

        // A trailing space means the word is finished: no prefix matching.
        assert_eq!(search(&app, "dav ").await.count, 0);
    }
}
