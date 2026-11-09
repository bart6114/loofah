"""Prewarm only the checksum-pinned SpeechCore SwiftPM artifact on macOS CI."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time
from urllib.parse import urljoin, urlsplit

REVISION = "7f382bb4bf24d560e927893a64d8ffb9cf30bc89"
VERSION = "0.0.29"
URL = "https://github.com/soniqo/speech-core/releases/download/v0.0.14/SpeechCore.xcframework.zip"
# The binaryTarget in speech-swift's Package.swift at REVISION is the authority.
SHA256 = "68517f044b98a54e8779dccab850ae383ad5a5595471430c9e10118abec345cd"
CACHE_NAME = "https___github_com_soniqo_speech_core_releases_download_v0_0_14_SpeechCore_xcframework_zip"
CACHE_KEY = "macos-swiftpm-speechcore-v1-" + SHA256
MAX_BYTES = 16 * 1024 * 1024
ATTEMPT_SECONDS = 45
MAX_ATTEMPTS = 3
BACKOFF_SECONDS = (3, 9)
OFFICIAL_HOSTS = {
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
}
TRANSIENT_HTTP = {408, 429, 500, 502, 503, 504}
TRANSIENT_CURL = {6, 7, 18, 28, 52, 55, 56}


class ArtifactError(Exception):
    pass


class TransientDownload(ArtifactError):
    pass


def validate_pins(repository):
    root = repository / "crates/transcribe-soniqo/swift-lib"
    manifest = (root / "Package.swift").read_text()
    resolved = json.loads((root / "Package.resolved").read_text())
    matches = [pin for pin in resolved["pins"] if pin["identity"] == "speech-swift"]
    if (
        len(matches) != 1
        or matches[0]["location"] != "https://github.com/soniqo/speech-swift"
        or matches[0]["state"] != {"revision": REVISION, "version": VERSION}
        or not re.search(
            r'\.package\(\s*url:\s*"https://github.com/soniqo/speech-swift",\s*exact:\s*"0\.0\.29"\s*\)',
            manifest,
        )
    ):
        raise ArtifactError(
            "SpeechCore prewarm must be reviewed with the changed Swift package pin"
        )


def validate_swiftpm(version):
    # This cache layout was reviewed against SwiftPM swift-6.3-RELEASE, both
    # Workspace+Configuration.swift and Workspace+BinaryArtifacts.swift.
    if not re.search(r"Swift Package Manager\s*-\s*Swift 6\.3(?:[.\s-]|$)", version):
        raise ArtifactError(
            "Unreviewed SwiftPM artifact-cache layout; expected Swift 6.3"
        )


def official_url(url):
    parsed = urlsplit(url)
    try:
        safe = parsed.port in (None, 443)
    except ValueError:
        safe = False
    if (
        any(ord(character) < 33 for character in url)
        or parsed.scheme != "https"
        or parsed.hostname not in OFFICIAL_HOSTS
        or parsed.username is not None
        or parsed.password is not None
        or parsed.fragment
        or not safe
    ):
        raise ArtifactError(
            "Artifact redirect must remain on an approved official HTTPS host"
        )
    return url


def verified(path, expected=SHA256):
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return False
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_nlink != 1
        or not 0 < metadata.st_size <= MAX_BYTES
    ):
        raise ArtifactError(
            "SpeechCore cache entry is not a bounded private regular file"
        )
    digest = hashlib.sha256()
    total = 0
    with path.open("rb") as source:
        while chunk := source.read(64 * 1024):
            total += len(chunk)
            if total > MAX_BYTES:
                raise ArtifactError("SpeechCore artifact exceeds 16 MiB")
            digest.update(chunk)
    if total != metadata.st_size or digest.hexdigest() != expected:
        raise ArtifactError("SpeechCore checksum mismatch; refusing this cache entry")
    return True


def download_attempt(destination, run=subprocess.run, clock=time.monotonic):
    deadline = clock() + ATTEMPT_SECONDS
    current = URL
    with tempfile.NamedTemporaryFile(
        dir=destination.parent, prefix=".speechcore-headers-", delete=False
    ) as headers:
        header_path = Path(headers.name)
    try:
        for redirect in range(4):
            official_url(current)
            remaining = deadline - clock()
            if remaining <= 0:
                raise TransientDownload("SpeechCore transfer deadline reached")
            command = [
                "curl",
                "--disable",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--connect-timeout",
                str(min(10, remaining)),
                "--max-time",
                str(remaining),
                "--retry",
                "0",
                "--max-filesize",
                str(MAX_BYTES),
                "--max-redirs",
                "0",
                "--header",
                "Accept: application/octet-stream",
                "--dump-header",
                str(header_path),
                "--output",
                str(destination),
                "--write-out",
                "%{http_code}",
                current,
            ]
            try:
                response = run(
                    command, capture_output=True, text=True, timeout=remaining + 1
                )
            except subprocess.TimeoutExpired:
                raise TransientDownload(
                    "SpeechCore transfer deadline reached"
                ) from None
            # Neither curl stderr nor Location is logged: either may contain a signed URL.
            if response.returncode:
                kind = (
                    TransientDownload
                    if response.returncode in TRANSIENT_CURL
                    else ArtifactError
                )
                raise kind(f"SpeechCore transfer failed (curl {response.returncode})")
            try:
                status = int(response.stdout.strip())
            except ValueError:
                raise ArtifactError("Invalid SpeechCore HTTP status") from None
            if status in TRANSIENT_HTTP:
                raise TransientDownload(f"SpeechCore transient HTTP {status}")
            if status == 200:
                return
            if status not in {301, 302, 303, 307, 308} or redirect == 3:
                raise ArtifactError(f"SpeechCore refused HTTP {status}")
            if header_path.stat().st_size > 64 * 1024:
                raise ArtifactError("SpeechCore redirect headers exceed their bound")
            locations = re.findall(
                r"(?im)^location:\s*([^\r\n]+)", header_path.read_text()
            )
            if len(locations) != 1:
                raise ArtifactError("Ambiguous SpeechCore redirect")
            current = official_url(urljoin(current, locations[0].strip()))
        raise ArtifactError("SpeechCore redirect limit reached")
    finally:
        header_path.unlink(missing_ok=True)


def prepare(path, expected=SHA256, download=download_attempt, sleep=time.sleep):
    for parent in path.parent.parents:
        if parent.is_symlink():
            raise ArtifactError("SpeechCore cache parent must not be a symlink")
    if path.parent.is_symlink():
        raise ArtifactError("SpeechCore cache parent must not be a symlink")
    if verified(path, expected):
        print("SpeechCore verified cache hit")
        return "cached"
    path.parent.mkdir(parents=True, exist_ok=True)
    for attempt in range(MAX_ATTEMPTS):
        with tempfile.NamedTemporaryFile(
            dir=path.parent, prefix=".speechcore-download-", delete=False
        ) as output:
            temporary = Path(output.name)
        try:
            try:
                download(temporary)
            except TransientDownload:
                if attempt + 1 == MAX_ATTEMPTS:
                    raise
                print(
                    f"SpeechCore transient download failure; retry {attempt + 2}/{MAX_ATTEMPTS}"
                )
                sleep(BACKOFF_SECONDS[attempt])
                continue
            verified(temporary, expected)
            with temporary.open("rb") as source:
                os.fsync(source.fileno())
            try:
                # Same-directory, atomic no-clobber publication; never expose partial bytes.
                os.link(temporary, path)
            except FileExistsError:
                verified(path, expected)
            temporary.unlink()
            directory = os.open(path.parent, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
            print(f"SpeechCore verified artifact cached ({path.stat().st_size} bytes)")
            return "downloaded"
        finally:
            temporary.unlink(missing_ok=True)
    raise ArtifactError("SpeechCore download attempts exhausted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--describe", action="store_true")
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    args = parser.parse_args()
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise ArtifactError(
            "This prewarm entrypoint is restricted to macOS GitHub Actions"
        )
    validate_pins(args.repository)
    swift = subprocess.run(
        ["swift", "package", "--version"],
        capture_output=True,
        text=True,
        timeout=15,
        check=True,
    )
    validate_swiftpm(swift.stdout.strip())
    print(swift.stdout.strip())
    path = Path.home() / "Library/Caches/org.swift.swiftpm/artifacts" / CACHE_NAME
    if args.describe:
        if args.github_output is None:
            raise ArtifactError("Describe requires GitHub output file")
        with args.github_output.open("a") as output:
            output.write(f"path={path}\nkey={CACHE_KEY}\n")
        return
    prepare(path)


if __name__ == "__main__":
    try:
        main()
    except (ArtifactError, OSError, ValueError, subprocess.SubprocessError) as cause:
        print(f"SpeechCore prewarm failed: {cause}", file=sys.stderr)
        sys.exit(1)
