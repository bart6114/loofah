#!/usr/bin/env python3
"""Disposable Ubuntu-only packaged/systemd/Caddy acceptance (requires sudo).

Build the default apps/server/Dockerfile image and the deployment integration
test executable first. Pass that executable with --driver; this script never
invokes Cargo or installs trust. Its output directory and persistent service
roots are retained on success AND failure. Only uniquely owned services stop.

Example: sudo python3 .github/scripts/backend_deployment_acceptance.py \
    --source-root "$PWD" --driver /absolute/deployment-test-executable \
    --server-image loofah-server:deployment --caddy-image caddy:2.10
The tested server is extracted unchanged from the default image and runs under
the documented hardened systemd unit on Ubuntu, not inside the Debian image.
"""

import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import signal
import socket
import ssl
import stat
import subprocess
import threading
import time
import urllib.request
import uuid


LIMIT = 64 * 1024


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def save(path, value):
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def substitute(template, replacements):
    result = template
    for old, new in replacements:
        if result.count(old) != 1:
            raise ValueError(f"documented template changed: {old!r}")
        result = result.replace(old, new, 1)
    return result


def service_unit(template, name, user, root):
    substitutions = [
        ("User=loofah", f"User={user}"),
        ("Group=loofah", f"Group={user}"),
        (
            "EnvironmentFile=-/etc/loofah/server.env",
            f"EnvironmentFile=-{root}/server.env",
        ),
        (
            "ExecStart=/usr/local/bin/loofah-server --config /etc/loofah/server.json",
            f"ExecStart={root}/bin/loofah-server --config {root}/server.json",
        ),
        ("StateDirectory=loofah", f"StateDirectory={name}"),
        ("CacheDirectory=loofah", f"CacheDirectory={name}"),
        (
            "ReadWritePaths=/var/lib/loofah /var/cache/loofah",
            f"ReadWritePaths=/var/lib/{name} /var/cache/{name}",
        ),
    ]
    return substitute(template, substitutions), substitutions


def caddy_config(template, origin, backend, overlay, root):
    substitutions = [
        ("{$LOOFAH_DOMAIN}", origin),
        ("127.0.0.1:8027", backend),
        (
            "    reverse_proxy",
            "    tls "
            + str(root / "tls/localhost.pem")
            + " "
            + str(root / "tls/localhost.key")
            + "\n"
            + "    handle /api/v1/capabilities {\n"
            + f"        reverse_proxy {overlay}\n    }}\n"
            + "    handle {\n    reverse_proxy",
        ),
    ]
    body = substitute(template, substitutions)
    # The final brace closes the documented site; close the added catch-all first.
    offset = body.rfind("}")
    body = body[:offset] + "    }\n" + body[offset:]
    return (
        "{\n    admin off\n    auto_https disable_redirects\n}\n" + body,
        substitutions,
    )


def capabilities_overlay(raw):
    """Insert exactly one feature without reserializing any original bytes."""
    if len(raw) > LIMIT:
        raise ValueError("capabilities body exceeds fixture limit")
    value = json.loads(raw)
    features = value.get("features")
    if not isinstance(features, list) or not all(isinstance(v, str) for v in features):
        raise ValueError("invalid original features")
    if "capture_append" in features:
        raise ValueError("production capture_append must remain unadvertised")
    # Locate the actual top-level key rather than matching a nested/string value.
    decoder = json.JSONDecoder()
    text = raw.decode("utf-8")
    position = text.index("{") + 1
    insertion = None
    while True:
        while text[position].isspace() or text[position] == ",":
            position += 1
        if text[position] == "}":
            break
        key, position = decoder.raw_decode(text, position)
        while text[position].isspace():
            position += 1
        if text[position] != ":":
            raise ValueError("invalid capabilities object")
        position += 1
        while text[position].isspace():
            position += 1
        _, end = decoder.raw_decode(text, position)
        if key == "features":
            if insertion is not None:
                raise ValueError("duplicate features")
            insertion = end - 1
        position = end
    if insertion is None:
        raise ValueError("features missing")
    addition = ("," if features else "") + '"capture_append"'
    output = (text[:insertion] + addition + text[insertion:]).encode()
    expected = dict(value)
    expected["features"] = [*features, "capture_append"]
    if json.loads(output) != expected:
        raise ValueError("fixture changed other capability fields")
    return output


