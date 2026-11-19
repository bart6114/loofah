use std::{fs, io, path::Path, process::Command};

pub const SWIFT_ARCHIVE: &str = "libsoniqo-swift.a";
pub const METALLIB: &str = "mlx.metallib";
const OUTPUTS: [&str; 2] = [SWIFT_ARCHIVE, METALLIB];
const MANIFEST: &str = "SHA256SUMS";

pub fn restore(cache: &Path, archive: &Path, metallib: &Path) -> io::Result<bool> {
    let Ok(expected) = fs::read(cache.join(MANIFEST)) else {
        return Ok(false);
    };
    if checksums(cache)? != expected {
        return Ok(false);
    }
    for (name, destination) in OUTPUTS.into_iter().zip([archive, metallib]) {
        fs::create_dir_all(destination.parent().expect("native output has a parent"))?;
        fs::copy(cache.join(name), destination)?;
    }
    Ok(true)
}

pub fn save(cache: &Path, archive: &Path, metallib: &Path) -> io::Result<()> {
    if cache.exists() {
        return Ok(());
    }
    let parent = cache.parent().expect("native cache has a parent");
    fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    for (name, source) in OUTPUTS.into_iter().zip([archive, metallib]) {
        fs::copy(source, staging.path().join(name))?;
    }
    fs::write(staging.path().join(MANIFEST), checksums(staging.path())?)?;
    fs::rename(staging.path(), cache).or_else(
        |error| {
            if cache.is_dir() { Ok(()) } else { Err(error) }
        },
    )
}

fn checksums(root: &Path) -> io::Result<Vec<u8>> {
    for name in OUTPUTS {
        let metadata = fs::metadata(root.join(name))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(io::Error::other(format!("invalid native output {name}")));
        }
    }
    let output = Command::new("shasum")
        .current_dir(root)
        .args(["-a", "256"])
        .args(OUTPUTS)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("could not checksum Soniqo native outputs"));
    }
    Ok(output.stdout)
}
