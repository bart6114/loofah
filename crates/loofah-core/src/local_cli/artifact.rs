use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};

pub struct LocalCliArtifact {
    file: File,
    pub size_bytes: u64,
    pub sha256: String,
    pub trusted_path: Option<PathBuf>,
    owner: Box<dyn Send + 'static>,
}
impl LocalCliArtifact {
    pub fn new(
        file: File,
        size_bytes: u64,
        sha256: String,
        trusted_path: Option<PathBuf>,
        owner: Box<dyn Send + 'static>,
    ) -> Self {
        Self {
            file,
            size_bytes,
            sha256,
            trusted_path,
            owner,
        }
    }
    pub fn into_parts(self) -> (File, Box<dyn Send + 'static>) {
        (self.file, self.owner)
    }
}
impl Read for LocalCliArtifact {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(output)
    }
}
impl Seek for LocalCliArtifact {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}
