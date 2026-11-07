use super::*;

async fn begin(io: &DeviceUploadIo, root: &Path, owner: &str, size: u64) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    io.stages
        .begin(
            root.into(),
            owner.into(),
            id.clone(),
            "clipboard ü.png".into(),
            size,
            io.reserve().unwrap(),
        )
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn chunks_are_bounded_ordered_replayable_and_only_finished_declarations_can_be_read() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("unknown.bin"), b"Preserve unknown").unwrap();
    let io = DeviceUploadIo::default();
    let data = vec![7_u8; CHUNK + 17];
    let id = begin(&io, root.path(), "first", data.len() as u64).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let entry = io.stages.entry("first", &id).unwrap();
        let row = entry.row.lock().await;
        assert_eq!(
            row.file.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    io.stages
        .begin(
            root.path().into(),
            "first".into(),
            id.clone(),
            "clipboard ü.png".into(),
            data.len() as u64,
            io.reserve().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    for (offset, chunk) in [(1, vec![7]), (0, vec![]), (0, vec![7; CHUNK + 1])] {
        assert!(
            io.stages
                .append("first", &id, offset, chunk, io.reserve().unwrap())
                .await
                .is_err()
        );
    }
    assert!(
        io.stages
            .finish("first", &id, io.reserve().unwrap())
            .await
            .is_err()
    );
    assert_eq!(
        io.stages
            .append(
                "first",
                &id,
                0,
                data[..CHUNK].to_vec(),
                io.reserve().unwrap()
            )
            .await
            .unwrap(),
        CHUNK as u64
    );
    assert_eq!(
        io.stages
            .append(
                "first",
                &id,
                0,
                data[..CHUNK].to_vec(),
                io.reserve().unwrap()
            )
            .await
            .unwrap(),
        CHUNK as u64
    );
    assert!(
        io.stages
            .append("first", &id, 0, vec![9; CHUNK], io.reserve().unwrap())
            .await
            .is_err()
    );
    assert_eq!(
        io.stages
            .append(
                "first",
                &id,
                CHUNK as u64,
                data[CHUNK..].to_vec(),
                io.reserve().unwrap()
            )
            .await
            .unwrap(),
        data.len() as u64
    );
    assert!(
        io.stages
            .append(
                "first",
                &id,
                data.len() as u64,
                vec![7],
                io.reserve().unwrap()
            )
            .await
            .is_err()
    );
    let input = io
        .stages
        .finish("first", &id, io.reserve().unwrap())
        .await
        .unwrap();
    assert_eq!(input.filename, "clipboard ü.png");
    assert_eq!(input.size_bytes, data.len() as u64);
    assert_eq!(
        io.stages
            .finish("first", &id, io.reserve().unwrap())
            .await
            .unwrap(),
        input
    );
    assert!(
        io.stages
            .append("first", &id, 0, vec![7], io.reserve().unwrap())
            .await
            .is_err()
    );
    let mut source = io
        .reader(
            "first",
            DeviceAttachmentSource::Staged { id: id.clone() },
            &input,
        )
        .unwrap();
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, data);
    drop(source);
    let mut changed = input.clone();
    changed.sha256 = "0".repeat(64);
    let mut source = io
        .reader(
            "first",
            DeviceAttachmentSource::Staged { id: id.clone() },
            &changed,
        )
        .unwrap();
    assert!(source.read(&mut [0; 1]).is_err());
    drop(source);
    io.stages.close("first", &id).await;
    assert_eq!(io.stages.used.load(Ordering::Acquire), 0);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(root.path().join("unknown.bin")).unwrap(),
        b"Preserve unknown"
    );
}

