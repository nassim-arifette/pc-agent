"""Download and verify the published release, without redistributing the base model."""

import argparse
import hashlib
from pathlib import Path
import shutil
from urllib.request import urlopen
from urllib.error import HTTPError
import zipfile

RELEASE = "https://github.com/nassim-arifette/pc-agent/releases/download/nano4b-specialist-v0.1.0/"
ARCHIVE = "Llama-PC-Agent-Nano4B-Specialist-v0.1.0.zip"
SHA256 = "53b38092038c5d8d0984b4d5940ed035ba24a6acc7c71c20b24a0afdc9efc704"
ROOT = "Llama-PC-Agent-Nano4B-Specialist-v0.1.0"


def download(directory, supplied_archive=None):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / ARCHIVE
    if supplied_archive and Path(supplied_archive).resolve() != archive:
        shutil.copyfile(supplied_archive, archive)
    if supplied_archive and hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise ValueError("Supplied release SHA-256 mismatch; refusing download and extraction")
    if not archive.is_file() or hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        temporary = directory / (ARCHIVE + ".partial")
        try:
            with urlopen(RELEASE + ARCHIVE, timeout=60) as source, temporary.open("wb") as target:
                shutil.copyfileobj(source, target)
        except HTTPError as error:
            raise SystemExit("Release unavailable. For a private repo, download the ZIP in your authenticated GitHub browser and pass --archive PATH. No token is needed by the specialist.") from error
        if hashlib.sha256(temporary.read_bytes()).hexdigest() != SHA256:
            temporary.unlink()
            raise ValueError("Release SHA-256 mismatch; refusing extraction")
        temporary.replace(archive)
    with zipfile.ZipFile(archive) as bundle:
        total = 0
        for item in bundle.infolist():
            path = directory.joinpath(item.filename).resolve()
            if not path.is_relative_to(directory / ROOT) or (item.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError("Unsafe archive entry")
            total += item.file_size
            if total > 512 * 1024 * 1024:
                raise ValueError("Archive exceeds expected size")
        bundle.extractall(directory)
    root = directory / ROOT
    for line in (root / "SHA256SUMS.txt").read_text(encoding="utf-8").splitlines():
        digest, name = line.split(maxsplit=1)
        path = (root / name.lstrip("*")).resolve()
        if not path.is_relative_to(root) or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError("Extracted file SHA-256 mismatch")
    return root


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--archive", type=Path, help="Manually downloaded release ZIP (private repositories)")
    args = parser.parse_args()
    print(download(args.directory, args.archive))
