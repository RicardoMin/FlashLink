#!/usr/bin/env python3
"""Build the arm64 Android app with Flutter, the Rust session manager and Moonlight."""

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


ANDROID = Path(__file__).resolve().parent
ROOT = ANDROID.parents[1]
PIN = "1b1b100e63e8ffcf1496e324ff7b57ee9f2db2a9"
COMMON_PIN = "8af4562af672dd6b9ed28553ead172984fd9a683"
COMMON = Path("app/src/main/jni/moonlight-core/moonlight-common-c")
ORIGIN = "https://github.com/moonlight-stream/moonlight-android.git"
MODULE = ROOT / "apps/flashlink"
RUST_TARGET = "aarch64-linux-android"


def run(args, cwd, env=None):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, env=env, check=True)


def check_revision(host, expected=PIN):
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=host, text=True).strip()
    if revision != expected:
        raise ValueError(f"Unexpected Moonlight revision {revision}; expected {expected}.")


def apply_patch(host, patch, *options):
    # Git for Windows can check patch files out as CRLF. A blank context line
    # must end in LF for "git apply" to recognize the next hunk.
    subprocess.run(["git", "apply", *options, "-"], cwd=host, input=patch, check=True)


def prepare(output, origin):
    android_patch, bridge_patch, common_patch = [
        (ANDROID / name).read_bytes().replace(b"\r\n", b"\n") for name in
        ("moonlight-v12.1.patch", "moonlight-live-bitrate.patch", "moonlight-common-live-bitrate.patch")]
    suffix = hashlib.sha256(android_patch + bridge_patch + common_patch).hexdigest()[:16]
    host = output / f"moonlight-{PIN[:8]}-{suffix}"
    output.mkdir(parents=True, exist_ok=True)
    if host.exists():
        if not (host / ".git/flashlink-generated").is_file():
            raise ValueError(f"Unmanaged source directory: {host}. Choose another --output.")
        check_revision(host)
        check_revision(host / COMMON, COMMON_PIN)
        apply_patch(host, android_patch, "--check", "--reverse")
        apply_patch(host, bridge_patch, "--check", "--reverse")
        apply_patch(host / COMMON, common_patch, "--check", "--reverse")
    else:
        with tempfile.TemporaryDirectory(dir=output) as tmp:
            checkout = Path(tmp) / "moonlight"
            run(["git", "clone", "--depth", "1", "--branch", "v12.1", origin, checkout], ROOT)
            check_revision(checkout)
            run(["git", "submodule", "update", "--init", "--recursive"], checkout)
            common = checkout / COMMON
            check_revision(common, COMMON_PIN)
            apply_patch(checkout, android_patch)
            apply_patch(checkout, bridge_patch)
            apply_patch(common, common_patch)
            (checkout / ".git/flashlink-generated").touch()
            checkout.rename(host)
    assets = host / "app/src/main/assets/licenses"
    assets.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "LICENSE", assets / "FlashLink-GPLv3.txt")
    shutil.copy2(ROOT / "LICENSES/FlashLink-MIT.txt", assets)
    shutil.copy2(host / "LICENSE.txt", assets / "Moonlight-GPLv3.txt")
    shutil.copy2(ANDROID / "NOTICE.txt", assets / "FlashLink-Moonlight.txt")
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


def copy_apk(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as staging:
        ready = Path(staging) / destination.name
        shutil.copy2(source, ready)
        ready.replace(destination)
    print(f"APK: {destination}")


def build(output, host):
    compiler, version = android_ndk()
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
    run([flutter, "build", "aar", "--no-debug", "--no-profile", "--build-number=1",
         "--target-platform=android-arm64"], MODULE)
    repo = MODULE / "build/host/outputs/repo"
    artifact, aar = flutter_artifact(repo)
    gradle = host / ("gradlew.bat" if os.name == "nt" else "gradlew")
    run([gradle, ":app:assembleNonRootDebug", f"-PflashlinkNdkVersion={version}",
         f"-PflashlinkJavaSrc={ANDROID / 'src/main/java'}",
         f"-PflashlinkFlutterArtifact={artifact}", f"-PflashlinkFlutterRepo={repo}"], host)
    apk = host / "app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk"
    with zipfile.ZipFile(apk) as package:
        required = {"libflutter.so", "libapp.so", "libflashlink_client.so", "libmoonlight-core.so"}
        missing = [name for name in required if f"lib/arm64-v8a/{name}" not in package.namelist()]
        if missing:
            raise ValueError("Incomplete APK. Missing native libraries: " + ", ".join(missing))
        check_flutter_payload(compiler, aar, package)
    destination = output / "flashlink-arm64-debug.apk"
    copy_apk(apk, destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare-only", action="store_true",
                        help="Apply and verify integration against the pinned host; skip compilation.")
    parser.add_argument("--upstream", default=ORIGIN,
                        help="Clone source (may be a local Moonlight git mirror containing tag v12.1).")
    parser.add_argument("--output", type=Path,
                        help="Use a separate build directory and leave the APK in install/ unchanged.")
    args = parser.parse_args()
    output = (args.output or ROOT / "build/android").resolve()
    host = prepare(output, args.upstream)
    if args.prepare_only:
        print(f"Patched Moonlight {PIN} in {host}")
    else:
        apk = build(output, host)
        if args.output is None:
            copy_apk(apk, ROOT / "install/FlashLink-Android-arm64.apk")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
