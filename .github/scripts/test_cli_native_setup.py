import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "actions/cli_native_setup/linux.sh"
DISPATCH = r"""
import hashlib, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
root = pathlib.Path(os.environ['SETUP_FIXTURE'])
if name == 'curl':
    assert '--proto' in sys.argv and '=https' in sys.argv and '--fail' in sys.argv
    destination = pathlib.Path(sys.argv[sys.argv.index('--output') + 1])
    destination.write_bytes(b'fixture archive')
    (root / 'url').write_text(sys.argv[sys.argv.index('--output') - 1])
elif name == 'sha256sum':
    if '--check' in sys.argv:
        checksum, filename = sys.stdin.read().strip().split('  ', 1)
        assert checksum == os.environ['FIXTURE_PIN']
        assert pathlib.Path(filename).read_bytes() == b'fixture archive'
        if os.environ.get('BROKEN_DIGEST'): sys.exit(1)
        (root / 'verified').touch()
    else:
        path = pathlib.Path(sys.argv[1])
        print(hashlib.sha256(path.read_bytes()).hexdigest(), str(path))
elif name == 'unzip':
    assert (root / 'verified').exists(), 'extraction preceded checksum verification'
    archive = next(pathlib.Path(arg) for arg in sys.argv if arg.endswith('.zip'))
    destination = pathlib.Path(sys.argv[sys.argv.index('-d') + 1]) / archive.stem / 'lib'
    destination.mkdir(parents=True)
    (destination / 'libonnxruntime.a').write_bytes(b'static ONNX')
elif name == 'g++-14':
    assert sys.argv[1:] == ['-print-file-name=libstdc++.a']
    print(root / 'libstdc++.a')
else:
    raise AssertionError(name)
"""


class LinuxNativeSetupTests(unittest.TestCase):
    def run_setup(self, target, pin, broken=False):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        (root / "bin").mkdir()
        (root / "libstdc++.a").write_bytes(b"static C++ runtime")
        for command in ("curl", "sha256sum", "unzip", "g++-14"):
            path = root / "bin" / command
            path.write_text(f"#!{sys.executable}\n" + DISPATCH)
            path.chmod(0o755)
        environment = dict(
            os.environ,
            PATH=f"{root / 'bin'}:{os.environ['PATH']}",
            RUNNER_TEMP=str(root / "native"),
            GITHUB_ENV=str(root / "environment"),
            SETUP_FIXTURE=str(root),
            FIXTURE_PIN=pin,
            RUSTFLAGS="--cfg existing_native_flags",
        )
        if broken:
            environment["BROKEN_DIGEST"] = "1"
        result = subprocess.run(
            ["bash", str(SCRIPT), target],
            env=environment,
            capture_output=True,
            text=True,
        )
        return root, result

    def test_both_existing_linux_pins_activate_static_runtime_and_keep_rustflags(self):
        for target, architecture, pin in (
            (
                "x86_64-unknown-linux-gnu",
                "x64",
                "204cd1ea7efdc76057974afe6a97141666fbed8cf5614fc52b305667a621c119",
            ),
            (
                "aarch64-unknown-linux-gnu",
                "aarch64",
                "31c56eaa8c79723555e2284b251e09a9924bf106560a2d6e4e93da6ddf44b4f2",
            ),
        ):
            with self.subTest(target=target):
                root, result = self.run_setup(target, pin)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(
                    f"onnxruntime-linux-{architecture}-static_lib-1.28.0-glibc2_28.zip",
                    (root / "url").read_text(),
                )
                values = dict(
                    line.split("=", 1)
                    for line in (root / "environment").read_text().splitlines()
                )
                self.assertEqual(values["CC"], "gcc-14")
                self.assertEqual(values["CXX"], "g++-14")
                self.assertEqual(values["ORT_CXX_STDLIB"], "static=stdc++")
                self.assertEqual(
                    values[f"CARGO_TARGET_{target.upper().replace('-', '_')}_LINKER"],
                    "gcc-14",
                )
                self.assertTrue(
                    (Path(values["ORT_LIB_PATH"]) / "libonnxruntime.a").is_file()
                )
                self.assertTrue(
                    values["RUSTFLAGS"].startswith(
                        "--cfg existing_native_flags -L native="
                    )
                )
                cxx_hash = hashlib.sha256(b"static C++ runtime").hexdigest()
                self.assertEqual(
                    values["LOOFAH_NATIVE_CACHE"],
                    f"onnxruntime-{pin}-static-cxx-{cxx_hash}",
                )

    def test_checksum_failure_precedes_extraction_and_environment_publication(self):
        root, result = self.run_setup(
            "x86_64-unknown-linux-gnu",
            "204cd1ea7efdc76057974afe6a97141666fbed8cf5614fc52b305667a621c119",
            broken=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((root / "verified").exists())
        self.assertFalse((root / "environment").exists())
        self.assertEqual(list(root.rglob("libonnxruntime.a")), [])

    def test_unsupported_target_precedes_download_and_directory_creation(self):
        root, result = self.run_setup("x86_64-apple-darwin", "unused")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((root / "native").exists())
        self.assertFalse((root / "url").exists())


if __name__ == "__main__":
    unittest.main()
