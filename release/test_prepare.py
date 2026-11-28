import base64
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import prepare


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "source"
        self.staging = self.root / "staging"
        self.source.mkdir()
        self.staging.mkdir()
        for files in prepare.NAMES.values():
            for filename in files.values():
                (self.source / filename).write_bytes(filename.encode())
        self.notes = self.root / "notes.txt"
        self.notes.write_text("版本更新\n第二行", encoding="utf-8-sig")
        for name in ("apk_identity", "windows_identity"):
            mock = patch.object(prepare, name)
            mock.start()
            self.addCleanup(mock.stop)
        self.versions = {platform: {"version": "0.4.0", "build": 320} for platform in prepare.NAMES}
        self.first = prepare.assemble("v0.4.0", self.versions, None,
                                      {"windows": self.notes, "android": self.notes}, self.source, self.staging)

    def test_android_only_preserves_windows(self):
        self.versions["android"] = {"version": "0.4.1", "build": 321}
        next_dir = self.root / "next"
        next_dir.mkdir()
        updated = prepare.assemble("v0.4.1", self.versions, self.first, {"android": self.notes, "windows": None},
                                   self.source, next_dir)
        self.assertEqual(updated["windows"], self.first["windows"])
        self.assertIn("/v0.4.1/", updated["android"]["assets"]["apk"]["url"])
        self.assertEqual([p.name for p in next_dir.iterdir()], ["FlashLink-Android-arm64.apk"])

    def test_build_and_tag_must_advance(self):
        notes = {"android": self.notes, "windows": None}
        with self.assertRaisesRegex(ValueError, "tag must be newer"):
            prepare.assemble("v0.4.0", self.versions, self.first, notes, self.source, self.staging)
        self.versions["android"]["version"] = "0.4.1"
        with self.assertRaisesRegex(ValueError, "build number must increase"):
            prepare.assemble("v0.4.1", self.versions, self.first, notes, self.source, self.staging)

    def test_cannot_relabel_an_unchanged_or_missing_platform(self):
        self.versions["windows"]["version"] = "0.4.1"
        with self.assertRaisesRegex(ValueError, "--windows-notes"):
            prepare.assemble("v0.4.1", self.versions, self.first, {"android": self.notes, "windows": None},
                             self.source, self.staging)
        with self.assertRaisesRegex(ValueError, "--windows-notes"):
            prepare.assemble("v0.4.1", self.versions, None, {"android": self.notes, "windows": None},
                             self.source, self.staging)

    def test_metadata_limits_and_tag_url(self):
        entry = self.first["android"]["assets"]["apk"]
        for key, bad in [("size", False), ("size", 512 * 1024**2 + 1), ("sha256", "0"),
                         ("url", entry["url"].replace("v0.4.0", "latest"))]:
            with self.subTest(key=key), patch.dict(entry, {key: bad}), self.assertRaises(ValueError):
                prepare.validate_payload(self.first)
        self.first["windows"]["notes"] = "帧" * 2048
        with self.assertRaisesRegex(ValueError, "release notes"):
            prepare.validate_payload(self.first)

    def test_real_signature_round_trip_and_tampering(self):
        private_key = self.root / "test.pem"
        public_key = self.root / "test.der"
        prepare.openssl("genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:3072", "-out", private_key)
        prepare.openssl("pkey", "-in", private_key, "-pubout", "-outform", "DER", "-out", public_key)
        data = prepare.sign(self.first, private_key, public_key)
        self.assertEqual(prepare.verify(data, public_key), self.first)
        envelope = json.loads(data)
        envelope["payload"] = base64.b64encode(base64.b64decode(envelope["payload"]) + b" ").decode()
        with self.assertRaises(subprocess.CalledProcessError):
            prepare.verify(json.dumps(envelope).encode(), public_key)


if __name__ == "__main__":
    unittest.main()
