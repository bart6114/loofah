import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from backend_onnx_models import pinned_manifest, provision


class OnnxFixturesTests(unittest.TestCase):
    def test_committed_manifest_has_every_model_asset_and_distinct_digests(self):
        repository = Path(__file__).resolve().parents[2]
        manifest = pinned_manifest(repository)
        self.assertEqual(len(manifest), 8)
        self.assertEqual(
            {asset["path"].split("/")[0] for asset in manifest},
            {"batch", "streaming", "diarizer"},
        )
        self.assertEqual(len({asset["sha256"] for asset in manifest}), 8)
        self.assertGreater(sum(asset["size_bytes"] for asset in manifest), 1024**3)

    def test_read_only_check_rejects_corrupt_cache_without_network_or_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "model.onnx"
            original = b"verified"
            manifest = [
                {
                    "path": path.name,
                    "size_bytes": len(original),
                    "sha256": hashlib.sha256(original).hexdigest(),
                    "url": "https://invalid.test/model",
                }
            ]
            path.write_bytes(original)
            with patch(
                "urllib.request.urlopen", side_effect=AssertionError("No network")
            ):
                provision(root, manifest, check_only=True)
                path.write_bytes(b"corrupt!")
                with self.assertRaises(ValueError):
                    provision(root, manifest, check_only=True)
                self.assertEqual(path.read_bytes(), b"corrupt!")


if __name__ == "__main__":
    unittest.main()
