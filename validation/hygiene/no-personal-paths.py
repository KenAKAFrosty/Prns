#!/usr/bin/env python3
import re
import subprocess
import sys

ALLOWED_HOME_USERS = {"op", "operator", "prns", "user"}
UNIX_HOME = re.compile(rb"/(?:home|Users)/([A-Za-z0-9_.-]+)")
WINDOWS_HOME = re.compile(rb"(?i)[a-z]:[\\/]+users[\\/]+([A-Za-z0-9_.-]+)")
TOKENS_SPLIT_SO_SCRUBS_NEVER_MATCH_THIS_FILE = ((b"kc", b"tra"),)
KNOWN_PERSONAL = re.compile(
    b"(?i)" + b"|".join(b"".join(parts) for parts in TOKENS_SPLIT_SO_SCRUBS_NEVER_MATCH_THIS_FILE)
)


def reachable_blobs(revs: list[str]):
    listing = subprocess.run(
        ["git", "rev-list", "--objects", "--filter=object:type=blob", *revs],
        capture_output=True,
        check=True,
    ).stdout
    for entry in listing.splitlines():
        sha, separator, path = entry.partition(b" ")
        if not separator:
            continue
        yield sha.decode(), path.decode()


def violations_in(content: bytes):
    for pattern in (UNIX_HOME, WINDOWS_HOME):
        for match in pattern.finditer(content):
            user = match.group(1).decode().lower()
            if user not in ALLOWED_HOME_USERS:
                yield match.group(0).decode()
    for match in KNOWN_PERSONAL.finditer(content):
        yield match.group(0).decode(errors="replace")


def scan(revs: list[str]) -> int:
    found = 0
    source = revs[0] if len(revs) == 1 else f"{len(revs)} revisions"
    catter = subprocess.Popen(
        ["git", "cat-file", "--batch"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    for sha, path in reachable_blobs(revs):
        catter.stdin.write((sha + "\n").encode())
        catter.stdin.flush()
        header = catter.stdout.readline()
        size = int(header.split()[2])
        content = catter.stdout.read(size)
        catter.stdout.read(1)
        for token in sorted(set(violations_in(content))):
            print(f"[personal-path] {source}:{path}: {token}")
            found += 1
    catter.stdin.close()
    catter.wait()
    return found


def main() -> None:
    revs = sys.argv[1:] or ["HEAD"]
    total = scan(revs)
    if total:
        raise SystemExit(
            f"personal-path gate: {total} personal path token(s) in tracked content"
        )
    print("PERSONAL_PATH_GATE_OK")


if __name__ == "__main__":
    main()
