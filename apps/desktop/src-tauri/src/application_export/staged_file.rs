use std::{
    ffi::CString,
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::fs::MetadataExt,
};

pub(super) struct StagedFile {
    file: File,
    directory: File,
    name: CString,
    cleanup: bool,
}
impl StagedFile {
    pub(super) fn create(directory: &File, prefix: &str) -> io::Result<Self> {
        let directory = directory.try_clone()?;
        let name = CString::new(format!("{prefix}{}", uuid::Uuid::new_v4()))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file: unsafe { File::from_raw_fd(fd) },
            directory,
            name,
            cleanup: true,
        })
    }
    pub(super) fn as_file(&self) -> &File {
        &self.file
    }
    pub(super) fn reopen(&self) -> io::Result<File> {
        // A duplicated original descriptor cannot follow a replaced staging path.
        self.file.try_clone()
    }
    fn owns_name(&self) -> io::Result<bool> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        let metadata = unsafe { metadata.assume_init() };
        let original = self.file.metadata()?;
        Ok(metadata.st_dev as u64 == original.dev()
            && metadata.st_ino == original.ino()
            && metadata.st_mode & libc::S_IFMT == libc::S_IFREG)
    }
    pub(super) fn publish(&mut self, name: &str) -> io::Result<()> {
        if !self.owns_name()? {
            return Err(io::Error::other("Export staging file changed"));
        }
        let destination = CString::new(name)?;
        let directory = self.directory.as_raw_fd();
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                directory,
                self.name.as_ptr(),
                directory,
                destination.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(not(target_os = "macos"))]
        let result = unsafe {
            libc::renameat2(
                directory,
                self.name.as_ptr(),
                directory,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        self.name = destination;
        Ok(())
    }
    pub(super) fn sync(&self) -> io::Result<()> {
        self.file.sync_all()?;
        self.directory.sync_all()
    }
    pub(super) fn keep(&mut self) {
        self.cleanup = false;
    }
    #[cfg(target_os = "macos")]
    pub(super) fn mark_download(&self) -> io::Result<()> {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let value = format!("0081;{seconds:x};Loofah;{}", uuid::Uuid::new_v4());
        let result = unsafe {
            libc::fsetxattr(
                self.file.as_raw_fd(),
                c"com.apple.quarantine".as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "macos"))]
    pub(super) fn mark_download(&self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        // The user-visible directory may have been renamed/replaced. Cleanup
        // must use the original directory owner, including failed publication.
        if self.cleanup && self.owns_name().unwrap_or(false) {
            unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn create_and_publish_use_original_directory_even_after_path_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("original");
        let retired = temp.path().join("retired");
        std::fs::create_dir(&root).unwrap();
        let directory = File::open(&root).unwrap();
        std::fs::rename(&root, &retired).unwrap();
        std::fs::create_dir(&root).unwrap();
        let mut staged = StagedFile::create(&directory, ".loofah-export-").unwrap();
        staged.as_file().write_all(b"original").unwrap();
        let basename = staged.name.to_str().unwrap().to_owned();
        std::fs::write(root.join(&basename), b"foreign staging sentinel").unwrap();
        std::fs::write(root.join("Meeting.txt"), b"foreign publication sentinel").unwrap();
        std::fs::write(retired.join("Meeting.txt"), b"existing original export").unwrap();
        assert_eq!(
            staged.publish("Meeting.txt").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            std::fs::read(retired.join("Meeting.txt")).unwrap(),
            b"existing original export"
        );
        staged.publish("Meeting (1).txt").unwrap();
        staged.sync().unwrap();
        assert_eq!(
            std::fs::read(retired.join("Meeting (1).txt")).unwrap(),
            b"original"
        );
        assert!(!root.join("Meeting (1).txt").exists());
        drop(staged);
        assert!(!retired.join(&basename).exists());
        assert!(!retired.join("Meeting (1).txt").exists());
        assert_eq!(
            std::fs::read(root.join(basename)).unwrap(),
            b"foreign staging sentinel"
        );
        assert_eq!(
            std::fs::read(root.join("Meeting.txt")).unwrap(),
            b"foreign publication sentinel"
        );
    }

    #[test]
    fn cleanup_does_not_remove_a_replaced_file_in_the_original_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = File::open(temp.path()).unwrap();
        let mut staged = StagedFile::create(&directory, ".loofah-export-").unwrap();
        let path = temp.path().join(staged.name.to_str().unwrap());
        std::fs::rename(&path, temp.path().join("original")).unwrap();
        std::fs::write(&path, b"foreign sentinel").unwrap();
        assert!(staged.publish("Meeting.txt").is_err());
        assert!(!temp.path().join("Meeting.txt").exists());
        drop(staged);
        assert_eq!(std::fs::read(path).unwrap(), b"foreign sentinel");
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn quarantine_marks_original_file_after_directory_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("original");
        std::fs::create_dir(&root).unwrap();
        let directory = File::open(&root).unwrap();
        let staged = StagedFile::create(&directory, ".loofah-source-").unwrap();
        std::fs::rename(&root, temp.path().join("retired")).unwrap();
        std::fs::create_dir(&root).unwrap();
        let foreign = File::create(root.join(staged.name.to_str().unwrap())).unwrap();
        staged.mark_download().unwrap();
        let attribute_size = |file: &File| unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                c"com.apple.quarantine".as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                0,
            )
        };
        assert!(attribute_size(staged.as_file()) > 0);
        assert_eq!(attribute_size(&foreign), -1);
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOATTR)
        );
    }
}
