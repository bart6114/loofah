#[cfg(target_os = "macos")]
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(target_os = "macos")]
const MACOS_MINIMUM_SYSTEM_VERSION: &str = "15.0";

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-env-changed=LOOFAH_WINDOWS_SIGN_COMMAND");
        println!("cargo:rerun-if-env-changed=LOOFAH_WINDOWS_SIGNER_THUMBPRINT");
        let config: serde_json::Value =
            serde_json::from_str(&std::env::var("TAURI_CONFIG").unwrap_or_else(|_| "{}".into()))
                .expect("valid Tauri config");
        if config["identifier"].as_str() == Some("io.loofah.stable") {
            for variable in [
                "LOOFAH_WINDOWS_SIGN_COMMAND",
                "LOOFAH_WINDOWS_SIGNER_THUMBPRINT",
                "TAURI_SIGNING_PRIVATE_KEY",
            ] {
                assert!(
                    std::env::var(variable).is_ok_and(|value| !value.trim().is_empty()),
                    "Stable Windows builds require {variable}; use the staging config for unsigned test installers"
                );
            }
            assert!(
                config.pointer("/bundle/windows/signCommand").is_some(),
                "Stable Windows builds require the verified Authenticode signing command"
            );
            assert!(
                config
                    .pointer("/plugins/updater/endpoints")
                    .and_then(|value| value.as_array())
                    .is_some_and(|endpoints| endpoints.iter().all(|endpoint| endpoint
                        .as_str()
                        .is_some_and(|url| url.ends_with("/latest-windows.json")))
                        && !endpoints.is_empty()),
                "Stable Windows builds must use the Windows updater feed"
            );
        }
        // Tauri's resource compiler only links the manifest into binary targets.
        // Library test executables also import Common Controls v6 through dialogs.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        tauri_build::try_build(
            tauri_build::Attributes::new()
                .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
        )
        .expect("build Windows resources");
        return;
    }

    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-fapple-link-rtlib");

    #[cfg(target_os = "macos")]
    build_check_permissions();

    tauri_build::build()
}

#[cfg(target_os = "macos")]
fn build_check_permissions() {
    let triple = std::env::var("TARGET").unwrap();
    let swift_target = swift_target(&triple);

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let build_rs = manifest_dir.join("build.rs");
    let swift_src = manifest_dir.join("../../../plugins/permissions/swift/check-permissions.swift");
    let binaries_dir = manifest_dir.join("binaries");
    let dst = binaries_dir.join(format!("check-permissions-{triple}"));
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let compiled = out_dir.join(format!("check-permissions-{triple}"));

    println!("cargo:rerun-if-changed={}", build_rs.display());
    println!("cargo:rerun-if-changed={}", swift_src.display());

    fs::create_dir_all(&binaries_dir).expect("create binaries/");

    if is_fresh(&dst, &[&build_rs, &swift_src]) {
        return;
    }

    let status = Command::new("swiftc")
        .args(["-O", "-target"])
        .arg(&swift_target)
        .arg("-o")
        .arg(&compiled)
        .arg(&swift_src)
        .status()
        .expect("failed to run swiftc");

    assert!(
        status.success(),
        "swiftc failed to compile check-permissions"
    );

    if !same_contents(&compiled, &dst) {
        fs::copy(&compiled, &dst).expect("copy check-permissions binary");
    }
}

#[cfg(target_os = "macos")]
fn swift_target(cargo_target: &str) -> String {
    let arch = cargo_target
        .split('-')
        .next()
        .expect("TARGET contains an architecture");
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => panic!("unsupported macOS target architecture: {arch}"),
    };

    format!("{arch}-apple-macosx{MACOS_MINIMUM_SYSTEM_VERSION}")
}

#[cfg(target_os = "macos")]
fn is_fresh(output: &Path, inputs: &[&Path]) -> bool {
    let Ok(output_modified) = fs::metadata(output).and_then(|metadata| metadata.modified()) else {
        return false;
    };

    inputs.iter().all(|input| {
        fs::metadata(input)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified <= output_modified)
    })
}

#[cfg(target_os = "macos")]
fn same_contents(a: &Path, b: &Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
