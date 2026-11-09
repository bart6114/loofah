import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import prepare_speechcore_artifact as artifact


class SpeechCoreArtifactTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.path = self.root / "artifacts" / artifact.CACHE_NAME
        self.body = b"synthetic verified archive"
        self.digest = hashlib.sha256(self.body).hexdigest()
        self.network = patch(
            "socket.create_connection", side_effect=AssertionError("Offline tests")
        )
        self.network.start()
        self.addCleanup(self.network.stop)

    def write_download(self, path):
        self.assertEqual(path.parent, self.path.parent)
        self.assertFalse(
            self.path.exists(), "Final path must stay absent during download"
        )
        path.write_bytes(self.body)

    def fake_curl(self, responses):
        calls = []

        def run(command, **options):
            calls.append(command)
            self.assertEqual(command[:2], ["curl", "--disable"])
            self.assertNotIn("--insecure", command)
            self.assertNotIn("--location", command)
            self.assertEqual(command[command.index("--proto") + 1], "=https")
            self.assertEqual(command[command.index("--retry") + 1], "0")
            self.assertEqual(
                int(command[command.index("--max-filesize") + 1]), 16 * 1024 * 1024
            )
            self.assertLessEqual(options["timeout"], artifact.ATTEMPT_SECONDS + 1)
            code, status, headers, body = responses.pop(0)
            Path(command[command.index("--dump-header") + 1]).write_text(headers)
            Path(command[command.index("--output") + 1]).write_bytes(body)
            return SimpleNamespace(
                returncode=code,
                stdout=str(status),
                stderr="private signed token must not appear",
            )

        return run, calls

    def test_committed_package_pin_and_exact_swiftpm_cache_key(self):
        artifact.validate_pins(Path(__file__).resolve().parents[2])
        self.assertEqual(
            artifact.CACHE_NAME,
            "".join(c if c.isascii() and c.isalnum() else "_" for c in artifact.URL),
        )
        self.assertIn(artifact.SHA256, artifact.CACHE_KEY)
        artifact.validate_swiftpm("Swift Package Manager - Swift 6.3.2-dev")
        artifact.validate_swiftpm("Swift Package Manager - Swift 6.3")
        for version in [
            "Swift Package Manager - Swift 6.4",
            "Swift version 6.3",
            "Swift Package Manager - Swift 16.3",
        ]:
            with self.assertRaises(artifact.ArtifactError):
                artifact.validate_swiftpm(version)

    def test_changed_package_pin_refuses_before_download(self):
        package = self.root / "crates/transcribe-soniqo/swift-lib"
        package.mkdir(parents=True)
        (package / "Package.swift").write_text(
            '.package(url: "https://github.com/soniqo/speech-swift", exact: "0.0.29")'
        )
        pin = {
            "identity": "speech-swift",
            "location": "https://github.com/soniqo/speech-swift",
            "state": {"version": "0.0.29", "revision": "changed"},
        }
        (package / "Package.resolved").write_text(json.dumps({"pins": [pin]}))
        with self.assertRaises(artifact.ArtifactError):
            artifact.validate_pins(self.root)

    def test_verified_cache_hit_has_no_network_or_replacement(self):
        self.path.parent.mkdir()
        self.path.write_bytes(self.body)
        before = self.path.stat()
        self.assertEqual(
            artifact.prepare(
                self.path, self.digest, download=lambda _: self.fail("No network")
            ),
            "cached",
        )
        self.assertEqual(self.path.stat().st_ino, before.st_ino)
        self.assertEqual(self.path.stat().st_mtime_ns, before.st_mtime_ns)

    def test_corrupt_existing_cache_fails_without_replacing_or_downloading(self):
        self.path.parent.mkdir()
        self.path.write_bytes(b"corrupt")
        with self.assertRaisesRegex(artifact.ArtifactError, "checksum"):
            artifact.prepare(
                self.path, self.digest, download=lambda _: self.fail("No network")
            )
        self.assertEqual(self.path.read_bytes(), b"corrupt")

    def test_verified_download_publishes_atomically_and_cleans_temporary_files(self):
        self.assertEqual(
            artifact.prepare(self.path, self.digest, self.write_download), "downloaded"
        )
        self.assertEqual(self.path.read_bytes(), self.body)
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])
        self.assertEqual(self.path.stat().st_nlink, 1)

    def test_racing_foreign_cache_file_is_not_overwritten(self):
        def fetch(path):
            path.write_bytes(self.body)
            self.path.write_bytes(b"foreign replacement")

        with self.assertRaises(artifact.ArtifactError):
            artifact.prepare(
                self.path, self.digest, fetch, lambda _: self.fail("No retry")
            )
        self.assertEqual(self.path.read_bytes(), b"foreign replacement")
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_transient_attempts_are_bounded_and_partial_files_are_removed(self):
        calls, waits = [], []

        def fail(path):
            calls.append(path)
            path.write_bytes(b"partial")
            raise artifact.TransientDownload("HTTP504")

        with self.assertRaises(artifact.TransientDownload):
            artifact.prepare(self.path, self.digest, fail, waits.append)
        self.assertEqual(len(calls), 3)
        self.assertEqual(waits, [3, 9])
        self.assertFalse(self.path.exists())
        self.assertEqual(list(self.path.parent.iterdir()), [])

    def test_transient_then_success_is_verified_before_publication(self):
        calls = []

        def fetch(path):
            calls.append(path)
            if len(calls) == 1:
                path.write_bytes(b"partial")
                raise artifact.TransientDownload("HTTP504")
            self.write_download(path)

        artifact.prepare(self.path, self.digest, fetch, lambda _: None)
        self.assertEqual(len(calls), 2)
        self.assertEqual(self.path.read_bytes(), self.body)

    def test_checksum_and_oversize_failures_do_not_retry(self):
        for oversized in [False, True]:
            calls = []

            def fetch(path):
                calls.append(path)
                path.write_bytes(b"wrong")
                if oversized:
                    with path.open("r+b") as output:
                        output.truncate(artifact.MAX_BYTES + 1)

            with self.assertRaises(artifact.ArtifactError):
                artifact.prepare(
                    self.path, self.digest, fetch, lambda _: self.fail("No retry")
                )
            self.assertEqual(len(calls), 1)
            self.assertEqual(list(self.path.parent.iterdir()), [])

    def test_official_redirect_is_followed_without_leaking_signed_url(self):
        destination = self.root / "partial"
        signed = "https://release-assets.githubusercontent.com/github-production-release-asset/123?sig=secret"
        run, calls = self.fake_curl(
            [(0, 302, "Location: " + signed + "\r\n", b""), (0, 200, "", self.body)]
        )
        artifact.download_attempt(destination, run=run)
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[-1][-1], signed)
        self.assertEqual(destination.read_bytes(), self.body)
        self.assertEqual(list(self.root.iterdir()), [destination])

    def test_redirects_to_nonofficial_http_credentials_and_ports_fail(self):
        for location in [
            "http://github.com/file",
            "https://example.com/file",
            "https://github.com.evil.test/file",
            "https://user@github.com/file",
            "https://github.com:444/file",
        ]:
            run, calls = self.fake_curl(
                [(0, 302, "Location: " + location + "\r\n", b"")]
            )
            with self.assertRaises(artifact.ArtifactError) as caught:
                artifact.download_attempt(self.root / "partial", run=run)
            self.assertEqual(len(calls), 1)
            self.assertNotIn(location, str(caught.exception))

    def test_http_and_curl_failures_have_exact_retry_classification(self):
        for status, expected in [
            (504, artifact.TransientDownload),
            (429, artifact.TransientDownload),
            (404, artifact.ArtifactError),
            (403, artifact.ArtifactError),
        ]:
            run, _ = self.fake_curl([(0, status, "", b"gateway")])
            with self.assertRaises(expected) as caught:
                artifact.download_attempt(self.root / "partial", run=run)
            self.assertEqual(type(caught.exception), expected)
        for code in [7, 28, 56, 35, 60, 77, 63]:
            expected = (
                artifact.TransientDownload
                if code in {7, 28, 56}
                else artifact.ArtifactError
            )
            run, _ = self.fake_curl([(code, 0, "", b"partial")])
            with self.assertRaises(expected) as caught:
                artifact.download_attempt(self.root / "partial", run=run)
            self.assertEqual(type(caught.exception), expected)
            self.assertNotIn("private signed token", str(caught.exception))

    def test_redirect_count_and_total_attempt_deadline_are_bounded(self):
        run, calls = self.fake_curl(
            [(0, 302, "Location: " + artifact.URL + "\n", b"")] * 4
        )
        with self.assertRaises(artifact.ArtifactError):
            artifact.download_attempt(self.root / "partial", run=run)
        self.assertEqual(len(calls), 4)
        times = iter([0, artifact.ATTEMPT_SECONDS + 1])
        with self.assertRaises(artifact.TransientDownload):
            artifact.download_attempt(
                self.root / "partial",
                run=lambda *_a, **_k: self.fail("No request after deadline"),
                clock=lambda: next(times),
            )

    def test_process_timeout_is_transient_and_redacted(self):
        def timeout(*_args, **_kwargs):
            raise subprocess.TimeoutExpired(["curl", "signed-secret"], 46)

        with self.assertRaises(artifact.TransientDownload) as caught:
            artifact.download_attempt(self.root / "partial", run=timeout)
        self.assertNotIn("signed-secret", str(caught.exception))

    def test_symlink_and_hardlink_cache_entries_are_refused(self):
        self.path.parent.mkdir()
        original = self.root / "original"
        original.write_bytes(self.body)
        self.path.symlink_to(original)
        with self.assertRaises(artifact.ArtifactError):
            artifact.prepare(
                self.path, self.digest, download=lambda _: self.fail("No network")
            )
        self.path.unlink()
        self.path.hardlink_to(original)
        with self.assertRaises(artifact.ArtifactError):
            artifact.prepare(
                self.path, self.digest, download=lambda _: self.fail("No network")
            )
        self.assertEqual(original.read_bytes(), self.body)


if __name__ == "__main__":
    unittest.main()
