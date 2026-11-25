#!/usr/bin/env python3
"""Run FlashLink development checks or collect a local field report (Python 3.8+)."""
import argparse
from collections import Counter, deque
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "apps/flashlink"
EVENTS = {"STREAM_STARTED", "STREAM_INTERRUPTED", "RETRY_ATTEMPT", "STREAM_RESUMED",
          "RETRY_EXHAUSTED", "MANUAL_EXIT"}
VIDEO = re.compile(r"VIDEO width=(\d+) height=(\d+) window_ms=(\d+) received=(\d+) "
                   r"rendered=(\d+) lost=(\d+) decode_ms_total=(\d+)")
LOGCAT = re.compile(r"^\s*(\d+\.\d+)\s+\d+\s+\d+\s+I\s+FlashLinkSession\s*:\s*(.*)$")
OBSERVATIONS = {
    "discovery": "安卓能在同一局域网发现已启动的 Windows 主机吗？",
    "first_pair": "两端核对确认码并在 Windows 允许后，安卓能进入桌面吗？",
    "trusted_launch": "退出后再点同一设备，能直接连接吗？",
    "interaction": "实际操作时，翻页、右键、滚动、拖动、键盘都生效吗？",
    "viewport": "双指放大后能移动到四角、点中可见按钮，并用「复位视图」恢复整幅桌面吗？",
    "audio": "从首句语音开始，声音连续、左右声道正确，且与画面同步吗？",
    "audio_route": "电脑和安卓原本都有音量时，在串流中依次选「仅手机或平板播放」「仅电脑播放」「两端同时播放」，实际出声的设备是否对应？",
    "audio_disconnect_sync": "选择跟随媒体音量，调低平板媒体音量后主动断开，Windows 音量百分比和静音状态是否对应？",
    "audio_disconnect_mute": "选择「保持电脑静音」并持续断网，经过重连期限后 Windows 是否静音且音量刻度保留？",
    "privacy_setup": "首次运行或升级后，Windows 主机能否离线准备防窥功能，并保持实体屏显示桌面直到你点「开启防窥模式」？",
    "privacy": "开启隐私后，每块物理屏都看不到桌面，而平板仍能正常翻页吗？",
    "reconnect": "短时断开 Wi-Fi 再恢复，能自动回到同一会话吗？",
    "desktop_after_restore": "持续断网直到超时，串流是否结束且实体屏恢复当前桌面？",
}
HOST_MARKERS = {
    "connect_events": "CLIENT CONNECTED",
    "disconnect_events": "CLIENT DISCONNECTED",
    "opus_initializations": "Opus initialized:",
}
ISSUES = {
    "encoder": r"encod|nvenc|amf|quicksync|h264|hevc",
    "audio": r"audio|opus|wasapi|sound",
    "display": r"display|monitor|topology",
    "capture": r"captur|dxgi|duplication",
    "connection": r"connect|socket|network|rtsp|port ",
    "identity": r"certificat|pairing|tls",
    "input": r"mouse|keyboard|input",
}

def execute(args, cwd=ROOT, timeout=30):
    return subprocess.run([str(arg) for arg in args], cwd=cwd, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, encoding="utf-8", errors="replace", timeout=timeout)


def source_version():
    if not shutil.which("git"):
        return None
    head = execute(["git", "rev-parse", "HEAD"])
    if head.returncode or not re.fullmatch(r"[0-9a-f]{40,64}", head.stdout.strip()):
        return None
    status = execute(["git", "status", "--porcelain"])
    return {"commit": head.stdout.strip(), "modified": bool(status.stdout) if not status.returncode else None}


def binary_identity():
    result = {}
    for name in ("FlashLink-Windows-x64.exe", "FlashLink-Windows-x64.zip", "FlashLink-Android-arm64.apk"):
        path = ROOT / "install" / name
        if path.is_file():
            digest = hashlib.sha256()
            with path.open("rb") as file:
                for chunk in iter(lambda: file.read(1024 * 1024), b""):
                    digest.update(chunk)
            result[name] = digest.hexdigest()
    return result


def jdk_tool(name):
    java_home = os.environ.get("JAVA_HOME")
    if java_home:
        binary = Path(java_home) / "bin" / (name + ".exe" if os.name == "nt" else name)
        if binary.is_file():
            return str(binary)
    return shutil.which(name)


