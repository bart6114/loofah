fn main() {
    println!("cargo:rerun-if-changed=native/coordinate.m");
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        cc::Build::new()
            .file("native/coordinate.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("loofah_file_coordinate");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}
