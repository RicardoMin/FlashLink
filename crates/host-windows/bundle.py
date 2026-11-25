"""Package FlashLink.exe with the verified, offline Sunshine runtime."""
import argparse
import hashlib
import io
import json
import pathlib
import shutil
import subprocess
import struct
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSETS = pathlib.Path(__file__).resolve().parent / "assets"
DIGEST = "233008e46f4c0e501a586cbfd6c4fd4a4c0d414a0b5fc7f13c070eb92ec3824b"
VDD_DIGESTS = {
    "MttVDD.inf": "550d211fe481e74dfe3f9d724ed78be48b3a9113405965d683d9373e8d672f5d",
    "MttVDD.dll": "c9ca837f57a98fbd43bc416a7f535a95843626e7759eaf85cf0cd7ce334dbb05",
    "mttvdd.cat": "08a0093fc9b2e32b287a6f8a77ca4de0a31830d29fc33d2b13a918dc859468f6",
    "vdd_settings.xml": "edb2501d6d5da17f66d15d4b97a6f4a3f0d8963165ac4a6a6259d95118288020",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=pathlib.Path, required=True)
    parser.add_argument("--sunshine-zip", type=pathlib.Path, required=True)
    parser.add_argument("--sunshine-license", type=pathlib.Path, required=True)
    parser.add_argument("--target", default="x86_64-pc-windows-msvc")
    parser.add_argument("--rust-sysroot", type=pathlib.Path)
    parser.add_argument("--llvm-mingw", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path,
                        help="Write a separate EXE/ZIP pair here. By default update dist/ and install/.")
    args = parser.parse_args()
    update_install = args.output is None
    if update_install:
        args.output = ROOT / "dist" / "FlashLink-Windows-x64.zip"
    if args.output.suffix.lower() != ".zip":
        parser.error("--output must name the portable .zip archive")
    host = args.exe.read_bytes()
    for name, digest in VDD_DIGESTS.items():
        component = (ASSETS / "mttvdd" / name).read_bytes()
        if hashlib.sha256(component).hexdigest() != digest:
            parser.error(f"Virtual display component {name} differs from release 25.7.23")
        if component not in host:
            parser.error(f"FlashLink.exe is missing the bundled virtual display component {name}")
    original = args.sunshine_zip.read_bytes()
    if hashlib.sha256(original).hexdigest() != DIGEST:
        parser.error("Sunshine ZIP digest differs from the pinned upstream release")
    sysroot = args.rust_sysroot or pathlib.Path(subprocess.check_output(
        ["rustc", "--print", "sysroot"], encoding="utf-8").strip())
    rust = sysroot/"share"/"doc"/"rust"
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", args.target],
        cwd=ROOT, encoding="utf-8"))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.output.parent) as temp:
        archive = pathlib.Path(temp)/args.output.name
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, strict_timestamps=False) as out:
            out.writestr("FlashLink.exe", host)
            out.write(ROOT/"LICENSE", "LICENSE")
            out.write(ASSETS/"NOTICE.txt", "NOTICE.txt")
            out.writestr("使用说明.txt", (ASSETS/"使用说明.txt").read_text(encoding="utf-8").replace("\n", "\r\n").encode("utf-8-sig"))
            out.write(ASSETS/"mttvdd/LICENSE", "licenses/MttVDD-LICENSE")
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
        single = args.output.with_suffix(".exe")
        application = pathlib.Path(temp)/single.name
        with zipfile.ZipFile(archive) as out:
            bad_entry = out.testzip()
            if bad_entry:
                raise OSError(f"CRC verification failed for {bad_entry}")
            sections = ["FlashLink 的使用说明和开源许可证", out.read("使用说明.txt").decode("utf-8-sig")]
            for entry in out.namelist():
                if entry in {"NOTICE.txt","LICENSE"} or entry.startswith("licenses/"):
                    sections.extend([entry, out.read(entry).decode("utf-8", errors="replace")])
            notices = "\n\n".join(sections).encode("utf-8-sig")
        if len(notices) > 16 * 1024**2:
            raise ValueError("Bundled notices exceed the host's 16 MiB limit")
        footer = struct.pack("<QQ16s", len(original), len(notices), b"FLASHLINK-PACK-1")
        with application.open("wb") as f:
            f.write(host)
            f.write(original)
            f.write(notices)
            f.write(footer)
        if update_install:
            install = ROOT / "install"
            install.mkdir(exist_ok=True)
            # Stage both verified packages on the destination filesystem before replacing either.
            with tempfile.TemporaryDirectory(dir=install) as staging:
                for package in (application, archive):
                    shutil.copyfile(package, pathlib.Path(staging) / package.name)
                for package in (application, archive):
                    (pathlib.Path(staging) / package.name).replace(install / package.name)
            print(f"Test downloads: {install / single.name}, {install / archive.name}")
        application.replace(single)
        print(single, single.stat().st_size, hashlib.sha256(single.read_bytes()).hexdigest())
        archive.replace(args.output)
    print(args.output, args.output.stat().st_size, hashlib.sha256(args.output.read_bytes()).hexdigest())


if __name__ == "__main__":
    main()