def development():
    checks = {}
    cargo = shutil.which("cargo")
    flutter = os.environ.get("FLUTTER_BIN") or shutil.which("flutter")
    def check(name, args, cwd=ROOT):
        if not args[0]:
            checks[name] = {"status": "skipped", "reason": "tool_unavailable"}
            return
        print("·", name, flush=True)
        started = time.monotonic()
        try:
            run = execute(args, cwd, timeout=600)
        except (OSError, subprocess.TimeoutExpired) as exc:
            checks[name] = {"status": "failed", "reason": type(exc).__name__}
            return
        entry = {"status": "passed" if run.returncode == 0 else "failed",
                 "exit_code": run.returncode, "seconds": round(time.monotonic() - started, 2)}
        if run.returncode:
            print(run.stdout[-5000:], file=sys.stderr)
            if name == "rust_tests":
                entry["failed_tests"] = re.findall(r"^test ([\w:]+) \.\.\. FAILED$", run.stdout, re.M)
            entry["compiler_codes"] = sorted(set(re.findall(r"\berror\[(E\d{4})\]", run.stdout)))
            if "Operation not permitted" in run.stdout or "Permission denied" in run.stdout:
                entry["permission_error"] = True
        if name == "rust_tests":
            entry["test_counts"] = [int(v) for v in re.findall(r"test result: ok\. (\d+) passed", run.stdout)]
        if name == "flutter_tests":
            counts = re.findall(r"\b\d+:\d+ \+(\d+)(?: ~(\d+))?(?: -(\d+))?:", run.stdout)
            if counts:
                passed, skipped, failed = counts[-1]
                entry.update(passed=int(passed), skipped=int(skipped or 0), failed=int(failed or 0))
        checks[name] = entry

    check("rust_tests", [cargo, "test", "--locked", "--workspace"])
    check("client_ffi_build", [cargo, "build", "--locked", "-p", "flashlink-client"])
    check("flutter_analyze", [flutter, "analyze"], APP)
    libraries = {"Windows": "flashlink_client.dll", "Darwin": "libflashlink_client.dylib",
                 "Linux": "libflashlink_client.so"}
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    native = (ROOT / target / "debug" / libraries.get(platform.system(), "")).resolve()
    flutter_test = [flutter, "test", "--reporter", "expanded"]
    if checks["client_ffi_build"]["status"] == "passed":
        flutter_test.append("--dart-define=FLASHLINK_TEST_CLIENT_LIBRARY=" + str(native))
    check("flutter_tests", flutter_test, APP)
    android = ROOT / "apps/android/src"
    main_java = android / "main/java/com/limelight"
    test_java = android / "test/java/com/limelight"
    with tempfile.TemporaryDirectory() as java_classes:
        for check_name, name in (("android_recovery", "FlashLinkReconnect"),
                                 ("android_touch", "FlashLinkTouchGesture"),
                                 ("android_quality", "FlashLinkQuality")):
            compile_check = check_name + "_compile"
            check(compile_check, [jdk_tool("javac"), "-sourcepath", android / "main/java", "-d", java_classes,
                  main_java / (name + ".java"), test_java / (name + "Test.java")])
            if checks[compile_check]["status"] == "passed":
                check(check_name + "_policy", [jdk_tool("java"), "-ea", "-cp", java_classes,
                      "com.limelight." + name + "Test"])
    return checks


def host_sample(path):
    # Read only a bounded log tail. Keep fixed markers, never export lines or device identities.
    try:
        if path is None or not path.is_file():
            return {"status": "unavailable", "reason": "host_log_missing"}
        with path.open("rb") as file:
            size = file.seek(0, os.SEEK_END)
            modified = os.fstat(file.fileno()).st_mtime
            file.seek(max(0, size - 2 * 1024 * 1024))
            lines = file.read().decode("utf-8", "replace").splitlines()
    except (FileNotFoundError, IsADirectoryError):
        return {"status": "unavailable", "reason": "host_log_missing"}
    except OSError as exc:
        return {"status": "unavailable", "reason": type(exc).__name__}
    counts = {key: sum(marker in line for line in lines) for key, marker in HOST_MARKERS.items()}
    modes = sorted(set(re.findall(r"Opus initialized: (\d{1,3} kHz, [1-8] channels?)", "\n".join(lines))))
    issues = Counter()
    for line in lines:
        if re.search(r"\b(?:Warning|Error|Fatal):", line):
            category = next((key for key, pattern in ISSUES.items() if re.search(pattern, line, re.I)), "other")
            issues[category] += 1
    return {"status": "collected", "log_tail_truncated": size > 2 * 1024 * 1024,
            "last_write_utc": datetime.fromtimestamp(modified, timezone.utc).isoformat(),
            "counts": counts, "opus_modes": modes, "issue_counts_including_startup_probes": issues}


