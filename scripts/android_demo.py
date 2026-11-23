#!/usr/bin/env python3
"""Build the arm64 FlashLink demo: Flutter AAR + Rust manager + Moonlight native activities."""

import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile


ROOT = Path(__file__).resolve().parents[1]
PIN = "1b1b100e63e8ffcf1496e324ff7b57ee9f2db2a9"
ORIGIN = "https://github.com/moonlight-stream/moonlight-android.git"
MODULE = ROOT / "apps/flashlink"
OVERLAY = ROOT / "packaging/android"
RUST_TARGET = "aarch64-linux-android"


def run(args, cwd, env=None):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, env=env, check=True)


def pinned_checkout(host, origin):
    if not host.exists():
        run(["git", "clone", "--depth", "1", "--branch", "v12.1", origin, host], ROOT)
        (host / ".git/flashlink-generated").touch()
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=host, text=True).strip()
    if revision != PIN:
        raise ValueError(f"Unexpected Moonlight revision {revision}; expected {PIN}.")


def apply_overlay(host, patch):
    def applicable(*options):
        result = subprocess.run(["git", "apply", "--check", *options, patch], cwd=host,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return result.returncode == 0

    if applicable("--reverse"):
        return True
    if not applicable():
        return False
    run(["git", "apply", patch], host)
    return True


def prepare(output, origin):
    host = output / "moonlight"
    marker = output / ".flashlink-android-demo"
    output.mkdir(parents=True, exist_ok=True)
    if host.exists() and not marker.exists():
        raise ValueError(f"Unmanaged source directory: {host}. Choose another --output.")
    marker.touch()
    patch = OVERLAY / "moonlight-v12.1.patch"
    pinned_checkout(host, origin)
    if not apply_overlay(host, patch):
        # A previous version of this demo or a developer's edits may occupy the original checkout.
        suffix = hashlib.sha256(patch.read_bytes()).hexdigest()[:16]
        host = output / f"moonlight-{suffix}"
        if host.exists() and not (host / ".git/flashlink-generated").is_file():
            raise ValueError(f"Unmanaged source directory: {host}. Choose another --output.")
        pinned_checkout(host, origin)
        if not apply_overlay(host, patch):
            raise ValueError(f"Local changes prevent applying the overlay in {host}. Choose another --output.")
    for adapter in OVERLAY.glob("FlashLink*.java"):
        shutil.copy2(adapter, host / "app/src/main/java/com/limelight" / adapter.name)
    assets = host / "app/src/main/assets/licenses"
    assets.mkdir(parents=True, exist_ok=True)
    shutil.copy2(host / "LICENSE.txt", assets / "Moonlight-GPLv3.txt")
    shutil.copy2(OVERLAY / "NOTICE.txt", assets / "FlashLink-Moonlight.txt")
    return host


def android_ndk():
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk:
        raise ValueError("Set ANDROID_HOME to your Android SDK with platforms;android-36 installed.")
    version = os.environ.get("FLASHLINK_NDK_VERSION", "28.2.13676358")
    ndk = Path(sdk) / "ndk" / version
    compiler = f"{RUST_TARGET}24-clang" + (".cmd" if os.name == "nt" else "")
    paths = list(ndk.glob(f"toolchains/llvm/prebuilt/*/bin/{compiler}"))
    if len(paths) != 1:
        raise ValueError(f"Cannot locate {compiler} in Android NDK {ndk}.")
    return paths[0], version


def flutter_artifact(repo):
    poms = list(repo.glob("**/flutter_release/1/flutter_release-1.pom"))
    if len(poms) != 1:
        raise ValueError(f"Expected one release AAR POM (version 1) under {repo}.")
    pom = ET.parse(poms[0]).getroot()
    coordinates = ":".join(pom.findtext(f"{{*}}{key}") for key in ("groupId", "artifactId", "version"))
    return coordinates, poms[0].with_suffix(".aar")


def check_flutter_payload(compiler, aar, apk):
    # Android may strip ELF symbols when packaging. The GNU build ID survives this step.
    readelf = compiler.parent / ("llvm-readelf.exe" if os.name == "nt" else "llvm-readelf")
    with tempfile.TemporaryDirectory() as directory, zipfile.ZipFile(aar) as library:
        before = Path(directory) / "flutter-module.so"
        after = Path(directory) / "packaged-app.so"
        before.write_bytes(library.read("jni/arm64-v8a/libapp.so"))
        after.write_bytes(apk.read("lib/arm64-v8a/libapp.so"))
        notes = subprocess.check_output([readelf, "--notes", before, after], text=True)
    ids = re.findall(r"Build ID: ([a-fA-F0-9]+)", notes)
    if len(ids) != 2 or ids[0] != ids[1]:
        raise ValueError("The APK does not contain the Dart AOT library from the current Flutter AAR.")
    print(f"Flutter AAR/APK build ID: {ids[0]}")


def build(output, host):
    compiler, version = android_ndk()
    run(["git", "submodule", "update", "--init", "--recursive"], host)
    rust_env = os.environ.copy()
    rust_env["CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"] = str(compiler)
    target = output / "cargo"
    run(["cargo", "build", "--locked", "--release", "--target", RUST_TARGET,
         "--target-dir", target, "-p", "flashlink-client"], ROOT, rust_env)
    abi = host / "app/src/main/jniLibs/arm64-v8a"
    abi.mkdir(parents=True, exist_ok=True)
    shutil.copy2(target / RUST_TARGET / "release/libflashlink_client.so", abi)

    flutter = os.environ.get("FLUTTER_BIN") or shutil.which("flutter")
    if not flutter:
        raise ValueError("Set FLUTTER_BIN to the Flutter 3.38.9 executable or add it to PATH.")
    run([flutter, "pub", "get"], MODULE)
    run([flutter, "build", "aar", "--no-debug", "--no-profile", "--build-number=1",
         "--target-platform=android-arm64"], MODULE)
    repo = MODULE / "build/host/outputs/repo"
    artifact, aar = flutter_artifact(repo)
    gradle = host / ("gradlew.bat" if os.name == "nt" else "gradlew")
    run([gradle, ":app:assembleNonRootDebug", f"-PflashlinkNdkVersion={version}",
         f"-PflashlinkFlutterArtifact={artifact}", f"-PflashlinkFlutterRepo={repo}"], host)
    apk = host / "app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk"
    with zipfile.ZipFile(apk) as package:
        required = {"libflutter.so", "libapp.so", "libflashlink_client.so", "libmoonlight-core.so"}
        missing = [name for name in required if f"lib/arm64-v8a/{name}" not in package.namelist()]
        if missing:
            raise ValueError("Incomplete APK. Missing native libraries: " + ", ".join(missing))
        check_flutter_payload(compiler, aar, package)
    destination = output / "flashlink-arm64-debug.apk"
    shutil.copy2(apk, destination)
    print(f"APK: {destination}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare-only", action="store_true",
                        help="Apply and verify integration against the pinned host; skip compilation.")
    parser.add_argument("--upstream", default=ORIGIN,
                        help="Clone source (may be a local Moonlight git mirror containing tag v12.1).")
    parser.add_argument("--output", type=Path, default=ROOT / "build/android-demo")
    args = parser.parse_args()
    output = args.output.resolve()
    host = prepare(output, args.upstream)
    if args.prepare_only:
        print(f"Patched Moonlight {PIN} in {host}")
    else:
        build(output, host)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