def configuration(storage, cache, origin, backend, issuer):
    return {
        "storage_root": str(storage),
        "cache_root": str(cache),
        "public_url": origin,
        "listen": backend,
        "oidc": {
            "issuer": issuer,
            "audience": "loofah-api",
            "client_id": "loofah-desktop",
            "required_scope": "loofah:access",
        },
        "allow_insecure_loopback": True,
    }


def inventory(root):
    values = []
    for path in sorted(root.rglob("*")):
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode) or not (path.is_dir() or path.is_file()):
            raise RuntimeError(f"unexpected service filesystem entry: {path}")
        item = {
            "path": str(path.relative_to(root)),
            "uid": info.st_uid,
            "mode": oct(stat.S_IMODE(info.st_mode)),
        }
        if path.is_file():
            item.update(bytes=info.st_size, sha256=sha(path))
        values.append(item)
    return values


def available_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def request(url, context=None):
    with urllib.request.urlopen(url, timeout=3, context=context) as response:
        body = response.read(LIMIT + 1)
        if len(body) > LIMIT:
            raise RuntimeError("HTTP fixture response exceeds bound")
        return response.status, body


def wait_for(predicate, description, driver=None, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        if driver is not None and driver.poll() is not None:
            raise RuntimeError(
                f"driver exited {driver.returncode} while waiting for {description}"
            )
        time.sleep(0.05)
    raise TimeoutError(description)


def marker(root, name, driver):
    def read():
        try:
            return json.loads((root / name).read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            return None

    return wait_for(read, name, driver)


class Overlay(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path != "/api/v1/capabilities":
            self.send_error(404)
            return
        try:
            status, raw = request(self.server.backend + self.path)
            if status != 200:
                raise RuntimeError("original discovery failed")
            body = capabilities_overlay(raw)
        except Exception:
            self.send_error(502, "fixture discovery unavailable")
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class DiscoveryServer(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address):
        self.capacity = threading.BoundedSemaphore(8)
        super().__init__(address, Overlay)

    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(5)
        return connection, address

    def process_request(self, connection, address):
        if not self.capacity.acquire(blocking=False):
            self.shutdown_request(connection)
            return
        try:
            super().process_request(connection, address)
        except BaseException:
            self.capacity.release()
            raise

    def process_request_thread(self, connection, address):
        try:
            super().process_request_thread(connection, address)
        finally:
            self.capacity.release()


def run(args):
    if os.geteuid() != 0 or not Path("/run/systemd/system").is_dir():
        raise RuntimeError("run with sudo on a disposable systemd Ubuntu host")
    if "Ubuntu" not in Path("/etc/os-release").read_text():
        raise RuntimeError("this acceptance is scoped to disposable Ubuntu")
    source = args.source_root.resolve(strict=True)
    driver_source = args.driver.resolve(strict=True)
    uid, gid = (
        int(os.environ.get("SUDO_UID", "0")),
        int(os.environ.get("SUDO_GID", "0")),
    )
    if uid == 0:
        raise RuntimeError("invoke through sudo from a non-root CI runner")
    suffix = uuid.uuid4().hex[:12]
    name, user = f"loofah-deployment-{suffix}", f"loofahqa{suffix}"
    root = Path("/opt") / name
    storage, cache = Path("/var/lib") / name, Path("/var/cache") / name
    unit_path = Path("/etc/systemd/system") / (name + ".service")
    for path in (root, storage, cache, unit_path):
        if path.exists() or path.is_symlink():
            raise RuntimeError(f"refusing existing path: {path}")
    try:
        pwd.getpwnam(user)
    except KeyError:
        pass
    else:
        raise RuntimeError("service user already exists")
    print(f"LOOFAH_DEPLOYMENT_EVIDENCE={root}", flush=True)
    os.umask(0o022)
    root.mkdir(mode=0o711)
    (root / "bin").mkdir(mode=0o755)
    control = root / "control"
    control.mkdir(mode=0o700)
    os.chown(control, uid, gid)
    report = {
        "schema": 1,
        "root": str(root),
        "unit": name + ".service",
        "production_sso": False,
        "inference": False,
        "physical_capture": False,
        "runtime": "default-image executable under documented systemd on Ubuntu",
        "started_at": time.time(),
        "passed": False,
        "commands": [],
    }
    driver = caddy = overlay = None
    unit_owned = False

    def command(argv, check=True):
        completed = subprocess.run(
            argv,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=150,
        )
        report["commands"].append(
            {"argv": argv, "exit": completed.returncode, "output": completed.stdout}
        )
        if check and completed.returncode:
            raise RuntimeError(f"command failed: {argv[0]} {argv[1:3]}")
        return completed.stdout

    def service(action):
        return command(["systemctl", action, name + ".service"])

    def properties():
        raw = command(
            [
                "systemctl",
                "show",
                name + ".service",
                "--property=MainPID,Result,ExecMainStatus,ExecMainCode,ActiveState,SubState,User,Group",
            ]
        )
        return dict(line.split("=", 1) for line in raw.splitlines() if "=" in line)

    def ready(origin, context=None):
        try:
            return request(origin + "/ready", context)[0] == 204
        except (OSError, ValueError):
            return False

    try:
        command(
            [
                "useradd",
                "--system",
                "--user-group",
                "--no-create-home",
                "--shell",
                "/usr/sbin/nologin",
                user,
            ]
        )
        account = pwd.getpwnam(user)
        report["service_uid"] = account.pw_uid
        report["runner_uid"] = uid
        report["source_commit"] = command(
            [
                "git",
                "-c",
                "safe.directory=" + str(source),
                "-C",
                str(source),
                "rev-parse",
                "HEAD",
            ]
        ).strip()
        report["source_dirty"] = command(
            [
                "git",
                "-c",
                "safe.directory=" + str(source),
                "-C",
                str(source),
                "status",
                "--porcelain",
            ]
        ).splitlines()
        report["images"] = {}
        for image, binary, destination in (
            (args.server_image, "/usr/local/bin/loofah-server", "loofah-server"),
            (args.caddy_image, "/usr/bin/caddy", "caddy"),
        ):
            description = json.loads(command(["docker", "image", "inspect", image]))[0]
            report["images"][destination] = {
                "id": description["Id"],
                "reference": image,
                "configured_user": description["Config"]["User"],
            }
            if (
                destination == "loofah-server"
                and description["Config"]["User"] != "10001:10001"
            ):
                raise RuntimeError(
                    "default packaged image must retain its non-root user"
                )
            container = command(["docker", "create", image]).strip()
            if not re.fullmatch(r"[a-f0-9]{64}", container):
                raise RuntimeError("unexpected owned extraction container ID")
            try:
                command(
                    [
                        "docker",
                        "cp",
                        container + ":" + binary,
                        str(root / "bin" / destination),
                    ]
                )
            finally:
                command(["docker", "rm", container])
            os.chmod(root / "bin" / destination, 0o755)
        shutil.copyfile(driver_source, root / "bin/deployment-test")
        os.chmod(root / "bin/deployment-test", 0o755)
        report["artifacts"] = {
            name: {"sha256": sha(path), "bytes": path.stat().st_size}
            for name, path in ((p.name, p) for p in (root / "bin").iterdir())
        }
        command([str(root / "bin/loofah-server"), "--version"])
        listing = command([str(root / "bin/deployment-test"), "--list"])
        if listing.splitlines().count("packaged_caddy_append_and_restart: test") != 1:
            raise RuntimeError("exact deployment test must exist once")
        tls = root / "tls"
        tls.mkdir(mode=0o711)
        fixtures = source / "crates/loofah-client/tests/fixtures"
        for filename in ("localhost.pem", "localhost.key", "localhost-ca.pem"):
            shutil.copyfile(fixtures / filename, tls / filename)
            os.chmod(tls / filename, 0o600 if filename.endswith(".key") else 0o644)
            os.chown(tls / filename, account.pw_uid, account.pw_gid)
        context = ssl.create_default_context(cafile=str(tls / "localhost-ca.pem"))
        env = dict(os.environ, LOOFAH_DEPLOYMENT_CONTROL=str(control))
        with (root / "driver.log").open("xb") as output:
            driver = subprocess.Popen(
                [
                    str(root / "bin/deployment-test"),
                    "--ignored",
                    "--exact",
                    "packaged_caddy_append_and_restart",
                    "--nocapture",
                ],
                env=env,
                cwd=control,
                stdout=output,
                stderr=subprocess.STDOUT,
                user=uid,
                group=gid,
                extra_groups=[],
            )
        issuer = marker(control, "issuer-ready.json", driver)["issuer"]
        if not re.fullmatch(r"http://127\.0\.0\.1:[0-9]+", issuer):
            raise RuntimeError("issuer must be this fixture's loopback listener")
        backend = f"127.0.0.1:{available_port()}"
        tls_port = available_port()
        while backend.endswith(":" + str(tls_port)):
            tls_port = available_port()
        origin = f"https://127.0.0.1:{tls_port}"
        config = configuration(storage, cache, origin, backend, issuer)
        save(root / "server.json", config)
        (root / "server.env").write_text("RUST_LOG=info\n")
        unit, changes = service_unit(
            (source / "apps/server/deploy/loofah-server.service").read_text(),
            name,
            user,
            root,
        )
        with unit_path.open("x") as unit_file:
            unit_file.write(unit)
        unit_owned = True
        (root / "server.service").write_text(unit)
        report["systemd_substitutions"] = changes
        command(["systemctl", "daemon-reload"])
        service("start")
        wait_for(lambda: ready("http://" + backend), "upstream ready", driver)
        first = properties()
        pid = int(first["MainPID"])
        process = Path(f"/proc/{pid}/status").read_text()
        actual_uids = re.search(r"^Uid:\s+(.+)$", process, re.M).group(1).split()
        assert set(actual_uids) == {str(account.pw_uid)} and account.pw_uid != 0
        for path in (storage, cache):
            assert stat.S_IMODE(path.stat().st_mode) == 0o700
            assert path.stat().st_uid == account.pw_uid
        report["persistent_roots"] = [
            {
                "path": str(path),
                "mode": oct(stat.S_IMODE(path.stat().st_mode)),
                "uid": path.stat().st_uid,
            }
            for path in (storage, cache)
        ]
        report["initial_service"] = first
        report["actual_process_uids"] = actual_uids
        raw = request("http://" + backend + "/api/v1/capabilities")[1]
        overlaid = capabilities_overlay(raw)
        if set(json.loads(raw)["features"]) & {
            "capture_processing",
            "capture_live_transcription",
            "batch_transcription_jobs",
        }:
            raise RuntimeError("default deployment must not configure inference")
        (root / "capabilities-production.json").write_bytes(raw)
        (root / "capabilities-fixture.json").write_bytes(overlaid)
        report["overlay"] = {
            "only_added_feature": "capture_append",
            "original_sha256": sha(root / "capabilities-production.json"),
            "fixture_sha256": sha(root / "capabilities-fixture.json"),
        }
        overlay = DiscoveryServer(("127.0.0.1", 0))
        overlay.backend = "http://" + backend
        threading.Thread(target=overlay.serve_forever, daemon=True).start()
        caddyfile, changes = caddy_config(
            (source / "apps/server/deploy/Caddyfile").read_text(),
            origin,
            backend,
            f"127.0.0.1:{overlay.server_port}",
            root,
        )
        (root / "Caddyfile").write_text(caddyfile)
        report["caddy_substitutions"] = changes
        report["caddy_fixture_additions"] = [
            "admin off",
            "auto_https disable_redirects",
            "explicit committed test certificate (no trust installation)",
            "exact capabilities-only overlay; all other paths use documented streaming proxy",
        ]
        caddy_home = root / "caddy-data"
        caddy_home.mkdir(mode=0o700)
        os.chown(caddy_home, account.pw_uid, account.pw_gid)
        caddy_env = dict(
            os.environ,
            HOME=str(caddy_home),
            XDG_DATA_HOME=str(caddy_home),
            XDG_CONFIG_HOME=str(caddy_home),
        )
        with (root / "caddy.log").open("xb") as output:
            caddy = subprocess.Popen(
                [
                    str(root / "bin/caddy"),
                    "run",
                    "--config",
                    str(root / "Caddyfile"),
                    "--adapter",
                    "caddyfile",
                ],
                cwd=root,
                env=caddy_env,
                stdout=output,
                stderr=subprocess.STDOUT,
                user=account.pw_uid,
                group=account.pw_gid,
                extra_groups=[],
            )
        wait_for(lambda: ready(origin, context), "Caddy TLS ready", driver)
        assert request(origin + "/api/v1/capabilities", context)[1] == overlaid
        save(control / "service-ready.json", {"origin": origin})
        marker(control, "restart-request.json", driver)
        service("stop")
        stopped = properties()
        assert stopped["Result"] == "success" and stopped["ExecMainStatus"] == "0"
        assert not Path(f"/proc/{pid}").exists()
        report["first_sigterm"] = stopped
        save(root / "inventory-before-restart.json", inventory(storage))
        save(control / "service-stopped.json", {"exit": 0})
        marker(control, "restart-continue.json", driver)
        service("start")
        wait_for(lambda: ready(origin, context), "restarted TLS ready", driver)
        assert request("http://" + backend + "/api/v1/capabilities")[1] == raw
        assert request(origin + "/api/v1/capabilities", context)[1] == overlaid
        second = properties()
        assert int(second["MainPID"]) != pid
        report["restarted_service"] = second
        save(control / "service-restarted.json", {"ready": True})
        result = marker(control, "driver-result.json", driver)
        assert result["passed"] is True
        assert driver.wait(timeout=30) == 0
        report["driver"] = result
        service("stop")
        final = properties()
        assert final["Result"] == "success" and final["ExecMainStatus"] == "0"
        assert not Path(f"/proc/{second['MainPID']}").exists()
        report["final_sigterm"] = final
        final_inventory = inventory(storage)
        assert all(int(item["mode"], 8) & 0o077 == 0 for item in final_inventory)
        assert all(item["uid"] == account.pw_uid for item in final_inventory)
        save(root / "inventory-final.json", final_inventory)
        cache_inventory = inventory(cache)
        assert all(int(item["mode"], 8) & 0o077 == 0 for item in cache_inventory)
        assert all(item["uid"] == account.pw_uid for item in cache_inventory)
        save(root / "cache-inventory-final.json", cache_inventory)
        report["passed"] = True
    except BaseException as error:
        report["failure"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        # Preserve all evidence/storage. Never stop an unrelated unit or process.
        stopped_owned_unit = False
        if unit_owned:
            try:
                service("stop")
                stopped_owned_unit = properties()["ActiveState"] == "inactive"
                if not stopped_owned_unit:
                    raise RuntimeError("owned unit did not become inactive")
                (root / "server-journal.log").write_text(
                    command(
                        [
                            "journalctl",
                            "--unit",
                            name + ".service",
                            "--no-pager",
                            "--output=short-iso",
                        ]
                    )
                )
            except Exception as error:
                report["cleanup_error"] = str(error)
                report["passed"] = False
        for process in (driver, caddy):
            if process is not None and process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    report["cleanup_error"] = "owned child did not drain after SIGTERM"
                    report["passed"] = False
                    process.kill()
                    process.wait(timeout=10)
        if caddy is not None:
            report["caddy_exit"] = caddy.poll()
            if caddy.returncode != 0:
                report["passed"] = False
        if overlay is not None:
            overlay.shutdown()
            overlay.server_close()
        if stopped_owned_unit:
            unit_path.unlink()
            command(["systemctl", "daemon-reload"])
        report["finished_at"] = time.time()
        report["evidence_hashes"] = {
            str(p.relative_to(root)): sha(p) for p in root.glob("*.log")
        }
        save(root / "acceptance.json", report)
        print(f"LOOFAH_DEPLOYMENT_PASSED={report['passed']}", flush=True)
    if not report["passed"]:
        raise RuntimeError("deployment or owned teardown failed; evidence retained")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--server-image", default="loofah-server:deployment")
    parser.add_argument("--caddy-image", default="caddy:2.10")
    run(parser.parse_args())
