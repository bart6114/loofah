/// Trim, strip a leading `#`, lowercase. `None` when nothing is left — the strict
/// charset filter stays on the frontend; this is only what file-level dedupe needs.
pub fn normalize_tag_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('#').trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize_tag_name("Project-X"), Some("project-x".into()));
        assert_eq!(normalize_tag_name("  #Hiring "), Some("hiring".into()));
        assert_eq!(normalize_tag_name("# spaced "), Some("spaced".into()));
        assert_eq!(normalize_tag_name("   "), None);
        assert_eq!(normalize_tag_name("#"), None);
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct TagItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
}
