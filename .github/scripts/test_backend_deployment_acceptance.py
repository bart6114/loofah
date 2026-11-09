import json
from pathlib import Path
import tempfile
import unittest

from backend_deployment_acceptance import (
    capabilities_overlay,
    caddy_config,
    configuration,
    inventory,
    owned_listeners,
    require_private_listener,
    service_unit,
    substitute,
)


SOURCE = Path(__file__).resolve().parents[2]


class DeploymentAcceptanceTests(unittest.TestCase):
    def socket_tables(self, rows, ipv6=()):
        header = "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n"

        def table(values):
            return header + "\n".join(
                f"{index}: {address}:{port:04X} 00000000:0000 {state} 00000000:00000000 00:00000000 00000000 1001 0 {inode} 1"
                for index, (address, port, state, inode) in enumerate(values)
            )

        return {"tcp": table(rows), "tcp6": table(ipv6)}

    def test_owned_listener_excludes_other_processes_and_connected_sockets(self):
        tables = self.socket_tables(
            [
                ("0100007F", 18027, "0A", 501),
                ("00000000", 80, "0A", 900),
                ("0100007F", 18027, "01", 502),
            ]
        )
        listeners = owned_listeners(tables, {501, 502}, "little")
        require_private_listener(listeners, "127.0.0.1:18027")
        self.assertEqual(len(listeners), 1)
        self.assertEqual(listeners[0]["inode"], 501)
        self.assertIn("0100007F:466B", listeners[0]["raw"])

    def test_owned_wildcard_ipv6_extra_missing_and_wrong_port_listeners_refused(self):
        base = ("0100007F", 18027, "0A", 501)
        variants = [
            self.socket_tables([("00000000", 18027, "0A", 501)]),
            self.socket_tables(
                [], [("00000000000000000000000000000000", 18027, "0A", 501)]
            ),
            self.socket_tables(
                [], [("00000000000000000000000001000000", 18027, "0A", 501)]
            ),
            self.socket_tables([base, ("0100007F", 18028, "0A", 502)]),
            self.socket_tables(
                [base], [("00000000000000000000000001000000", 18028, "0A", 502)]
            ),
            self.socket_tables([]),
            self.socket_tables([("0100007F", 18028, "0A", 501)]),
        ]
        for tables in variants:
            with self.subTest(tables=tables), self.assertRaises(RuntimeError):
                require_private_listener(
                    owned_listeners(tables, {501, 502}, "little"), "127.0.0.1:18027"
                )

    def test_proc_parser_rejects_malformed_owned_listener_and_handles_native_words(
        self,
    ):
        for tables in (
            {"tcp": "not proc data"},
            self.socket_tables([("BAD", 80, "0A", 501)]),
        ):
            with self.assertRaises(ValueError):
                owned_listeners(tables, {501}, "little")
        tables = self.socket_tables([("7F000001", 18027, "0A", 501)])
        require_private_listener(
            owned_listeners(tables, {501}, "big"), "127.0.0.1:18027"
        )
        for endpoint in ("0.0.0.0:18027", "127.0.0.1:0", "[::1]:18027"):
            with self.assertRaises(ValueError):
                require_private_listener([], endpoint)

    def test_real_unit_substitutions_preserve_every_hardening_directive(self):
        original = (SOURCE / "apps/server/deploy/loofah-server.service").read_text()
        actual, changes = service_unit(
            original,
            "loofah-deployment-123",
            "loofahqa123",
            Path("/opt/loofah-deployment-123"),
        )
        for old, new in reversed(changes):
            actual = actual.replace(new, old, 1)
        self.assertEqual(actual, original)
        for directive in (
            "NoNewPrivileges=true",
            "ProtectSystem=strict",
            "ProtectHome=true",
            "PrivateTmp=true",
            "TimeoutStopSec=60",
            "UMask=0077",
            "StateDirectoryMode=0700",
            "CacheDirectoryMode=0700",
        ):
            self.assertIn(directive, actual)

    def test_changed_templates_refuse_silent_partial_replacement(self):
        for original in ("", "one one"):
            with self.assertRaises(ValueError):
                substitute(original, [("one", "two")])

    def test_caddy_preserves_streaming_and_limits_overlay_to_exact_discovery(self):
        original = (SOURCE / "apps/server/deploy/Caddyfile").read_text()
        actual, _ = caddy_config(
            original,
            "https://127.0.0.1:14443",
            "127.0.0.1:18027",
            "127.0.0.1:18028",
            Path("/opt/test"),
        )
        self.assertIn("flush_interval -1", actual)
        self.assertIn("handle /api/v1/capabilities {", actual)
        self.assertIn("handle {\n    reverse_proxy 127.0.0.1:18027", actual)
        self.assertEqual(actual.count("{"), actual.count("}"))
        self.assertNotIn("tls internal", actual)
        self.assertNotIn("skip_verify", actual)
        self.assertIn("admin off", actual)

    def test_overlay_preserves_original_bytes_and_unknown_fields(self):
        for features in ([], ["notes", "recording_leases"]):
            raw = json.dumps(
                {
                    "future": {"features": ["do not change"]},
                    "message": 'quoted "features":[]',
                    "features": features,
                    "api_major": 1,
                },
                indent=2,
            ).encode()
            actual = capabilities_overlay(raw)
            added = (b"," if features else b"") + b'"capture_append"'
            self.assertEqual(actual.replace(added, b"", 1), raw)
            expected = json.loads(raw)
            expected["features"].append("capture_append")
            self.assertEqual(json.loads(actual), expected)
        unicode_raw = '{"name":"café","features":[]}'.encode()
        self.assertEqual(
            capabilities_overlay(unicode_raw),
            '{"name":"café","features":["capture_append"]}'.encode(),
        )

    def test_overlay_refuses_activation_ambiguity_and_unbounded_bodies(self):
        for raw in (
            b'{"features":["capture_append"]}',
            b'{"features":null}',
            b'{"features":[],"features":[]}',
            b"{}",
            b'{"features":[1]}',
            b" " * (64 * 1024 + 1),
        ):
            with self.assertRaises((ValueError, KeyError)):
                capabilities_overlay(raw)

    def test_config_has_only_loopback_identity_no_model_or_provider(self):
        config = configuration(
            Path("/var/lib/test"),
            Path("/var/cache/test"),
            "https://127.0.0.1:14443",
            "127.0.0.1:18027",
            "http://127.0.0.1:18028",
        )
        self.assertNotIn("processing", config)
        self.assertNotIn("transcription", config)
        self.assertTrue(config["allow_insecure_loopback"])
        self.assertEqual(config["storage_root"], "/var/lib/test")
        self.assertNotEqual(config["storage_root"], config["cache_root"])

    def test_inventory_hashes_hidden_files_and_refuses_links_without_following(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / ".proof").write_bytes(b"original")
            before = inventory(root)
            self.assertEqual(before[0]["path"], ".proof")
            self.assertEqual(before[0]["bytes"], 8)
            (root / ".proof").write_bytes(b"changed!")
            self.assertNotEqual(inventory(root)[0]["sha256"], before[0]["sha256"])
            (root / "link").symlink_to(".proof")
            with self.assertRaises(RuntimeError):
                inventory(root)


if __name__ == "__main__":
    unittest.main()