#[tokio::test]
async fn sources_are_window_scoped_and_closing_retains_budget_and_files_until_real_readers_drop() {
    let root = tempfile::tempdir().unwrap();
    let io = DeviceUploadIo::default();
    let id = begin(&io, root.path(), "first", 3).await;
    assert!(
        io.stages
            .begin(
                root.path().into(),
                "second".into(),
                id.clone(),
                "clipboard ü.png".into(),
                3,
                io.reserve().unwrap()
            )
            .await
            .is_err()
    );
    assert!(
        io.stages
            .append("second", &id, 0, b"abc".to_vec(), io.reserve().unwrap())
            .await
            .is_err()
    );
    io.stages
        .append("first", &id, 0, b"abc".to_vec(), io.reserve().unwrap())
        .await
        .unwrap();
    let input = io
        .stages
        .finish("first", &id, io.reserve().unwrap())
        .await
        .unwrap();
    let mut foreign = io
        .reader(
            "second",
            DeviceAttachmentSource::Staged { id: id.clone() },
            &input,
        )
        .unwrap();
    assert!(foreign.read(&mut [0; 1]).is_err());
    drop(foreign);
    io.stages.close("second", &id).await;
    let mut source = io
        .reader(
            "first",
            DeviceAttachmentSource::Staged { id: id.clone() },
            &input,
        )
        .unwrap();
    assert_eq!(source.read(&mut [0; 1]).unwrap(), 1);
    io.release_window("first");
    assert_eq!(io.stages.used.load(Ordering::Acquire), 3);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    assert!(source.read(&mut [0; 1]).is_err());
    drop(source);
    settled(&io, 0, 0).await;
    assert_eq!(io.stages.used.load(Ordering::Acquire), 0);
    assert_eq!(io.slots.available_permits(), 2);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn empty_sources_finish_and_window_count_and_declared_disk_budgets_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let io = DeviceUploadIo::default();
    let id = begin(&io, root.path(), "first", 0).await;
    let input = io
        .stages
        .finish("first", &id, io.reserve().unwrap())
        .await
        .unwrap();
    let mut source = io
        .reader(
            "first",
            DeviceAttachmentSource::Staged { id: id.clone() },
            &input,
        )
        .unwrap();
    assert_eq!(source.read(&mut [0; 1]).unwrap(), 0);
    drop(source);
    for _ in 0..7 {
        begin(&io, root.path(), "first", 0).await;
    }
    assert!(
        io.stages
            .begin(
                root.path().into(),
                "first".into(),
                uuid::Uuid::new_v4().to_string(),
                "x.bin".into(),
                0,
                io.reserve().unwrap()
            )
            .await
            .is_err()
    );
    io.release_window("first");
    settled(&io, 0, 0).await;
    begin(&io, root.path(), "first", MAX_SOURCE_BYTES).await;
    begin(&io, root.path(), "second", MAX_SOURCE_BYTES).await;
    assert_eq!(io.stages.used.load(Ordering::Acquire), MAX_STAGED_BYTES);
    assert!(
        io.stages
            .begin(
                root.path().into(),
                "third".into(),
                uuid::Uuid::new_v4().to_string(),
                "x.bin".into(),
                1,
                io.reserve().unwrap()
            )
            .await
            .is_err()
    );
    io.release_window("first");
    settled(&io, 1, MAX_SOURCE_BYTES).await;
    begin(&io, root.path(), "third", 1).await;
    io.shutdown();
    settled(&io, 0, 0).await;
    assert_eq!(io.stages.used.load(Ordering::Acquire), 0);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert!(io.reserve().is_err());
}

#[tokio::test]
async fn invalid_declarations_and_rebinding_an_existing_source_never_change_its_bytes() {
    let root = tempfile::tempdir().unwrap();
    let io = DeviceUploadIo::default();
    for (name, size) in [
        ("../notes.md", 1),
        (".hidden", 1),
        ("x.bin", MAX_SOURCE_BYTES + 1),
    ] {
        assert!(
            io.stages
                .begin(
                    root.path().into(),
                    "first".into(),
                    uuid::Uuid::new_v4().to_string(),
                    name.into(),
                    size,
                    io.reserve().unwrap()
                )
                .await
                .is_err()
        );
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    let id = begin(&io, root.path(), "first", 2).await;
    assert!(
        io.stages
            .begin(
                root.path().into(),
                "first".into(),
                id.clone(),
                "changed.pdf".into(),
                2,
                io.reserve().unwrap()
            )
            .await
            .is_err()
    );
    let input = UploadAttachmentRequest {
        filename: "clipboard ü.png".into(),
        size_bytes: 2,
        sha256: "0".repeat(64),
    };
    let mut source = io
        .reader("first", DeviceAttachmentSource::Staged { id }, &input)
        .unwrap();
    assert!(source.read(&mut [0; 2]).is_err());
    drop(source);
    io.shutdown();
    settled(&io, 0, 0).await;
}

async fn settled(io: &DeviceUploadIo, entries: usize, bytes: u64) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while io.stages.slots.available_permits() != 32 - entries
            || io.stages.used.load(Ordering::Acquire) != bytes
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
