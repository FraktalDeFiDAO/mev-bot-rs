#!/usr/bin/env python3
"""Import an official HashDB pruned tar using one SHA256-verified part buffer.

No joined multi-terabyte archive is created. The live DB is promoted only after
the complete archive has been verified and extracted. Interrupted imports replay
the same immutable snapshot into the staging directory; no partial DB is served.
"""
import fcntl
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import time

BASE = "https://snapshot.arbitrum.foundation/"
ROOT = Path("/data/arbitrum")
STAGE = ROOT / ".snapshot-import"
CACHE = ROOT / "snapshot-cache"
OLD = Path("/data/arbitrum-old/snapshot-cache")
PLAN = ROOT / "snapshot-import-plan.json"
DONE = ROOT / "snapshot-import-complete.json"
STATUS = ROOT / "snapshot-import-status.json"
CHUNK = 8 * 1024 * 1024


def save(path, value):
    tmp = path.with_name(f".{path.name}.{os.getpid()}.new")
    with tmp.open("w") as output:
        json.dump(value, output, indent=2)
        output.flush()
        os.fsync(output.fileno())
    tmp.replace(path)


def progress(phase, **fields):
    record = {"phase": phase, "utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), **fields}
    save(STATUS, record)
    print(json.dumps(record), flush=True)


def fetch(url):
    # The official CDN rejects Python's default urllib user agent on EC2.
    return subprocess.check_output(["curl", "--http1.1", "--fail", "--silent", "--show-error",
                                    "--max-time", "60", "--retry", "3", url], text=True)


def download_ranges(url, destination, expected):
    """Bounded HTTP/1.1 ranges avoid CDN long-stream resets and preserve retries.

    Each request writes at most 1 GiB. Up to four independent ranges download
    concurrently, then append in order. Only the final whole-part SHA256 permits
    extraction; a successful HTTP request alone never establishes integrity.
    """
    with destination.with_suffix(destination.suffix + ".download-lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        _download_ranges_locked(url, destination, expected)


def _download_ranges_locked(url, destination, expected):
    step = 2**30
    start = destination.stat().st_size if destination.exists() else 0

    def chunk(offset):
        end = min(offset + step, expected) - 1
        temporary = CACHE / f".{destination.name}.range-{offset}"
        headers = Path(str(temporary) + ".headers")
        count = end - offset + 1
        for attempt in range(8):
            result = subprocess.run(["curl", "--http1.1", "--fail", "--silent", "--show-error",
                                     "--connect-timeout", "30", "--max-time", "300", "--max-filesize", str(count),
                                     "--range", f"{offset}-{end}", "--dump-header", str(headers),
                                     "--output", str(temporary), url])
            received_range = f"content-range: bytes {offset}-{end}/{expected}"
            if result.returncode == 0 and temporary.stat().st_size == count and received_range in headers.read_text().lower():
                headers.unlink()
                return temporary
            time.sleep(min(2**attempt, 30))
        raise RuntimeError(f"Range download failed at offset {offset}; existing part retained")

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        while start < expected:
            offsets = list(range(start, min(start + 4 * step, expected), step))
            futures = [pool.submit(chunk, offset) for offset in offsets]
            for future in futures:
                temporary = future.result()
                with destination.open("ab") as output, temporary.open("rb") as source:
                    shutil.copyfileobj(source, output, CHUNK)
                    output.flush()
                    os.fsync(output.fileno())
                start = destination.stat().st_size
                temporary.unlink()
            progress("downloading", part=destination.name, bytes=start, expected_bytes=expected, cache=str(destination))


def snapshot_plan():
    if PLAN.exists():
        plan = json.loads(PLAN.read_text())
    else:
        url = os.environ.get("ARBITRUM_SNAPSHOT_URL", "")
        if not url:
            url = BASE + fetch(BASE + "arb1/latest-pruned.txt").strip()
        if not re.fullmatch(re.escape(BASE) + r"arb1/[A-Za-z0-9_-]+/", url):
            raise RuntimeError("Untrusted or invalid immutable snapshot URL")
        metadata = json.loads(fetch(url + "metadata.json"))
        manifest = fetch(url + ".manifest.txt")
        manifest_hashes = {}
        for line in manifest.splitlines():
            sha, filename = line.split()
            manifest_hashes[PurePosixPath(filename).name] = sha
        for part in metadata["parts"]:
            if manifest_hashes.get(part["filename"]) != part["sha256"]:
                raise RuntimeError("Metadata and official SHA256 manifest disagree")
        plan = {"url": url, "metadata": metadata}
        save(PLAN, plan)
    metadata = plan["metadata"]
    if not re.fullmatch(re.escape(BASE) + r"arb1/[A-Za-z0-9_-]+/", plan["url"]):
        raise RuntimeError("Invalid saved snapshot URL")
    for key, value in {"chain_name": "arb1", "state_scheme": "hash", "snapshot_kind": "pruned", "compression": "none"}.items():
        if metadata.get(key) != value:
            raise RuntimeError(f"Unsupported snapshot metadata: {key}")
    parts = metadata["parts"]
    for number, part in enumerate(parts):
        if part["filename"] != f"pruned.tar.part{number:04d}" or not re.fullmatch(r"[0-9a-f]{64}", part["sha256"]):
            raise RuntimeError("Unexpected snapshot part or SHA256")
        if not isinstance(part["size_bytes"], int) or part["size_bytes"] <= 0:
            raise RuntimeError("Invalid part size")
    total = sum(p["size_bytes"] for p in parts)
    if not parts or total != metadata["size"]["total_bytes"]:
        raise RuntimeError("Snapshot byte counts disagree")
    # Database + the largest verified download buffer, with 15% reserve.
    if total + max(p["size_bytes"] for p in parts) + 4 * 2**30 > shutil.disk_usage(ROOT).total * .85:
        raise RuntimeError("Snapshot exceeds mounted volume capacity with 15% reserve")
    return plan


class VerifiedParts:
    def __init__(self, plan):
        self.plan = plan
        self.index = 0
        self.file = None
        self.path = None
        self.remaining = 0
        self.consumed = 0

    def open_next(self):
        if self.index == len(self.plan["metadata"]["parts"]):
            return False
        part = self.plan["metadata"]["parts"][self.index]
        name = part["filename"]
        # Preserve the already-downloaded first part on the retained old volume.
        candidate = OLD / name
        self.path = candidate if os.path.ismount(OLD.parent) and candidate.exists() else CACHE / name
        if self.path.is_symlink():
            raise RuntimeError("Refusing a symlink as a snapshot cache file")
        current = self.path.stat().st_size if self.path.exists() else 0
        if current > part["size_bytes"]:
            raise RuntimeError(f"Oversized cached part: {name}")
        if current < part["size_bytes"]:
            if shutil.disk_usage(self.path.parent).free < part["size_bytes"] - current + 64 * 2**30:
                raise RuntimeError("Insufficient free space for download with 64 GiB emergency reserve")
            progress("downloading", part=name, bytes=current, expected_bytes=part["size_bytes"], cache=str(self.path))
            download_ranges(self.plan["url"] + name, self.path, part["size_bytes"])
        if self.path.stat().st_size != part["size_bytes"]:
            raise RuntimeError(f"Incomplete download: {name}")
        progress("verifying-sha256", part=name, bytes=part["size_bytes"])
        digest = hashlib.sha256()
        with self.path.open("rb") as source:
            while block := source.read(CHUNK):
                digest.update(block)
        if digest.hexdigest() != part["sha256"]:
            # Retain evidence. Do not repeatedly redownload or extract bad data.
            raise RuntimeError(f"SHA256 mismatch: {name}; inspect/remove this cache file explicitly")
        progress("extracting-verified-part", part=name, sha256=part["sha256"], archive_bytes_consumed=self.consumed)
        self.file = self.path.open("rb")
        self.remaining = part["size_bytes"]
        self.index += 1
        return True

    def read(self, size):
        if size <= 0:
            return b""
        if self.file is None and not self.open_next():
            return b""
        data = self.file.read(min(size, self.remaining))
        if not data:
            raise RuntimeError("Verified part ended unexpectedly")
        self.remaining -= len(data)
        self.consumed += len(data)
        if self.remaining == 0:
            self.file.close()
            self.file = None
            if self.path.parent == CACHE:
                self.path.unlink()  # bounded to one part; never delete retained old data
        return data


def main():
    if not os.path.ismount(ROOT):
        raise RuntimeError("Persistent EBS filesystem must be mounted")
    if os.getuid() != 1000:
        raise RuntimeError("Importer must run as UID 1000, matching Nitro")
    os.umask(0o027)
    with (ROOT / ".snapshot-import.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if DONE.exists():
            if not (ROOT / "nitro/l2chaindata").is_dir():
                raise RuntimeError("Completion marker exists without database")
            progress("complete", **json.loads(DONE.read_text()))
            return
        plan = snapshot_plan()
        ready = ROOT / "snapshot-import-ready.json"
        if (ROOT / "nitro").exists():
            # Recover an interrupted final atomic promotion only with proof.
            if ready.exists() and not STAGE.exists():
                save(DONE, json.loads(ready.read_text()))
                progress("complete", **json.loads(DONE.read_text()))
                return
            raise RuntimeError("Existing live Nitro directory; refusing to overwrite it")
        CACHE.mkdir(mode=0o750, exist_ok=True)
        STAGE.mkdir(mode=0o750, exist_ok=True)
        reader = VerifiedParts(plan)
        extracted = 0
        try:
            with tarfile.open(fileobj=reader, mode="r|", bufsize=CHUNK) as archive:
                for member in archive:
                    relative = PurePosixPath(member.name)
                    if relative.is_absolute() or ".." in relative.parts:
                        raise RuntimeError("Unsafe archive path")
                    if relative.parts and relative.parts[0].lower() == "wasm":
                        continue  # same import-wasm=false behavior as official Nitro
                    target = STAGE.joinpath(*relative.parts)
                    if target.is_symlink() or STAGE.resolve() not in [target.resolve(), *target.resolve().parents]:
                        raise RuntimeError("Unsafe extraction target")
                    if member.isdir():
                        target.mkdir(mode=0o750, parents=True, exist_ok=True)
                    elif member.isfile() and not member.sparse:
                        extracted += member.size
                        if extracted > plan["metadata"]["size"]["total_bytes"]:
                            raise RuntimeError("Extracted file sizes exceed verified archive size")
                        if shutil.disk_usage(ROOT).free < member.size + 64 * 2**30:
                            raise RuntimeError("Extraction would breach 64 GiB emergency free-space reserve")
                        target.parent.mkdir(mode=0o750, parents=True, exist_ok=True)
                        with archive.extractfile(member) as source, target.open("wb") as output:
                            shutil.copyfileobj(source, output, CHUNK)
                        target.chmod(0o640)
                    else:
                        raise RuntimeError(f"Unsupported archive member type: {member.name}")
            # Tar end markers can precede padding. Verify/consume every part.
            while reader.read(CHUNK):
                pass
        finally:
            if reader.file:
                reader.file.close()
        if not (STAGE / "l2chaindata").is_dir():
            raise RuntimeError("Official archive did not contain l2chaindata")
        subprocess.run(["sync", "-f", str(ROOT)], check=True)
        complete = {"snapshot_url": plan["url"], "archive_bytes": reader.consumed,
                    "extracted_file_bytes": extracted, "state_scheme": "hash", "chain_id": 42161}
        save(ready, complete)
        STAGE.rename(ROOT / "nitro")
        save(DONE, complete)
        progress("complete", **complete)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        progress("failed", error=str(error))
        raise
