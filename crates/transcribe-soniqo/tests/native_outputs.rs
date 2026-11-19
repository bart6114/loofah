#![cfg(target_os = "macos")]

#[path = "../build/native_outputs.rs"]
mod native_outputs;

use std::fs;

#[test]
fn restores_complete_outputs_and_leaves_local_files_intact_if_cache_changes() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("first.a");
    let metallib = temp.path().join("first.metallib");
    fs::write(&archive, b"archive").unwrap();
    fs::write(&metallib, b"metallib").unwrap();
    let cache = temp.path().join("shared");
    let restored_archive = temp.path().join("second/lib.a");
    let restored_metallib = temp.path().join("second/mlx.metallib");

    assert!(!native_outputs::restore(&cache, &restored_archive, &restored_metallib).unwrap());
    native_outputs::save(&cache, &archive, &metallib).unwrap();
    assert!(native_outputs::restore(&cache, &restored_archive, &restored_metallib).unwrap());
    assert_eq!(fs::read(&restored_archive).unwrap(), b"archive");
    assert_eq!(fs::read(&restored_metallib).unwrap(), b"metallib");

    fs::write(cache.join(native_outputs::SWIFT_ARCHIVE), b"damaged").unwrap();
    assert!(!native_outputs::restore(&cache, &restored_archive, &restored_metallib).unwrap());
    assert_eq!(fs::read(&restored_archive).unwrap(), b"archive");
    assert_eq!(fs::read(&restored_metallib).unwrap(), b"metallib");
}
