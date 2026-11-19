#!/usr/bin/env python3
"""Verify the iCloud Documents authorization used to sign an Apple bundle."""

import argparse
import datetime
import plistlib
import subprocess


def profile(path):
    return plistlib.loads(
        subprocess.check_output(["security", "cms", "-D", "-i", path])
    )


def verify_entitlements(entitlements, app_id):
    container = "iCloud.io.loofah.capture"
    identifier = entitlements.get(
        "com.apple.application-identifier", entitlements.get("application-identifier")
    )
    if identifier != app_id:
        raise SystemExit(f"Expected app identifier {app_id}")
    for field in ("icloud-container-identifiers", "ubiquity-container-identifiers"):
        if container not in entitlements.get(f"com.apple.developer.{field}", []):
            raise SystemExit(f"Missing {container} in {field}")
    services = entitlements.get("com.apple.developer.icloud-services", [])
    if "CloudDocuments" not in services and "*" not in services:
        raise SystemExit("Missing CloudDocuments service")
    environment = entitlements.get("com.apple.developer.icloud-container-environment")
    if "Production" not in (
        [environment] if isinstance(environment, str) else environment or []
    ):
        raise SystemExit("Missing Production iCloud environment")


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("profile")
parser.add_argument("bundle_id")
parser.add_argument("--app")
args = parser.parse_args()

app_id = f"L8GUHT98B4.{args.bundle_id}"
embedded = profile(args.profile)
if embedded["ExpirationDate"] <= datetime.datetime.now(datetime.timezone.utc).replace(
    tzinfo=None
):
    raise SystemExit("iCloud provisioning profile has expired")
verify_entitlements(embedded["Entitlements"], app_id)

if args.app:
    signed = plistlib.loads(
        subprocess.check_output(["codesign", "-d", "--entitlements", ":-", args.app])
    )
    verify_entitlements(signed, app_id)
