import argparse
import hashlib
import json
from pathlib import Path
import subprocess

from backend_onnx_models import pinned_manifest, provision

ONNX_CASES = (
    "tests::transcription_host::media_jobs::real_configured_onnx_media_preserves_shared_checked_publication",
    "transcription_onnx::tests::real_onnx_live_cpu_preserves_channels_and_source_through_join",
    "tests::transcription_host::reserved_cpu::real_reserved_cpu_delivers_live_caption_while_batch_owns_model_and_http_stays_responsive",
)


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


def validate_inventory(listing, case, onnx_cases=()):
    tests = [
        line.removesuffix(": test")
        for line in listing.splitlines()
        if line.endswith(": test")
    ]
    for required in (case, *onnx_cases):
        if tests.count(required) != 1:
            raise ValueError("Configured service CPU case is absent or ambiguous")
    for prefix in (
        "transcription_cpu::tests::",
        "speech_admission::tests::",
        "tests::transcription_host::",
        "config::tests::",
    ):
        if not any(test.startswith(prefix) for test in tests):
            raise ValueError(f"Required CPU test group is empty: {prefix}")
    return tests


def validate_native_features(messages):
    required = {
        "whisper_local",
        "whisper_rs",
        "whisper_rs_sys",
        "ort",
        "ort_sys",
        "parakeet_rs",
        "transcribe_onnx",
    }
    native = {
        item["target"]["name"]: item["features"]
        for item in messages
        if item.get("reason") == "compiler-artifact"
        and item["target"]["name"] in required
    }
    forbidden = {
        "coreml",
        "metal",
        "cuda",
        "hipblas",
        "vulkan",
        "directml",
        "migraphx",
        "tensorrt",
        "openvino",
        "webgpu",
        "nnapi",
        "load-dynamic",
        "preload-dylibs",
    }
    if set(native) != required or any(
        set(features) & forbidden for features in native.values()
    ):
        raise ValueError(
            "Missing CPU native graph or unexpected GPU/dynamic-runtime features"
        )
    return native


def file_sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--messages", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--onnx-models", type=Path, required=True)
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
    native = validate_native_features(messages)
    onnx = pinned_manifest(repository)
    provision(args.onnx_models, onnx, check_only=True)
    model_sha = file_sha256(args.model)
    if model_sha != "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9":
        raise ValueError("The cached model does not match the existing pinned fixture")
    listing = subprocess.run(
        [str(binary), "--list", "--format", "terse"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    tests = validate_inventory(listing, args.case, ONNX_CASES)
    args.output.with_suffix(".inventory.txt").write_text(listing)
    proof = {
        "artifact": artifact,
        "binary_sha256": file_sha256(binary),
        "model_sha256": model_sha,
        "native_features": native,
        "required_case": args.case,
        "required_onnx_cases": ONNX_CASES,
        "onnx_models": onnx,
        "inventory": tests,
        "source_sha256": {
            str(path.relative_to(repository)): file_sha256(path)
            for root in (
                repository / "apps/server/src",
                repository / "crates/transcribe-onnx/src",
            )
            for path in sorted(root.rglob("*.rs"))
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
