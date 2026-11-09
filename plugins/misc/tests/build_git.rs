#[path = "../build_git.rs"]
mod build_git;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Repository {
    root: PathBuf,
    main: PathBuf,
    linked: PathBuf,
}

impl Repository {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "loofah-build-git-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("config"), "").unwrap();
        fs::create_dir(root.join("hooks")).unwrap();
        let repo = Self {
            main: root.join("main"),
            linked: root.join("linked"),
            root,
        };
        fs::create_dir(&repo.main).unwrap();
        repo.git(&repo.main, &["init", "--initial-branch=main"]);
        repo.commit(&repo.main);
        repo.git(
            &repo.main,
            &["worktree", "add", "-b", "refactor/metadata", "../linked"],
        );
        repo
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        assert!(cwd == self.main || cwd == self.linked);
        // Keep verbatim Windows paths out of Git config and hook arguments.
        let output = Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "../config")
            .args([
                "-c",
                "user.name=Build Metadata Test",
                "-c",
                "user.email=build@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=../hooks",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn commit(&self, cwd: &Path) {
        self.git(cwd, &["commit", "--allow-empty", "-m", "metadata fixture"]);
    }

    fn admin(&self) -> PathBuf {
        PathBuf::from(self.git(&self.linked, &["rev-parse", "--absolute-git-dir"]))
            .canonicalize()
            .unwrap()
    }

    fn watched(&self, start: &Path) -> Vec<PathBuf> {
        let paths = build_git::watched_paths(start);
        assert!(paths.iter().all(|path| path.exists()));
        assert!(
            paths
                .iter()
                .all(|path| { !path.is_dir() || path.starts_with(self.main.join(".git/refs")) })
        );
        paths
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn linked_worktree_tracks_common_loose_ref_when_symbolic_head_does_not_change() {
    let repo = Repository::new();
    let nested = repo.linked.join("plugins/misc");
    fs::create_dir_all(&nested).unwrap();
    let paths = repo.watched(&nested);
    let head = repo.admin().join("HEAD");
    let loose = repo.main.join(".git/refs/heads/refactor/metadata");
    assert!(paths.contains(&repo.linked.join(".git")));
    assert!(paths.contains(&repo.admin().join("commondir")));
    assert!(paths.contains(&head));
    assert!(paths.contains(&loose));
    let old_head = fs::read(&head).unwrap();
    let old_ref = fs::read(&loose).unwrap();
    repo.commit(&repo.linked);
    assert_eq!(fs::read(head).unwrap(), old_head);
    assert_ne!(fs::read(loose).unwrap(), old_ref);
}

#[test]
fn packed_ref_tracks_existing_ancestor_for_the_next_loose_commit() {
    let repo = Repository::new();
    let loose = repo.main.join(".git/refs/heads/refactor/metadata");
    assert!(repo.watched(&repo.linked).contains(&loose));
    repo.git(&repo.main, &["pack-refs", "--all", "--prune"]);
    assert!(!loose.exists());
    let packed = repo.main.join(".git/packed-refs");
    let old_packed = fs::read(&packed).unwrap();
    let paths = repo.watched(&repo.linked);
    assert!(paths.contains(&packed));
    assert!(!paths.contains(&loose));
    assert!(
        paths
            .iter()
            .any(|path| path.is_dir() && loose.starts_with(path))
    );
    repo.commit(&repo.linked);
    assert_eq!(fs::read(packed).unwrap(), old_packed);
    assert!(repo.watched(&repo.linked).contains(&loose));
}

#[test]
fn detached_head_and_branch_switch_are_tracked_without_object_directory_watches() {
    let repo = Repository::new();
    repo.git(&repo.linked, &["checkout", "--detach"]);
    let head = repo.admin().join("HEAD");
    let before = fs::read(&head).unwrap();
    let paths = repo.watched(&repo.linked);
    assert!(paths.contains(&head));
    assert!(paths.iter().all(|path| path.is_file()));
    repo.commit(&repo.linked);
    assert_ne!(fs::read(&head).unwrap(), before);
    repo.git(&repo.linked, &["checkout", "refactor/metadata"]);
    assert!(
        repo.watched(&repo.linked)
            .contains(&repo.main.join(".git/refs/heads/refactor/metadata"))
    );
}

#[test]
fn ordinary_repository_and_source_archive_keep_existing_fallback_behavior() {
    let repo = Repository::new();
    let paths = repo.watched(&repo.main);
    assert!(paths.contains(&repo.main.join(".git/HEAD")));
    assert!(paths.contains(&repo.main.join(".git/refs/heads/main")));
    let archive = repo.root.join("archive/plugins/misc");
    fs::create_dir_all(&archive).unwrap();
    assert!(build_git::watched_paths(&archive).is_empty());
}
