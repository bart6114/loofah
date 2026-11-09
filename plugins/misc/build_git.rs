use std::{fs, path::Path, path::PathBuf};

pub fn watched_paths(start: &Path) -> Vec<PathBuf> {
    let Some(dot_git) = start
        .ancestors()
        .map(|path| path.join(".git"))
        .find(|path| path.exists())
    else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        paths.push(dot_git.clone());
        let Some(target) = fs::read_to_string(&dot_git).ok().and_then(|text| {
            text.strip_prefix("gitdir: ")
                .map(str::trim)
                .map(str::to_owned)
        }) else {
            return paths;
        };
        dot_git.parent().unwrap().join(target)
    };
    let Ok(git_dir) = git_dir.canonicalize() else {
        return paths;
    };
    let common_pointer = git_dir.join("commondir");
    let common_dir = match fs::read_to_string(&common_pointer) {
        Ok(target) => {
            paths.push(common_pointer);
            match git_dir.join(target.trim()).canonicalize() {
                Ok(path) => path,
                Err(_) => return paths,
            }
        }
        Err(_) => git_dir.clone(),
    };
    let head = git_dir.join("HEAD");
    if !head.is_file() {
        return paths;
    }
    paths.push(head.clone());
    let Some(reference) = fs::read_to_string(head)
        .ok()
        .and_then(|text| text.strip_prefix("ref: ").map(str::trim).map(str::to_owned))
    else {
        return paths;
    };
    if !reference.starts_with("refs/")
        || reference
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || reference.contains('\\')
    {
        return paths;
    }
    let ref_dir = if ["refs/bisect/", "refs/worktree/", "refs/rewritten/"]
        .iter()
        .any(|prefix| reference.starts_with(prefix))
    {
        &git_dir
    } else {
        &common_dir
    };
    let loose = ref_dir.join(reference);
    if loose.is_file() {
        paths.push(loose);
    } else {
        // A commit after pack-refs creates a loose ref without changing packed-refs.
        // Watch only its existing refs ancestor, never the admin/object tree.
        let refs = ref_dir.join("refs");
        if let Some(parent) = loose
            .ancestors()
            .skip(1)
            .take_while(|path| path.starts_with(&refs))
            .find(|path| path.is_dir())
        {
            paths.push(parent.to_path_buf());
        }
    }
    let packed = common_dir.join("packed-refs");
    if packed.is_file() {
        paths.push(packed);
    }
    paths
}
