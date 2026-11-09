use std::{cmp::Ordering, collections::BTreeSet};

use crate::{SessionStore, StoreError};

const MAX_QUERY: usize = 4096;
const MAX_EXCLUDED: usize = 256;
const MAX_RESULT: usize = 64 * 1024;

#[derive(Eq, PartialEq)]
struct JsName<'a>(&'a str);

impl Ord for JsName<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.encode_utf16().cmp(other.0.encode_utf16())
    }
}

impl PartialOrd for JsName<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl SessionStore {
    pub async fn tag_suggestions(
        &self,
        query: &str,
        excluded: &[String],
        limit: usize,
    ) -> Result<Vec<String>, StoreError> {
        self.ensure_ready()?;
        if query.len() > MAX_QUERY || excluded.len() > MAX_EXCLUDED || !(1..=200).contains(&limit) {
            return Err(StoreError::ResourceLimit(
                "Invalid tag suggestion query".into(),
            ));
        }
        let query = query
            .strip_prefix('#')
            .unwrap_or(query)
            .trim_matches(js_whitespace)
            .to_lowercase();
        let excluded = excluded.to_vec();
        let registry = self.list_tags().await?;
        let store = self.clone();
        self.spawn_blocking(move || {
            store.ensure_ready()?;
            let index = store.index.read().unwrap();
            let candidates = registry.iter().map(|tag| tag.name.as_str()).chain(
                index
                    .sessions
                    .values()
                    .flat_map(|entry| entry.meta.tags.iter().map(String::as_str)),
            );
            let mut names = BTreeSet::new();
            for name in candidates {
                if name.contains(&query) && !excluded.iter().any(|excluded| excluded == name) {
                    names.insert(JsName(name));
                    if names.len() > limit {
                        names.pop_last();
                    }
                }
            }
            let mut bytes = 0usize;
            names
                .into_iter()
                .map(|name| {
                    bytes = bytes.checked_add(name.0.len()).ok_or_else(result_limit)?;
                    if bytes > MAX_RESULT {
                        return Err(result_limit());
                    }
                    Ok(name.0.to_owned())
                })
                .collect()
        })
        .await
        .map_err(|error| StoreError::Io(format!("tag lookup worker failed: {error}")))?
    }
}

fn result_limit() -> StoreError {
    StoreError::ResourceLimit("Tag suggestion response exceeds its text budget".into())
}

fn js_whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[cfg(test)]
mod tests;
