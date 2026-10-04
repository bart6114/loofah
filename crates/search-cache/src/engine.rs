use std::collections::BTreeSet;

use crate::query::build_created_at_range_query;
use crate::schema::{extract_search_document, get_fields};
use crate::{HighlightRange, SearchHit, SearchRequest, SearchResult, Snippet};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, PhraseQuery, Query, TermQuery,
};
use tantivy::schema::{Facet, Field, IndexRecordOption};
use tantivy::snippet::SnippetGenerator;
use tantivy::tokenizer::TextAnalyzer;
use tantivy::{Index, Searcher, TantivyDocument, Term};
fn parse_query_parts(query: &str) -> (Vec<&str>, Vec<&str>) {
    let mut phrases = Vec::new();
    let mut regular_terms = Vec::new();
    let mut in_quote = false;
    let mut quote_start = 0;
    let mut current_start = 0;

    for (i, ch) in query.char_indices() {
        if ch == '"' {
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

pub fn search(
    index: &Index,
    reader: &tantivy::IndexReader,
    request: SearchRequest,
) -> tantivy::Result<SearchResult> {
    let schema = &index.schema();
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
        let mut title_gen = SnippetGenerator::create(&searcher, &*combined_query, fields.title)?;
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

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(b.document.created_at.cmp(&a.document.created_at))
            .then(a.document.id.cmp(&b.document.id))
    });
    Ok(SearchResult { hits, count })
}

/// Match source-specific CLI filters with the same analysis and query grammar as
/// the index. Offsets refer to original UTF-8 bytes for snippets and timestamps.
pub fn matching_range(text: &str, query: &str) -> Option<(usize, usize)> {
    let mut analyzer = crate::tokenizer::multilang_analyzer();
    let mut stream = analyzer.token_stream(text);
    let mut tokens = Vec::new();
    while let Some(token) = stream.next() {
        tokens.push(token.clone());
    }
    drop(stream);
    let (phrases, terms) = parse_query_parts(query);
    let mut first = None;
    for phrase in phrases {
        let wanted = analyze_tokens(&mut analyzer, phrase);
        if wanted.is_empty() {
            continue;
        }
        let found = tokens
            .windows(wanted.len())
            .find(|window| window.iter().zip(&wanted).all(|(a, b)| a.text == *b))?;
        first.get_or_insert((found[0].offset_from, found.last()?.offset_to));
    }
    let words: Vec<_> = terms
        .iter()
        .flat_map(|term| analyze_tokens(&mut analyzer, term))
        .collect();
    let prefix = !query.ends_with(|c: char| c.is_whitespace() || c == '"');
    for (i, word) in words.iter().enumerate() {
        let found = tokens.iter().find(|token| {
            token.text == *word || (prefix && i + 1 == words.len() && token.text.starts_with(word))
        })?;
        first.get_or_insert((found.offset_from, found.offset_to));
    }
    first
}
