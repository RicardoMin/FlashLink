"""Run the shipping Windows install transaction with isolated stand-ins for the host process."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "crates/host-windows/assets/install-update.ps1"
HOST = r'''
using System;
using System.IO;
using System.Threading;
public static class Fixture {
    public static int Main(string[] args) {
        string dir = AppDomain.CurrentDomain.BaseDirectory;
        string boot = Environment.GetEnvironmentVariable("FLASHLINK_UPDATE_BOOT");
        File.AppendAllText(Path.Combine(dir, "observed.log"), "VERSION|" +
            (boot == null ? "started" : "trial") + "|" + String.Join("|", args) + Environment.NewLine);
        if (boot != null && File.Exists(Path.Combine(dir, "fail-start"))) return 7;
        if (boot != null) {
            using (EventWaitHandle ready = EventWaitHandle.OpenExisting(@"Local\FlashLinkUpdate-" + boot))
                ready.Set();
        }
        return 0;
    }
}
'''


@unittest.skipUnless(os.name == "nt", "Requires the Windows process and file replacement APIs")
class WindowsInstall(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="flashlink-update-qa-")
        cls.base = Path(cls.temp.name).resolve()
        windir = Path(os.environ["SystemRoot"])
        cls.powershell = windir / "System32/WindowsPowerShell/v1.0/powershell.exe"
        compiler = windir / "Microsoft.NET/Framework64/v4.0.30319/csc.exe"
        if not compiler.is_file():
            cls.temp.cleanup()
            raise unittest.SkipTest(".NET Framework compiler is required for the Windows host fixture")
        for version in ("old", "new"):
            source = cls.base / (version + ".cs")
            source.write_text(HOST.replace("VERSION", version), encoding="utf-8")
            subprocess.run([compiler, "/nologo", "/target:winexe", "/out:" + str(source.with_suffix(".exe")), source],
                           check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=self.base)
        self.addCleanup(directory.cleanup)
        self.sandbox = Path(directory.name)
        self.install = self.sandbox / "portable host"
        self.install.mkdir()
        self.exe = self.install / "FlashLink.exe"
        shutil.copyfile(self.base / "old.exe", self.exe)
        self.payload = self.sandbox / "download"
        self.archive = {"FlashLink.exe": (self.base / "new.exe").read_bytes(),
                        "Sunshine/sunshine.exe": b"new engine", "Sunshine/new.dll": b"new file"}

    def portable(self):
        for name, body in {"Sunshine/sunshine.exe": b"old engine", "Sunshine/old.dll": b"old file",
                           "Sunshine/config/private.conf": b"paired host", "Sunshine/extra.txt": b"user notes",
                           "unrelated.txt": b"user data"}.items():
            item = self.install / name
            item.parent.mkdir(parents=True, exist_ok=True)
            item.write_bytes(body)
        (self.install / "package-files.json").write_text(
            json.dumps(["FlashLink.exe", "Sunshine/sunshine.exe", "Sunshine/old.dll"]))
        with zipfile.ZipFile(self.payload, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, data in self.archive.items():
                archive.writestr(name, data)
            archive.writestr("package-files.json", json.dumps(list(self.archive)))

    def run_helper(self, *, single, reject_hash=False, recover=False):
        # Suppress only the modal notification in headless CI. All transaction and restart operations run.
        script = INSTALLER.read_text(encoding="utf-8-sig")
        dialog = "[void][Windows.Forms.MessageBox]::Show($detail, 'FlashLink 更新失败')"
        self.assertIn(dialog, script)
        script = script.replace(dialog, "[Console]::Error.WriteLine($detail)")
        encoded = base64.b64encode(script.encode("utf-16-le")).decode()
        data = self.payload.read_bytes() if self.payload.is_file() else b""
        env = {**os.environ, "LOCALAPPDATA": str(self.sandbox), "FLASHLINK_UPDATE_PARENT": "2147483647",
               "FLASHLINK_UPDATE_TARGET": str(self.exe), "FLASHLINK_UPDATE_FILE": str(self.payload),
               "FLASHLINK_UPDATE_HASH": "0" * 64 if reject_hash else hashlib.sha256(data).hexdigest(),
               "FLASHLINK_UPDATE_SIZE": str(len(data)), "FLASHLINK_UPDATE_SINGLE": str(int(single)),
               "FLASHLINK_UPDATE_RECOVER": str(int(recover)), "FLASHLINK_UPDATE_ARGS": '"with space" "two"'}
        env.pop("PSModulePath", None)
        completed = subprocess.run([self.powershell, "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden",
                                    "-EncodedCommand", encoded], cwd=self.sandbox, env=env, timeout=120,
                                   creationflags=subprocess.CREATE_NO_WINDOW, capture_output=True)
        self.assertEqual(completed.returncode, 0, completed.stderr.decode(errors="replace"))
        self.assertFalse(self.payload.exists())

    def observed(self, line):
        log = self.install / "observed.log"
        for _ in range(60):
            if log.exists() and line in log.read_text():
                return
            time.sleep(0.05)
        self.fail(f"Child did not report {line!r}. Observed: {log.read_text() if log.is_file() else ''}")

    def test_single_exe_restarts_and_acknowledges(self):
        shutil.copyfile(self.base / "new.exe", self.payload)
        self.run_helper(single=True)
        self.observed("new|trial|with space|two")
        self.assertEqual(self.exe.read_bytes(), (self.base / "new.exe").read_bytes())
        self.assertFalse((self.install / ".flashlink-update.pending").exists())

    def test_rejects_damaged_download(self):
        shutil.copyfile(self.base / "new.exe", self.payload)
        self.run_helper(single=True, reject_hash=True)
        self.observed("old|started|with space|two")
        self.assertEqual(self.exe.read_bytes(), (self.base / "old.exe").read_bytes())

    def test_portable_replaces_only_managed_files(self):
        self.portable()
        self.run_helper(single=False)
        self.observed("new|trial|with space|two")
        self.assertEqual((self.install / "Sunshine/sunshine.exe").read_bytes(), b"new engine")
        self.assertFalse((self.install / "Sunshine/old.dll").exists())
        self.assertTrue((self.install / "Sunshine/new.dll").exists())
        self.assertEqual((self.install / "Sunshine/config/private.conf").read_bytes(), b"paired host")
        self.assertEqual((self.install / "Sunshine/extra.txt").read_bytes(), b"user notes")
        self.assertEqual((self.install / "unrelated.txt").read_bytes(), b"user data")

    def test_failure_to_start_restores_original_portable_tree(self):
        self.portable()
        (self.install / "fail-start").touch()
        originals = {p: p.read_bytes() for p in self.install.rglob("*") if p.is_file()}
        self.run_helper(single=False)
        self.observed("new|trial|with space|two")
        self.observed("old|started|with space|two")
        for path, data in originals.items():
            self.assertEqual(path.read_bytes(), data, str(path))
        self.assertFalse((self.install / "Sunshine/new.dll").exists())
        self.assertFalse((self.install / ".flashlink-update.pending").exists())

    def test_write_failure_rolls_back_before_restart(self):
        self.portable()
        collision = self.install / "Sunshine/new.dll"
        collision.mkdir()
        (collision / "user.txt").write_text("keep me")
        self.run_helper(single=False)
        self.observed("old|started|with space|two")
        self.assertEqual((collision / "user.txt").read_text(), "keep me")
        self.assertEqual((self.install / "Sunshine/sunshine.exe").read_bytes(), b"old engine")
        self.assertEqual(self.exe.read_bytes(), (self.base / "old.exe").read_bytes())
        self.assertFalse((self.install / ".flashlink-update.pending").exists())

    def test_next_start_recovers_interrupted_transaction(self):
        work = self.install / (".flashlink-update-" + "f" * 32)
        backups = work / "old"
        backups.mkdir(parents=True)
        shutil.copyfile(self.exe, backups / "FlashLink.exe")
        shutil.copyfile(self.base / "new.exe", self.exe)
        added = self.install / "Sunshine/added.tmp"
        added.parent.mkdir()
        added.write_bytes(b"incomplete update")
        (work / "plan.json").write_text(json.dumps([{"name": "FlashLink.exe", "existed": True},
                                                    {"name": "Sunshine/added.tmp", "existed": False}]))
        marker = self.install / ".flashlink-update.pending"
        marker.write_text(work.name)
        self.run_helper(single=False, recover=True)
        self.observed("old|started|with space|two")
        self.assertEqual(self.exe.read_bytes(), (self.base / "old.exe").read_bytes())
        self.assertFalse(added.exists())
        self.assertFalse(marker.exists())
        self.assertFalse(work.exists())


if __name__ == "__main__":
    unittest.main()
