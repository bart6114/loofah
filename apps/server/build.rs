fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../package.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let package: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("root package.json"))
            .expect("valid package.json");
    println!(
        "cargo:rustc-env=LOOFAH_VERSION={}",
        package["version"].as_str().expect("Loofah version")
    );
}
