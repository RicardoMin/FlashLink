"""Package FlashLink.exe with the verified, offline Sunshine runtime."""
import argparse
import hashlib
import io
import json
import pathlib
import subprocess
import struct
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
HERE = pathlib.Path(__file__).resolve().parent
DIGEST = "233008e46f4c0e501a586cbfd6c4fd4a4c0d414a0b5fc7f13c070eb92ec3824b"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=pathlib.Path, required=True)
    parser.add_argument("--sunshine-zip", type=pathlib.Path, required=True)
    parser.add_argument("--sunshine-license", type=pathlib.Path, required=True)
    parser.add_argument("--target", default="x86_64-pc-windows-msvc")
    parser.add_argument("--rust-sysroot", type=pathlib.Path)
    parser.add_argument("--llvm-mingw", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path, default=ROOT/"dist"/"FlashLink-Windows-x64.zip")
    args = parser.parse_args()
    original = args.sunshine_zip.read_bytes()
    if hashlib.sha256(original).hexdigest() != DIGEST:
        parser.error("Sunshine ZIP digest differs from the pinned upstream release")
    sysroot = args.rust_sysroot or pathlib.Path(subprocess.check_output(
        ["rustc", "--print", "sysroot"], text=True).strip())
    rust = sysroot/"share"/"doc"/"rust"
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", args.target],
        cwd=ROOT, text=True))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.output.parent) as temp:
        archive = pathlib.Path(temp)/args.output.name
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, strict_timestamps=False) as out:
            out.write(args.exe, "FlashLink.exe")
            out.write(ROOT/"LICENSE", "LICENSE")
            out.write(HERE/"NOTICE.txt", "NOTICE.txt")
            out.writestr("使用说明.txt", (HERE/"使用说明.txt").read_text().replace("\n", "\r\n").encode("utf-8-sig"))
            out.write(args.sunshine_license, "licenses/Sunshine-LICENSE")
            with zipfile.ZipFile(io.BytesIO(original)) as engine:
                for item in engine.infolist():
                    if not item.is_dir():
                        out.writestr(item.filename, engine.read(item))
            for license_ in sorted((rust/"licenses").glob("*.txt")):
                out.write(license_, f"licenses/rust/{license_.name}")
            if (rust/"COPYRIGHT-library.html").exists():
                out.write(rust/"COPYRIGHT-library.html", "licenses/Rust-Std-COPYRIGHT-library.html")
            if args.llvm_mingw:
                out.write(args.llvm_mingw/"LICENSE.TXT", "licenses/LLVM-MinGW-LICENSE.TXT")
                for notice in sorted((args.llvm_mingw/"x86_64-w64-mingw32/share/mingw32").glob("COPYING*")):
                    out.write(notice, f"licenses/mingw32/{notice.name}")
            summaries = []
            for pkg in metadata["packages"]:
                if not pkg["source"]:
                    continue
                label = f"{pkg['name']}-{pkg['version']}"
                source = pathlib.Path(pkg["manifest_path"]).parent
                summaries.append(f"{label}  {pkg['license'] or ''}  {pkg['repository'] or ''}")
                for notice in sorted(source.iterdir()):
                    if notice.is_file() and notice.name.upper().startswith(("LICENSE","COPYING","NOTICE")):
                        out.write(notice, f"licenses/crates/{label}/{notice.name}")
            out.writestr("licenses/Rust-dependencies.txt", "\n".join(sorted(summaries)) + "\n")
        with zipfile.ZipFile(archive) as out, zipfile.ZipFile(io.BytesIO(original)) as engine:
            if out.testzip():
                raise OSError("An output entry failed CRC verification")
            assert out.read("FlashLink.exe") == args.exe.read_bytes()
            for item in engine.infolist():
                if not item.is_dir():
                    assert out.read(item.filename) == engine.read(item)
        single = args.output.with_suffix(".exe")
        application = pathlib.Path(temp)/single.name
        with zipfile.ZipFile(archive) as out:
            sections = ["FlashLink 的使用说明和开源许可证", out.read("使用说明.txt").decode("utf-8-sig")]
            for entry in out.namelist():
                if entry in {"NOTICE.txt","LICENSE"} or entry.startswith("licenses/"):
                    sections.extend([entry, out.read(entry).decode("utf-8", errors="replace")])
            notices = "\n\n".join(sections).encode("utf-8-sig")
        assert len(notices) < 16 * 1024**2
        footer = struct.pack("<QQ16s", len(original), len(notices), b"FLASHLINK-PACK-1")
        with application.open("wb") as f:
            f.write(args.exe.read_bytes())
            f.write(original)
            f.write(notices)
            f.write(footer)
        with application.open("rb") as f:
            f.seek(args.exe.stat().st_size)
            assert hashlib.sha256(f.read(len(original))).hexdigest() == DIGEST
            assert f.read(len(notices)) == notices and f.read() == footer
        application.replace(single)
        print(single, single.stat().st_size, hashlib.sha256(single.read_bytes()).hexdigest())
        archive.replace(args.output)
    print(args.output, args.output.stat().st_size, hashlib.sha256(args.output.read_bytes()).hexdigest())


if __name__ == "__main__":
    main()
