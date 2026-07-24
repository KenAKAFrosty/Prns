#!/usr/bin/env python3
import re
import subprocess
import sys

ALLOWED_HOME_USERS = {"op", "operator", "prns", "user"}
SELF_PATH = "validation/hygiene/no-personal-paths.py"
UNIX_HOME = re.compile(rb"/(?:home|Users)/([A-Za-z0-9_.-]+)")
WINDOWS_HOME = re.compile(rb"(?i)[a-z]:[\\/]+users[\\/]+([A-Za-z0-9_.-]+)")
KNOWN_PERSONAL = re.compile(rb"(?i)_____")


def tree_blobs(rev: str):
    listing = subprocess.run(
        ["git", "ls-tree", "-r", "-z", rev],
        capture_output=True,
        check=True,
    ).stdout
    for entry in listing.split(b"\0"):
        if not entry:
            continue
        meta, path = entry.split(b"\t", 1)
        _mode, object_type, sha = meta.split()
        if object_type == b"blob" and path.decode() != SELF_PATH:
            yield sha.decode(), path.decode()


def violations_in(content: bytes):
    for pattern in (UNIX_HOME, WINDOWS_HOME):
        for match in pattern.finditer(content):
            user = match.group(1).decode().lower()
            if user not in ALLOWED_HOME_USERS:
                yield match.group(0).decode()
    for match in KNOWN_PERSONAL.finditer(content):
        yield match.group(0).decode(errors="replace")


def scan(rev: str) -> int:
    found = 0
    catter = subprocess.Popen(
        ["git", "cat-file", "--batch"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    for sha, path in tree_blobs(rev):
        catter.stdin.write((sha + "\n").encode())
        catter.stdin.flush()
        header = catter.stdout.readline()
        size = int(header.split()[2])
        content = catter.stdout.read(size)
        catter.stdout.read(1)
        for token in sorted(set(violations_in(content))):
            print(f"[personal-path] {rev}:{path}: {token}")
            found += 1
    catter.stdin.close()
    catter.wait()
    return found


def main() -> None:
    revs = sys.argv[1:] or ["HEAD"]
    total = sum(scan(rev) for rev in revs)
    if total:
        raise SystemExit(
            f"personal-path gate: {total} personal path token(s) in tracked content"
        )
    print("PERSONAL_PATH_GATE_OK")


if __name__ == "__main__":
    main()
