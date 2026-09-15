use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::error::Result;

pub fn to_relative_path(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .ok()
        .and_then(|p| p.to_str())
        .map(|s| s.replace(std::path::MAIN_SEPARATOR, "/"))
        .unwrap_or_default()
}

pub fn is_uuid(name: &str) -> bool {
    Uuid::try_parse(name).is_ok()
}

pub fn resolve_path_inside_base(base: &Path, path: &Path) -> Result<PathBuf> {
    let base = base.canonicalize()?;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let candidate = normalize_absolute_path(&candidate)?;

    let mut existing = candidate.as_path();
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| crate::Error::Path("path_parent_missing".into()))?;
    }

    let existing_canonical = existing.canonicalize()?;
    if !existing_canonical.starts_with(&base) {
        return Err(crate::Error::Path("path_outside_base".into()));
    }

    let suffix = candidate
        .strip_prefix(existing)
        .map_err(|_| crate::Error::Path("path_outside_base".into()))?;
    if suffix.as_os_str().is_empty() {
        return Ok(existing_canonical);
    }

    let resolved = existing_canonical.join(suffix);
    if !resolved.starts_with(&base) {
        return Err(crate::Error::Path("path_outside_base".into()));
    }

    Ok(resolved)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(crate::Error::Path("path_traversal_not_allowed".into()));
                }
            }
        }
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{UUID_1, UUID_2};
    use assert_fs::fixture::PathChild;
    use assert_fs::prelude::*;

    #[test]
    fn test_is_uuid() {
        assert!(is_uuid(UUID_1));
        assert!(is_uuid(UUID_2));
        assert!(is_uuid("550E8400-E29B-41D4-A716-446655440000"));
        assert!(!is_uuid("_default"));
        assert!(!is_uuid("work"));
        assert!(!is_uuid("not-a-uuid"));
    }

    #[test]
    fn resolve_path_inside_base_allows_existing_file() {
        let temp = assert_fs::TempDir::new().unwrap();
        let base = temp.path();
        temp.child("note.txt").write_str("hello").unwrap();

        let result = resolve_path_inside_base(base, &base.join("note.txt")).unwrap();

        assert_eq!(result, base.canonicalize().unwrap().join("note.txt"));
    }

    #[test]
    fn resolve_path_inside_base_existing_file_is_writable() {
        let temp = assert_fs::TempDir::new().unwrap();
        let base = temp.path();
        temp.child("note.txt").write_str("hello").unwrap();

        let result = resolve_path_inside_base(base, Path::new("note.txt")).unwrap();

        std::fs::write(&result, "updated").unwrap();
        assert_eq!(std::fs::read_to_string(&result).unwrap(), "updated");
    }

    #[test]
    fn resolve_path_inside_base_allows_missing_child() {
        let temp = assert_fs::TempDir::new().unwrap();
        let base = temp.path();

        let result = resolve_path_inside_base(base, Path::new("nested/note.txt")).unwrap();

        assert_eq!(
            result,
            base.canonicalize().unwrap().join("nested").join("note.txt")
        );
    }

    #[test]
    fn resolve_path_inside_base_rejects_parent_traversal() {
        let temp = assert_fs::TempDir::new().unwrap();
        let base = temp.path();

        let result = resolve_path_inside_base(base, Path::new("../outside.txt"));

        assert!(
            matches!(result, Err(crate::Error::Path(message)) if message == "path_outside_base")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_inside_base_rejects_symlink_escape() {
        let temp = assert_fs::TempDir::new().unwrap();
        let base = temp.child("vault");
        let outside = temp.child("outside");
        base.create_dir_all().unwrap();
        outside.create_dir_all().unwrap();
        std::os::unix::fs::symlink(outside.path(), base.child("link").path()).unwrap();

        let result = resolve_path_inside_base(base.path(), Path::new("link/file.txt"));

        assert!(
            matches!(result, Err(crate::Error::Path(message)) if message == "path_outside_base")
        );
    }
}
