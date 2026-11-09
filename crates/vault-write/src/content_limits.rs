#[derive(Debug, Clone, Copy)]
pub struct ContentLimits {
    pub note_read_bytes: usize,
    pub transcript_read_bytes: usize,
    pub note_write_bytes: usize,
    pub title_bytes: usize,
    pub task_count: usize,
    pub task_id_bytes: usize,
    pub task_status_bytes: usize,
    pub task_text_bytes: usize,
    pub task_due_bytes: usize,
    pub task_request_bytes: usize,
    pub editor_request_bytes: usize,
    pub tasks_file_bytes: usize,
    pub metadata_bytes: usize,
    pub recovery_bytes: usize,
}

impl Default for ContentLimits {
    fn default() -> Self {
        Self {
            note_read_bytes: 16 * 1024 * 1024,
            transcript_read_bytes: 64 * 1024 * 1024,
            note_write_bytes: 4 * 1024 * 1024,
            title_bytes: 4096,
            task_count: 2000,
            task_id_bytes: 512,
            task_status_bytes: 128,
            task_text_bytes: 16 * 1024,
            task_due_bytes: 256,
            task_request_bytes: 4 * 1024 * 1024,
            editor_request_bytes: 5 * 1024 * 1024,
            tasks_file_bytes: 8 * 1024 * 1024,
            metadata_bytes: 8 * 1024 * 1024,
            recovery_bytes: 32 * 1024 * 1024,
        }
    }
}

impl ContentLimits {
    // Trusted local IPC must not inherit admission limits intended for remote callers.
    pub const fn unlimited() -> Self {
        Self {
            note_read_bytes: usize::MAX,
            transcript_read_bytes: usize::MAX,
            note_write_bytes: usize::MAX,
            title_bytes: usize::MAX,
            task_count: usize::MAX,
            task_id_bytes: usize::MAX,
            task_status_bytes: usize::MAX,
            task_text_bytes: usize::MAX,
            task_due_bytes: usize::MAX,
            task_request_bytes: usize::MAX,
            editor_request_bytes: usize::MAX,
            tasks_file_bytes: usize::MAX,
            metadata_bytes: usize::MAX,
            recovery_bytes: usize::MAX,
        }
    }
}
