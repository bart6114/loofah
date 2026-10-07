use std::path::{Path, PathBuf};

use crate::{EnhancedDoc, Error, Result, enhanced, paths};

pub struct LocatedSummary {
    pub markdown: String,
    pub relative_path: PathBuf,
    pub legacy_id: Option<String>,
}

pub fn read_canonical_in(vault: &Path, session_dir: &Path) -> Result<Option<String>> {
    let path = vault.join(paths::summary_path_in(session_dir));
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Io(e.to_string())),
        Ok(meta) if !meta.is_file() => {
            return Err(Error::Io("summary.md is not a regular file".into()));
        }
        Ok(_) => {}
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|e| Error::Io(e.to_string()))
}

pub fn locate_in(
    vault: &Path,
    session_dir: &Path,
    session_id: &str,
) -> Result<Option<LocatedSummary>> {
    let canonical = read_canonical_in(vault, session_dir);
    let mut first_error = None;
    let mut legacy = Vec::new();
    for (_, parsed) in enhanced::scan_legacy_docs_in(vault, session_dir, session_id)? {
        let doc = match parsed {
            Ok(doc) => doc,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        if doc.kind == "summary" {
            legacy.push(doc);
        }
    }
    legacy.sort_by(|a, b| (a.sort_order, &a.id).cmp(&(b.sort_order, &b.id)));
    if let Some(doc) = legacy.into_iter().next() {
        return Ok(Some(LocatedSummary {
            relative_path: paths::enhanced_doc_path_in(session_dir, &doc.id),
            legacy_id: Some(doc.id),
            markdown: doc.markdown,
        }));
    }
    let canonical = canonical?;
    if canonical.is_none() {
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    Ok(canonical.map(|markdown| LocatedSummary {
        markdown,
        relative_path: paths::summary_path_in(session_dir),
        legacy_id: None,
    }))
}

pub fn as_document(session_id: &str, markdown: String) -> EnhancedDoc {
    EnhancedDoc {
        id: session_id.into(),
        session_id: session_id.into(),
        kind: "summary".into(),
        title: "Summary".into(),
        template_id: String::new(),
        sort_order: 0,
        markdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_summary_is_plain_text_and_attachments_are_not_documents() {
        let vault = tempfile::tempdir().unwrap();
        let dir = Path::new("sessions/s1");
        std::fs::create_dir_all(vault.path().join(dir).join("attachments")).unwrap();
        std::fs::write(
            vault.path().join(dir).join("attachments/summary.md"),
            "attachment",
        )
        .unwrap();
        assert!(locate_in(vault.path(), dir, "s1").unwrap().is_none());
        let body = "---\nnot: [yaml\n---\n# Summary";
        std::fs::write(vault.path().join(dir).join("summary.md"), body).unwrap();
        let summary = locate_in(vault.path(), dir, "s1").unwrap().unwrap();
        assert_eq!(summary.markdown, body);
        assert_eq!(summary.legacy_id, None);
        assert_eq!(summary.relative_path, dir.join("summary.md"));
        let docs = enhanced::list_enhanced_docs_in(vault.path(), dir, "s1").unwrap();
        assert_eq!(docs, vec![as_document("s1", body.into())]);
    }

    #[test]
    fn summary_symlink_is_not_followed() {
        let vault = tempfile::tempdir().unwrap();
        let dir = Path::new("sessions/s1");
        std::fs::create_dir_all(vault.path().join(dir)).unwrap();
        std::fs::write(vault.path().join("private.md"), "private").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                vault.path().join("private.md"),
                vault.path().join(dir).join("summary.md"),
            )
            .unwrap();
            assert!(read_canonical_in(vault.path(), dir).is_err());
        }
    }
}
