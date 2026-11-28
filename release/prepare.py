#!/usr/bin/env python3
"""Prepare a signed FlashLink stable release locally. Publication is a separate step."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from urllib.request import urlopen
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(ROOT / "apps"))
from android.build import RELEASE_CERT_SHA256

RELEASES = "https://github.com/RicardoMin/FlashLink/releases"
MANIFEST = "flashlink-update.json"
MAX_MANIFEST = 128 * 1024
NAMES = {
    "windows": {"exe": "FlashLink-Windows-x64.exe", "zip": "FlashLink-Windows-x64.zip"},
    "android": {"apk": "FlashLink-Android-arm64.apk"},
}
LIMITS = {"exe": 1 << 30, "zip": 1 << 30, "apk": 512 << 20}


def version_parts(value):
    if not isinstance(value, str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2}", value):
        raise ValueError(f"Expected a stable numeric X.Y.Z version, got {value!r}")
    parts = tuple(map(int, value.split(".")))
    if max(parts) > 65535:
        raise ValueError("Version components must fit the numeric Windows PE version")
    return parts


def validate_payload(payload):
    if not isinstance(payload, dict) or type(payload.get("schema")) is not int or payload["schema"] != 1:
        raise ValueError("Unsupported update manifest")
    for platform, filenames in NAMES.items():
        part = payload[platform]
        version_parts(part["version"])
        max_build = 65535 if platform == "windows" else 2_100_000_000
        if type(part["build"]) is not int or not 0 < part["build"] <= max_build:
            raise ValueError(f"Invalid {platform} build number")
        if not isinstance(part["notes"], str) or not part["notes"].strip() or len(part["notes"].encode()) > 4096:
            raise ValueError(f"Expected 1–4096 bytes of UTF-8 release notes for {platform}")
        if set(part["assets"]) != set(filenames):
            raise ValueError(f"Incomplete {platform} release")
        for kind, filename in filenames.items():
            file = part["assets"][kind]
            if (file["url"] != f"{RELEASES}/download/v{part['version']}/{filename}"
                    or not re.fullmatch(r"[0-9a-f]{64}", file["sha256"])
                    or type(file["size"]) is not int or not 0 < file["size"] <= LIMITS[kind]):
                raise ValueError(f"Invalid manifest asset: {filename}")


def openssl(*args, data=None):
    return subprocess.run(["openssl", *map(str, args)], input=data, capture_output=True, check=True).stdout


def verify(envelope, public_key):
    if len(envelope) > MAX_MANIFEST:
        raise ValueError("Update manifest exceeds 128 KiB")
    fields = json.loads(envelope)
    payload = base64.b64decode(fields["payload"], validate=True)
    signature = base64.b64decode(fields["signature"], validate=True)
    with tempfile.TemporaryDirectory() as directory:
        sig_path = Path(directory) / "signature"
        sig_path.write_bytes(signature)
        openssl("dgst", "-sha256", "-keyform", "DER", "-verify", public_key, "-signature", sig_path, data=payload)
    parsed = json.loads(payload)
    validate_payload(parsed)
    return parsed


def sign(payload, private_key, public_key):
    validate_payload(payload)
    data = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode()
    signature = openssl("dgst", "-sha256", "-sign", private_key, data=data)
    envelope = json.dumps({"payload": base64.b64encode(data).decode(),
                           "signature": base64.b64encode(signature).decode()}).encode() + b"\n"
    verify(envelope, public_key)  # Match the private key to the verifier already shipped to users.
    return envelope


def apk_identity(apk, version, build):
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk:
        raise ValueError("Set ANDROID_HOME to the SDK used by apps/android/build.py")
    tools = Path(sdk) / "build-tools/36.0.0"
    java = Path(os.environ["JAVA_HOME"]) / "bin/java" if "JAVA_HOME" in os.environ else "java"
    report = subprocess.check_output([java, "-jar", tools / "lib/apksigner.jar", "verify", "--print-certs", apk])
    if re.findall(rb"^Signer #\d+ certificate SHA-256 digest: ([a-fA-F0-9]+)", report, re.M) != [RELEASE_CERT_SHA256.encode()]:
        raise ValueError("APK is not signed with the FlashLink release certificate")
    aapt = tools / ("aapt.exe" if os.name == "nt" else "aapt")
    badging = subprocess.check_output([aapt, "dump", "badging", apk])
    identity = f"package: name='io.github.ricardomin.flashlink' versionCode='{build}' versionName='{version}'"
    if not badging.startswith(identity.encode()) or b"native-code: 'arm64-v8a'" not in badging.splitlines():
        raise ValueError("APK package, version, build or architecture does not match the release")


def file_digest(reader):
    digest = hashlib.sha256()
    for block in iter(lambda: reader.read(1 << 20), b""):
        digest.update(block)
    return digest.hexdigest()


def windows_identity(exe, archive, version, build):
    # The single-file variant starts with the exact PE shipped as FlashLink.exe in the ZIP.
    with zipfile.ZipFile(archive) as package, exe.open("rb") as application:
        names = package.namelist()
        if (sum(item.file_size for item in package.infolist()) > 2 << 30
                or len(set(name.casefold() for name in names)) != len(names)
                or not {"FlashLink.exe", "Sunshine/sunshine.exe", "package-files.json"}.issubset(names)
                or package.testzip() is not None):
            raise ValueError("Incomplete Windows portable archive")
        inventory = json.loads(package.read("package-files.json"))
        if sorted(inventory) != sorted(name for name in names if name != "package-files.json"):
            raise ValueError("Windows package inventory does not match the contents of the ZIP")
        host = package.read("FlashLink.exe")
        pe_offset = int.from_bytes(host[60:64], "little")
        if host[:2] != b"MZ" or host[pe_offset:pe_offset + 6] != b"PE\0\0\x64\x86":
            raise ValueError("Windows host is not an x64 PE executable")
        if application.read(len(host)) != host:
            raise ValueError("Windows EXE and ZIP contain different hosts")
        with package.open("Sunshine/sunshine.exe") as engine:
            if file_digest(engine).encode() not in host:
                raise ValueError("Portable Sunshine differs from the host's pinned digest")
        application.seek(-32, os.SEEK_END)
        engine_size, notices_size, magic = struct.unpack("<QQ16s", application.read(32))
        if (magic != b"FLASHLINK-PACK-1" or len(host) + engine_size + notices_size + 32 != application.tell()
                or not engine_size or not notices_size):
            raise ValueError("Invalid single-file Windows package")
        application.seek(len(host))
        digest = hashlib.sha256()
        remaining = engine_size
        while remaining:
            chunk = application.read(min(1 << 20, remaining))
            if not chunk:
                raise ValueError("Truncated embedded engine")
            digest.update(chunk)
            remaining -= len(chunk)
        if digest.hexdigest().encode() not in host:
            raise ValueError("Embedded engine differs from the host's pinned digest")

    if os.name != "nt":
        raise ValueError("Prepare Windows releases on Windows to verify the executable's PE version")
    command = "$v=[Diagnostics.FileVersionInfo]::GetVersionInfo($env:FLASHLINK_RELEASE_EXE); " \
              "$v.ProductVersion; $v.FileVersion"
    actual = subprocess.check_output(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command],
                                     env={**os.environ, "FLASHLINK_RELEASE_EXE": str(exe.resolve())}, text=True)
    if actual.splitlines() != [version, f"{version}.{build}"]:
        raise ValueError(f"Windows PE version does not match {version} (build {build})")


def asset(path, version, kind):
    size = path.stat().st_size
    if not 0 < size <= LIMITS[kind]:
        raise ValueError(f"Invalid release package size: {path.name}")
    with path.open("rb") as reader:
        digest = file_digest(reader)
    return {"url": f"{RELEASES}/download/v{version}/{path.name}", "size": size, "sha256": digest}


def assemble(tag, versions, previous, notes, source, staging):
    release = tag[1:] if tag.startswith("v") else None
    number = version_parts(release)
    if previous and any(number <= version_parts(previous[key]["version"]) for key in NAMES):
        raise ValueError("Release tag must be newer than the previous stable release")
    manifest = {"schema": 1}
    for platform, names in NAMES.items():
        version, build = versions[platform]["version"], versions[platform]["build"]
        old = previous[platform] if previous else None
        if notes[platform] is None:
            if not old or (old["version"], old["build"]) != (version, build):
                raise ValueError(f"Provide --{platform}-notes for a first or changed {platform} release")
            manifest[platform] = old
            continue
        if version != release or (old and build <= old["build"]):
            raise ValueError(f"{platform} version must match tag and its build number must increase")
        for name in names.values():
            shutil.copyfile(source / name, staging / name)
        if platform == "windows":
            windows_identity(staging / names["exe"], staging / names["zip"], version, build)
        else:
            apk_identity(staging / names["apk"], version, build)
        manifest[platform] = {
            "version": version, "build": build, "notes": notes[platform].read_text(encoding="utf-8-sig").strip(),
            "assets": {kind: asset(staging / name, version, kind) for kind, name in names.items()},
        }
    validate_payload(manifest)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True, help="New stable tag, e.g. v0.4.0")
    parser.add_argument("--windows-notes", type=Path, help="UTF-8 notes; include to release both Windows packages")
    parser.add_argument("--android-notes", type=Path, help="UTF-8 notes; include to release Android")
    parser.add_argument("--first", action="store_true", help="Bootstrap the first published release, supplying both platforms")
    parser.add_argument("--output", type=Path, help="New staging directory (default dist/releases/<tag>)")
    args = parser.parse_args()
    if not args.tag.startswith("v"):
        parser.error("Expected a vX.Y.Z release tag")
    version_parts(args.tag[1:])
    if not args.windows_notes and not args.android_notes:
        parser.error("Supply release notes for at least one platform")
    private_key = os.environ.get("FLASHLINK_UPDATE_SIGNING_KEY")
    if not private_key:
        parser.error("Set FLASHLINK_UPDATE_SIGNING_KEY to the external PEM key")
    public_key = HERE / "update-key.der"
    previous = None
    if not args.first:
        with urlopen(f"{RELEASES}/latest/download/{MANIFEST}", timeout=20) as response:
            previous = verify(response.read(MAX_MANIFEST + 1), public_key)
    versions = json.loads((HERE / "versions.json").read_text(encoding="utf-8"))
    output = args.output or ROOT / "dist/releases" / args.tag
    output = output.resolve()
    if output.exists():
        raise ValueError(f"Staging directory already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        staging = Path(temporary) / "assets"
        staging.mkdir()
        notes = {"windows": args.windows_notes, "android": args.android_notes}
        payload = assemble(args.tag, versions, previous, notes, ROOT / "install", staging)
        (staging / MANIFEST).write_bytes(sign(payload, private_key, public_key))
        staging.rename(output)
    print(f"Verified assets for {args.tag}: {output}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
