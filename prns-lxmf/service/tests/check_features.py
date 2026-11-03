#!/usr/bin/env python3
"""Build isolated consumers and inspect their selected dependency closure."""
import json
from pathlib import Path
import subprocess

manifest = Path(__file__).resolve().parent / "fixtures/consumer/Cargo.toml"
for features in ("", "runtime", "mailbox", "runtime,mailbox", "host,mailbox"):
    flags = ["--locked", "--manifest-path", str(manifest), "--no-default-features"]
    if features:
        flags += ["--features", features]
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", *flags, "--format-version", "1"], text=True
    ))
    packages = {package["id"]: package["name"] for package in metadata["packages"]}
    selected = {packages[node["id"]] for node in metadata["resolve"]["nodes"]}
    host = "host" in features
    runtime = host or "runtime" in features
    mailbox = "mailbox" in features
    for package, expected in {
        "tokio": runtime,
        "personal-rns": runtime,
        "prns-host-runtime": host,
        "prns-runtime-tokio": host,
        "redb": mailbox,
    }.items():
        if (package in selected) != expected:
            raise SystemExit(f"{features or 'portable'}: unexpected {package} presence")
    if any(name.startswith(("prns-chat-app", "prns-groups", "prns-service-runtime")) for name in selected):
        raise SystemExit("LXMF consumer acquired application/group/native composition")
    print(f"LXMF isolated closure: {features or 'portable'}", flush=True)
    subprocess.run(["cargo", "check", *flags, "--lib"], check=True)
