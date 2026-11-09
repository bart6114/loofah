import argparse
import hashlib
import json
from pathlib import Path
import re
import tempfile
import urllib.request


def pinned_manifest(repository):
    # The runtime's committed Rust manifest remains the authority. A layout or
    # model change fails this extractor closed instead of silently skipping it.
    source = (repository / "crates/transcribe-onnx/src/models.rs").read_text()
    groups = re.findall(r"Self::(\w+) => &\[(.*?)\n\s*\],", source, re.S)
    if {name for name, _ in groups} != {"Streaming", "Batch", "Diarizer"}:
        raise ValueError("Unexpected ONNX runtime model manifest")
    assets = []
    for model, group in groups:
        base = re.search(r"Self::" + model + r' => \{\s*"([^"]+)"', source)
        if not base:
            raise ValueError("Missing pinned ONNX repository revision")
        parsed = re.findall(r"Asset\s*\{(.*?)\n\s*\}", group, re.S)
        if not parsed:
            raise ValueError("Empty ONNX model asset group")
        for asset in parsed:
            name = re.search(r'name:\s*"([^"]+)"', asset)
            size = re.search(r"bytes:\s*(\d+)", asset)
            digest = re.search(r'sha256:\s*"([0-9a-f]{64})"', asset)
            url = re.search(r'url:\s*Some\(\s*"([^"]+)"', asset)
            if not name or not size or not digest:
                raise ValueError("Incomplete pinned ONNX asset")
            name, size = name.group(1), int(size.group(1))
            if (
                Path(name).name != name
                or name in ("", ".", "..")
                or not 0 < size <= 2**31
            ):
                raise ValueError("Invalid ONNX asset path or bound")
            address = url.group(1) if url else base.group(1) + "/" + name
            if not address.startswith("https://"):
                raise ValueError("Model fixtures require verified HTTPS")
            assets.append(
                {
                    "path": model.lower() + "/" + name,
                    "size_bytes": size,
                    "sha256": digest.group(1),
                    "url": address,
                }
            )
    if len({asset["path"] for asset in assets}) != len(assets):
        raise ValueError("Duplicate ONNX asset path")
    return assets


def file_sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify(path, asset):
    return (
        path.is_file()
        and not path.is_symlink()
        and path.stat().st_size == asset["size_bytes"]
        and file_sha256(path) == asset["sha256"]
    )


def provision(root, manifest, check_only):
    for asset in manifest:
        path = root / asset["path"]
        if verify(path, asset):
            continue
        if check_only:
            raise ValueError(f"Missing or changed pinned ONNX asset: {asset['path']}")
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
                temporary = Path(output.name)
                with urllib.request.urlopen(asset["url"], timeout=60) as response:
                    count = 0
                    while chunk := response.read(1024 * 1024):
                        count += len(chunk)
                        if count > asset["size_bytes"]:
                            raise ValueError("Oversized ONNX model fixture")
                        output.write(chunk)
            if not verify(temporary, asset):
                raise ValueError(f"ONNX checksum mismatch: {asset['path']}")
            temporary.chmod(0o644)
            temporary.replace(path)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--proof", type=Path)
    args = parser.parse_args()
    manifest = pinned_manifest(args.repository)
    provision(args.root, manifest, args.check)
    proof = {
        "assets": manifest,
        "manifest_sha256": hashlib.sha256(
            json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
    }
    if args.proof:
        args.proof.write_text(json.dumps(proof, indent=2) + "\n")
    print(
        json.dumps(
            {
                "verified_assets": len(manifest),
                "manifest_sha256": proof["manifest_sha256"],
            }
        )
    )


if __name__ == "__main__":
    main()
