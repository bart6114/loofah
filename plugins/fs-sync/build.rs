const COMMANDS: &[&str] = &[
    "deserialize",
    "write_json_batch",
    "write_document_batch",
    "read_document_batch",
    "audio_exist",
    "audio_delete",
    "audio_metadata",
    "audio_peaks",
    "audio_import",
    "audio_import_data",
    "audio_source_metadata",
    "audio_path",
    "session_dir",
    "load_session_content",
    "delete_session_folder",
    "scan_and_read",
    "entity_dir",
    "attachment_save",
    "attachment_import_path",
    "audio_collect_import_sources",
    "attachment_dir",
    "attachment_list",
    "attachment_read",
    "attachment_remove",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
