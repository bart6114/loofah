import copy
from pathlib import Path
import unittest

from backend_cpu_artifact import (
    ONNX_CASES,
    select_artifact,
    validate_inventory,
    validate_native_features,
)


class BackendCpuArtifactTests(unittest.TestCase):
    def artifact(self):
        return {
            "reason": "compiler-artifact",
            "target": {
                "name": "loofah_server",
                "src_path": "/repository/apps/server/src/lib.rs",
            },
            "profile": {"test": True},
            "features": ["cpu-transcription"],
            "executable": "/repository/target/release/deps/server-test",
            "fresh": False,
        }

    def test_requires_one_own_source_cpu_executable(self):
        artifact = self.artifact()
        self.assertEqual(select_artifact([artifact], Path("/repository")), artifact)
        for messages in ([], [artifact, artifact]):
            with self.assertRaises(ValueError):
                select_artifact(messages, Path("/repository"))
        for key, value in (("features", []), ("executable", None)):
            changed = copy.deepcopy(artifact)
            changed[key] = value
            with self.assertRaises(ValueError):
                select_artifact([changed], Path("/repository"))
        with self.assertRaises(ValueError):
            select_artifact([artifact], Path("/another-checkout"))

    def test_zero_matches_or_ambiguous_case_never_count_as_success(self):
        case = "tests::transcription_host::real_cpu"
        listing = "\n".join(
            f"{name}: test"
            for name in (
                case,
                "transcription_cpu::tests::ownership",
                "config::tests::validation",
            )
        )
        self.assertEqual(len(validate_inventory(listing, case)), 3)
        for changed in (
            "",
            listing.replace(case, "renamed_case"),
            listing + f"\n{case}: test",
        ):
            with self.assertRaises(ValueError):
                validate_inventory(changed, case)
        with self.assertRaises(ValueError):
            validate_inventory(f"{case}: test", case)

    def test_requires_both_real_onnx_cases_and_cpu_only_static_native_graph(self):
        case = "tests::transcription_host::whisper"
        listing = "\n".join(
            f"{name}: test"
            for name in (
                case,
                *ONNX_CASES,
                "transcription_cpu::tests::owner",
                "config::tests::bounds",
            )
        )
        validate_inventory(listing, case, ONNX_CASES)
        with self.assertRaises(ValueError):
            validate_inventory(
                listing.replace(ONNX_CASES[0], "renamed"), case, ONNX_CASES
            )
        messages = [
            {"reason": "compiler-artifact", "target": {"name": name}, "features": []}
            for name in (
                "whisper_local",
                "whisper_rs",
                "whisper_rs_sys",
                "ort",
                "ort_sys",
                "parakeet_rs",
                "transcribe_onnx",
            )
        ]
        self.assertEqual(len(validate_native_features(messages)), 7)
        with self.assertRaises(ValueError):
            validate_native_features(messages[:-1])
        for feature in ("cuda", "load-dynamic", "preload-dylibs", "directml"):
            changed = copy.deepcopy(messages)
            changed[3]["features"] = [feature]
            with self.assertRaises(ValueError):
                validate_native_features(changed)


if __name__ == "__main__":
    unittest.main()