def android_samples(lines):
    events = []
    totals = {}
    recent = deque(maxlen=240)
    start = None
    for line in lines:
        match = LOGCAT.fullmatch(line)
        if not match:
            continue
        stamp = float(match[1])
        item = match[2].strip()
        if item in EVENTS:
            if item == "STREAM_STARTED":
                events, totals, start = [], {}, stamp
                recent.clear()
            if start is None:
                start = stamp
            events.append({"after_s": round(stamp - start, 2), "event": item})
            continue
        sample = VIDEO.fullmatch(item)
        if not sample:
            continue
        width, height, interval, received, rendered, lost, decode = map(int, sample.groups())
        if not (width and height and interval):
            continue
        if start is None:
            start = stamp
        group = totals.setdefault(f"{width}x{height}", Counter())
        group.update(windows=1, window_ms=interval, received=received, rendered=rendered,
                     lost=lost, decode_ms_total=decode)
        recent.append({"after_s": round(stamp - start, 2), "size": f"{width}x{height}",
                       "window_ms": interval, "received": received, "surface_frames": rendered,
                       "lost": lost, "decode_ms_total": decode})
    for group in totals.values():
        group["mean_received_fps"] = round(1000 * group["received"] / group["window_ms"], 2)
        group["mean_surface_fps"] = round(1000 * group["rendered"] / group["window_ms"], 2)
    return {"status": "collected" if events or totals else "unavailable",
            "observed_from_utc": datetime.fromtimestamp(start, timezone.utc).isoformat() if start else None,
            "events": events, "video": totals, "recent_video_windows": list(recent)}


def android_report(adb):
    def adb_run(*args):
        return execute([adb, *args], timeout=30)
    devices = adb_run("get-state")
    if devices.returncode or devices.stdout.strip() != "device":
        return {"status": "unavailable", "reason": "select_one_authorized_device_with_ANDROID_SERIAL"}
    version = adb_run("shell", "dumpsys", "package", "dev.flashlink.demo.debug")
    installed = re.search(r"\bversionCode=(\d+)", version.stdout)
    if not installed:
        return {"status": "unavailable", "reason": "flashlink_demo_missing"}
    uid = re.search(r"\b(?:userId|appId)=(\d+)", version.stdout)
    switches = []
    if uid and "--uid" in adb_run("logcat", "--help").stdout:
        current_user = adb_run("shell", "am", "get-current-user").stdout.strip()
        if current_user.isdecimal():
            switches = ["--uid=" + str(int(current_user) * 100000 + int(uid[1]) % 100000)]
    log = adb_run("logcat", "-d", "-v", "epoch", *switches, "-s", "FlashLinkSession:I")
    if log.returncode:
        return {"status": "unavailable", "reason": "logcat_failed"}
    result = android_samples(log.stdout.splitlines())
    result["android_build_code"] = int(installed[1])
    paths = adb_run("shell", "pm", "path", "dev.flashlink.demo.debug")
    if paths.returncode == 0:
        for line in paths.stdout.splitlines():
            # Android's shell reconstructs its arguments as a command. Accept only
            # an APK path under /data/app and exclude traversal or shell syntax.
            match = re.fullmatch(r"package:(/data/app/(?:[A-Za-z0-9_~=+.-]+/)+base\.apk)", line.strip())
            if not match or any(part in (".", "..") for part in match[1].split("/")):
                continue
            checksum = adb_run("shell", "toybox", "sha256sum", match[1])
            digest = re.fullmatch(r"([0-9a-fA-F]{64})\s+" + re.escape(match[1]), checksum.stdout.strip())
            if checksum.returncode == 0 and digest:
                result["installed_apk_sha256"] = digest[1].lower()
            break
    result["emulator"] = adb_run("shell", "getprop", "ro.kernel.qemu").stdout.strip() == "1" or (
        adb_run("shell", "getprop", "ro.boot.qemu").stdout.strip() == "1")
    if result["status"] == "unavailable":
        result["reason"] = "no_flashlink_stream_events_in_log_buffer"
    return result


def privacy_events(log):
    if not log:
        return []
    path = log.with_name("privacy-events.log")
    if not path.is_file():
        return []
    with path.open("rb") as file:
        size = file.seek(0, os.SEEK_END)
        file.seek(max(0, size - 16384))
        lines = file.read().decode("ascii", errors="replace").splitlines()[-256:]
    events = []
    for line in lines:
        match = re.fullmatch(r"(\d{1,12})\s+(private|lost|resumed|locking|locked|restored|restore_pending|display_reapplied|guard_(?:pipe_closed|display_changed|display_timeout|display_without_lease|desktop_unavailable|engine_exited|lease_expired|user_end|owner_unavailable|management_unavailable))", line)
        if match:
            events.append({"unix_seconds": int(match[1]), "event": match[2]})
    return events


