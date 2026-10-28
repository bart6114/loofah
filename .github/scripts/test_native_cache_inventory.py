from pathlib import Path
import tempfile
import unittest

from native_cache_inventory import inventory


class NativeCacheInventoryTests(unittest.TestCase):
    def test_archive_hit_without_native_outputs_is_not_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scratch = (
                root / "release/build/transcribe-soniqo-abc/out/swift-rs/soniqo-swift"
            )
            scratch.mkdir(parents=True)
            self.assertFalse(inventory([root])[0]["complete"])

    def test_requires_both_nonempty_native_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for unit, profile in (
                ("cli", "release"),
                ("desktop", "arm64-apple-macosx/release"),
            ):
                output = (
                    root
                    / f"release/build/transcribe-soniqo-{unit}/out/swift-rs/soniqo-swift"
                    / profile
                )
                output.mkdir(parents=True)
                (output / "libsoniqo-swift.a").write_bytes(b"library")
                (output / "mlx.metallib").touch()
            self.assertFalse(any(unit["complete"] for unit in inventory([root])))
            for path in root.rglob("mlx.metallib"):
                path.write_bytes(b"metal")
            units = inventory([root])
            self.assertEqual(len(units), 2)
            self.assertTrue(all(unit["complete"] for unit in units))


if __name__ == "__main__":
    unittest.main()
