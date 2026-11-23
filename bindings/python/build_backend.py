"""Builds the fakes3-sdk wheel: the Python package plus one prebuilt fakes3 binary.

Each wheel carries the executable for one platform and is tagged ``py3-none-<platform>``, so pip picks
the one that matches. No third-party build tool is involved.

From CI, with a binary that is already built::

    python build_backend.py --binary fakes3 --platform manylinux_2_17_x86_64.musllinux_1_1_x86_64

It is also the PEP 517 backend of this directory, so ``pip install bindings/python`` works in a
checkout with a Rust toolchain: it runs ``cargo build --release`` and tags the wheel for this machine.
``FAKES3_WHEEL_BINARY`` and ``FAKES3_WHEEL_PLATFORM`` override the binary and the tag.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import os
import re
import subprocess
import sys
import sysconfig
import zipfile
from pathlib import Path
from typing import List, Optional, Tuple

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
DIST_NAME = "fakes3-sdk"
SUMMARY = "In-memory S3 server for tests: a prebuilt fakes3 binary, a Python API and pytest fixtures"


def version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'^\[package\][^\[]*?^version\s*=\s*"([^"]+)"', text, re.M | re.S)
    if not m:
        raise RuntimeError("no package version in Cargo.toml")
    return m.group(1)


def host_platform() -> str:
    return re.sub(r"[-.]", "_", sysconfig.get_platform())


def metadata(ver: str) -> str:
    readme = (HERE / "README.md").read_text(encoding="utf-8")
    headers = [
        "Metadata-Version: 2.4",
        f"Name: {DIST_NAME}",
        f"Version: {ver}",
        f"Summary: {SUMMARY}",
        "License-Expression: MIT",
        "License-File: LICENSE",
        "Requires-Python: >=3.9",
        "Keywords: s3,testing,pytest,mock,fake,boto3",
        "Project-URL: Homepage, https://github.com/AkaraChen/fakes3",
        "Project-URL: Source, https://github.com/AkaraChen/fakes3/tree/main/bindings/python",
        "Project-URL: Issues, https://github.com/AkaraChen/fakes3/issues",
        "Classifier: Development Status :: 4 - Beta",
        "Classifier: Framework :: Pytest",
        "Classifier: Intended Audience :: Developers",
        "Classifier: Programming Language :: Python :: 3",
        "Classifier: Programming Language :: Rust",
        "Classifier: Topic :: Software Development :: Testing",
        "Classifier: Typing :: Typed",
        "Description-Content-Type: text/markdown",
    ]
    return "\n".join(headers) + "\n\n" + readme


ENTRY_POINTS = """\
[console_scripts]
fakes3 = fakes3.__main__:main

[pytest11]
fakes3 = fakes3.pytest_plugin
"""


def _record_hash(data: bytes) -> str:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"sha256={digest}"


def build(binary: Path, platform: str, out_dir: Path) -> Path:
    ver = version()
    exe = "fakes3.exe" if platform.startswith("win") else "fakes3"
    dist_info = f"{DIST_NAME.replace('-', '_')}-{ver}.dist-info"
    wheel_name = f"{DIST_NAME.replace('-', '_')}-{ver}-py3-none-{platform}.whl"

    # (archive path, content, executable)
    files: List[Tuple[str, bytes, bool]] = []
    src = HERE / "src"
    for path in sorted((src / "fakes3").rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts and path.suffix in (".py", ".typed"):
            files.append((path.relative_to(src).as_posix(), path.read_bytes(), False))
    files.append((f"fakes3/bin/{exe}", binary.read_bytes(), True))
    files.append((f"{dist_info}/METADATA", metadata(ver).encode(), False))
    wheel_meta = f"Wheel-Version: 1.0\nGenerator: fakes3 build_backend\nRoot-Is-Purelib: false\n"
    wheel_meta += "".join(f"Tag: py3-none-{p}\n" for p in platform.split("."))
    files.append((f"{dist_info}/WHEEL", wheel_meta.encode(), False))
    files.append((f"{dist_info}/entry_points.txt", ENTRY_POINTS.encode(), False))
    files.append((f"{dist_info}/licenses/LICENSE", (ROOT / "LICENSE").read_bytes(), False))

    record = "".join(f"{name},{_record_hash(data)},{len(data)}\n" for name, data, _ in files)
    record += f"{dist_info}/RECORD,,\n"
    files.append((f"{dist_info}/RECORD", record.encode(), False))

    # Reproducible: fixed timestamps, fixed order.
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "315532800"))
    import time

    date_time = time.gmtime(max(epoch, 315532800))[:6]
    out_dir.mkdir(parents=True, exist_ok=True)
    target = out_dir / wheel_name
    with zipfile.ZipFile(target, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for name, data, executable in files:
            info = zipfile.ZipInfo(name, date_time=date_time)  # type: ignore[arg-type]
            info.external_attr = ((0o100755 if executable else 0o100644) & 0xFFFF) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(info, data)
    return target


def _cargo_binary() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")],
        check=True,
        stdout=sys.stderr,
    )
    exe = "fakes3.exe" if os.name == "nt" else "fakes3"
    return ROOT / "target" / "release" / exe


# -- PEP 517 ------------------------------------------------------------------------------------


def get_requires_for_build_wheel(config_settings: Optional[dict] = None) -> List[str]:
    return []


def build_wheel(
    wheel_directory: str,
    config_settings: Optional[dict] = None,
    metadata_directory: Optional[str] = None,
) -> str:
    settings = config_settings or {}
    binary = settings.get("binary") or os.environ.get("FAKES3_WHEEL_BINARY")
    platform = settings.get("platform") or os.environ.get("FAKES3_WHEEL_PLATFORM") or host_platform()
    path = Path(binary) if binary else _cargo_binary()
    return build(path, platform, Path(wheel_directory)).name


def main(argv: Optional[List[str]] = None) -> None:
    p = argparse.ArgumentParser(description="Build a fakes3-sdk wheel around a fakes3 binary.")
    p.add_argument("--binary", type=Path, required=True, help="the fakes3 executable to bundle")
    p.add_argument("--platform", default=host_platform(), help="wheel platform tag(s), dot-separated")
    p.add_argument("--out", type=Path, default=HERE / "dist", help="output directory")
    args = p.parse_args(argv)
    print(build(args.binary, args.platform, args.out))


if __name__ == "__main__":
    main()
