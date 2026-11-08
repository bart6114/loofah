import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def select_artifact(messages, repository):
    artifacts = [
        item
        for item in messages
        if item.get("reason") == "compiler-artifact"
        and item["target"]["name"] == "loofah_server"
        and item.get("executable")
        and item["profile"]["test"]
    ]
    if len(artifacts) != 1:
        raise ValueError("Expected exactly one server library test executable")
    artifact = artifacts[0]
    source = repository / "apps/server/src/lib.rs"
    if Path(artifact["target"]["src_path"]).resolve() != source.resolve():
        raise ValueError("Server artifact belongs to another source tree")
    if "cpu-transcription" not in artifact["features"]:
        raise ValueError("CPU transcription was not enabled")
    return artifact


def validate_inventory(listing, case):
    tests = [
        line.removesuffix(": test")
        for line in listing.splitlines()
        if line.endswith(": test")
    ]
    if tests.count(case) != 1:
        raise ValueError("Configured service CPU case is absent or ambiguous")
    for prefix in (
        "transcription_cpu::tests::",
        "tests::transcription_host::",
        "config::tests::",
    ):
        if not any(test.startswith(prefix) for test in tests):
            raise ValueError(f"Required CPU test group is empty: {prefix}")
    return tests


def file_sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--messages", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--github-output", type=Path, required=True)
    args = parser.parse_args()
    repository = Path.cwd().resolve()
    messages = []
    for line in args.messages.read_text().splitlines():
        try:
            messages.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    artifact = select_artifact(messages, repository)
    binary = Path(artifact["executable"]).resolve()
    with binary.open("rb") as source:
        header = source.read(20)
    if (
        header[:6] != b"\x7fELF\x02\x01"
        or int.from_bytes(header[18:20], "little") != 62
    ):
        raise ValueError("Expected a Linux x64 ELF test executable")
    native = {
        item["target"]["name"]: item["features"]
        for item in messages
        if item.get("reason") == "compiler-artifact"
        and item["target"]["name"] in ("whisper_local", "whisper_rs", "whisper_rs_sys")
    }
    if len(native) != 3 or any(
        set(features) & {"coreml", "metal", "cuda", "hipblas", "vulkan", "directml"}
        for features in native.values()
    ):
        raise ValueError("Missing CPU native graph or unexpected GPU features")
    model_sha = file_sha256(args.model)
    if model_sha != "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9":
        raise ValueError("The cached model does not match the existing pinned fixture")
    listing = subprocess.run(
        [str(binary), "--list", "--format", "terse"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    tests = validate_inventory(listing, args.case)
    args.output.with_suffix(".inventory.txt").write_text(listing)
    proof = {
        "artifact": artifact,
        "binary_sha256": file_sha256(binary),
        "model_sha256": model_sha,
        "native_features": native,
        "required_case": args.case,
        "inventory": tests,
        "source_sha256": {
            str(path.relative_to(repository)): file_sha256(path)
            for path in sorted((repository / "apps/server/src").rglob("*.rs"))
        },
        "cargo_lock_sha256": file_sha256(repository / "Cargo.lock"),
    }
    args.output.write_text(json.dumps(proof, indent=2) + "\n")
    with args.github_output.open("a") as output:
        output.write(f"test_binary={binary}\n")
    print(
        json.dumps(
            {
                "case": args.case,
                "binary_sha256": proof["binary_sha256"],
                "fresh": artifact["fresh"],
            }
        )
    )


if __name__ == "__main__":
    main()
