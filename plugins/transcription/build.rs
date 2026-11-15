const COMMANDS: &[&str] = &[
    "list_microphone_devices",
    "get_current_microphone_device",
    "start_capture",
    "stop_capture",
    "get_capture_state",
    "get_capture_snapshot",
    "is_supported_languages_live",
    "suggest_providers_for_languages_live",
    "list_documented_language_codes_live",
    "render_transcript_segments",
    "start_transcription",
    "stop_transcription",
    "parse_subtitle",
    "is_supported_languages_batch",
    "suggest_providers_for_languages_batch",
    "list_documented_language_codes_batch",
];

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Standalone Tauri plugin tests also link dialogs requiring Common Controls v6.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src-tauri/windows.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    tauri_plugin::Builder::new(COMMANDS).build();
}
