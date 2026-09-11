const COMMANDS: &[&str] = &[
    "global_base",
    "vault_base",
    "move_vault",
    "copy_vault",
    "set_vault_base",
    "is_empty_or_missing_dir",
    "classify_vault_dir",
    "path",
    "load",
    "save",
    "get_config",
    "set_config_values",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
