import unittest

from ci_scope import scope


class ScopeTests(unittest.TestCase):
    def test_frontend_copy(self):
        self.assertEqual(
            scope(["apps/desktop/src/views/summary.tsx"]),
            {
                "frontend": True,
                "desktop_rust": False,
                "windows": False,
                "cli": False,
                "docs": False,
            },
        )

    def test_shared_rust(self):
        self.assertEqual(
            scope(["crates/vault-read/src/lib.rs"]),
            {
                "frontend": False,
                "desktop_rust": True,
                "windows": True,
                "cli": True,
                "docs": False,
            },
        )

    def test_unknown_fails_closed(self):
        self.assertTrue(all(scope(["pnpm-lock.yaml"]).values()))

    def test_docs_only(self):
        self.assertEqual(
            scope(["docs/guide.md", "skills/loofah/SKILL.md"]),
            {
                "frontend": False,
                "desktop_rust": False,
                "windows": False,
                "cli": False,
                "docs": True,
            },
        )


if __name__ == "__main__":
    unittest.main()
