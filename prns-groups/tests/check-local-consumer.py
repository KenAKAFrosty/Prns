#!/usr/bin/env python3
"""Check the isolated local consumer without workspace feature unification."""
import json
from pathlib import Path
import subprocess
import os

if os.name != "posix":
    raise SystemExit("LocalOwner fixture requires the implemented Unix private-file contract.")

manifest = Path(__file__).resolve().parent / "fixtures/local-consumer/Cargo.toml"
cargo = ["cargo", "--locked", "--manifest-path", str(manifest)]
metadata = json.loads(subprocess.check_output(
    [cargo[0], "metadata", *cargo[1:], "--format-version", "1"], text=True
))
packages = {package["id"]: package["name"] for package in metadata["packages"]}
selected = {packages[node["id"]]: node["features"] for node in metadata["resolve"]["nodes"]}
forbidden = {
    "prns-host-runtime", "prns-host-snapshot", "prns-runtime-tokio",
    "prns-interfaces-tokio", "personal-rns", "prns-group-text",
    "uniffi", "uniffi_core", "prns-identities",
}
unexpected = sorted(forbidden & selected.keys())
if unexpected:
    raise SystemExit(f"Local consumer includes forbidden dependencies: {unexpected}")
for package, features in {
    "prns-groups": [],
    "prns-groups-reticulum": [],
    "tokio": ["sync"],
}.items():
    if selected.get(package) != features:
        raise SystemExit(f"Unexpected features for {package}: {selected.get(package)}")
if "prns-storage" not in selected:
    raise SystemExit("Production local owner is missing its storage worker dependency.")
print("Production LocalOwner dependency boundary verified: no native host, text profile, or runtime.", flush=True)
subprocess.run([cargo[0], "test", *cargo[1:]], check=True)