def privacy_display_log(log):
    if not log:
        return []
    path = log.with_name("privacy-display.log")
    if not path.is_file():
        return []
    with path.open("rb") as file:
        size = file.seek(0, os.SEEK_END)
        file.seek(max(0, size - 32768))
        lines = file.read().decode("ascii", errors="replace").splitlines()[-256:]
    pattern = re.compile(r"^\d{1,12}\s+phase=[a-z_]+(?:\s+[a-z_]+=[-0-9a-zA-Z_,/:]+)+$")
    return [line for line in lines if pattern.fullmatch(line)]


def field(args):
    path = args.host_log
    if path is None and os.name == "nt" and os.environ.get("LOCALAPPDATA"):
        path = Path(os.environ["LOCALAPPDATA"]) / "FlashLink/DemoHost/flashlink-sunshine.log"
    host = host_sample(path)
    previous_path = path.with_name("flashlink-sunshine-previous.log") if path and path.name == "flashlink-sunshine.log" else None
    previous = host_sample(previous_path)
    try:
        privacy = privacy_events(path)
    except OSError:
        privacy = []
    try:
        display_log = privacy_display_log(path)
    except OSError:
        display_log = []
    android = {"status": "not_requested"}
    if args.android:
        adb = os.environ.get("ADB") or shutil.which("adb")
        if adb:
            try:
                android = android_report(adb)
            except (OSError, subprocess.TimeoutExpired) as exc:
                android = {"status": "unavailable", "reason": type(exc).__name__}
        else:
            android = {"status": "unavailable", "reason": "adb_missing"}
    seen = dict.fromkeys(OBSERVATIONS, "not_tested")
    if args.observe:
        print("请按实际观察输入 y 通过、n 失败，回车跳过。")
        for key, prompt in OBSERVATIONS.items():
            while True:
                try:
                    value = input(prompt + " [y/n/回车] ").strip().lower()
                except EOFError:
                    value = ""
                if value in ("y", "n", ""):
                    seen[key] = {"y": "passed", "n": "failed", "": "not_tested"}[value]
                    break
    host["platform"] = args.host_platform or ("windows" if os.name == "nt" and args.host_log is None else "unknown")
    return {"host": host, "previous_stream_log": previous, "privacy_events": privacy, "privacy_display_log": display_log, "android": android, "observations": seen}


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--mode", choices=("dev", "field"), default="field" if os.name == "nt" else "dev")
    cli.add_argument("--host-log", type=Path, help="Sunshine log from a test run (field mode)")
    cli.add_argument("--host-platform", choices=("windows", "linux-test"), help="origin of an imported host log")
    cli.add_argument("--android", action="store_true", help="read FlashLink metrics from one USB/ADB device")
    cli.add_argument("--observe", action="store_true", help="record optional physical-device observations")
    cli.add_argument("--output", type=Path, default=ROOT / "testdata")
    args = cli.parse_args()
    if args.mode == "dev" and (args.host_log or args.host_platform or args.android or args.observe):
        cli.error("host logs, --android and --observe require --mode field")
    now = datetime.now(timezone.utc)
    report = {"schema_version": 1, "mode": args.mode, "created_utc": now.isoformat(),
              "platform": platform.system(), "os_release": platform.release(), "arch": platform.machine(),
              "source": source_version()}
    if args.mode == "dev":
        results = development()
        report["checks"] = results
        states = {item["status"] for item in results.values()}
        code = 1 if "failed" in states else 2 if "skipped" in states else 0
    else:
        results = field(args)
        report.update(results)
        report["local_package_sha256"] = binary_identity()
        seen = set(results["observations"].values())
        collected = any(results[k]["status"] == "collected" for k in ("host", "previous_stream_log", "android"))
        incomplete = ((args.android and results["android"]["status"] != "collected") or
                      (args.host_log is not None and results["host"]["status"] != "collected"))
        code = 1 if "failed" in seen else 2 if incomplete or (not collected and "passed" not in seen) else 0
    args.output.mkdir(parents=True, exist_ok=True)
    prefix = f"{args.mode}-{now:%Y%m%dT%H%M%SZ}-"
    with tempfile.NamedTemporaryFile(mode="w", dir=args.output, prefix=prefix, suffix=".tmp",
                                     encoding="utf-8", delete=False) as file:
        json.dump(report, file, indent=2, ensure_ascii=False)
        file.write("\n")
    destination = Path(file.name).with_suffix(".json")
    os.replace(file.name, destination)
    print(destination)
    return code


if __name__ == "__main__":
    sys.exit(main())
