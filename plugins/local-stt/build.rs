const COMMANDS: &[&str] = &[
    "models_dir",
    "is_model_downloaded",
    "is_model_downloading",
    "download_model",
    "cancel_download",
    "delete_model",
    "start_server",
    "stop_server",
    "get_server_for_model",
    "get_servers",
    "list_supported_models",
    "list_supported_languages",
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
