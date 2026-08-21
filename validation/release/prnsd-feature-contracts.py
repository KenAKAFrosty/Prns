#!/usr/bin/env python3
"""Prove the complete and cloud prnsd feature profiles do not drift."""

from __future__ import annotations

import subprocess
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "prnsd" / "Cargo.toml"

CLOUD_FEATURES = {
    "std",
    "personal-rns/tokio-host",
    "personal-rns/config",
    "personal-rns/parallel-persistence",
    "personal-rns/shared-instance",
    "personal-rns/tcp",
    "personal-rns/udp",
    "personal-rns/pipe",
    "personal-rns/backbone",
    "personal-rns/i2p",
    "personal-rns/websocket",
    "personal-rns/browser-rendezvous",
    "personal-rns/interface-discovery-archive",
    "personal-rns/signed-artifact",
    "personal-rns/rnx",
    "personal-rns/parallel-resource-hash",
    "dep:clap",
    "dep:tokio",
}
OTLP_FEATURES = {
    "observability",
    "personal-rns/runtime-metrics",
    "dep:opentelemetry",
    "dep:opentelemetry-otlp",
    "dep:opentelemetry_sdk",
    "dep:tracing-opentelemetry",
}
CLOUD_IMAGE_FEATURES = "tokio-cloud-host,observability,otlp"
LOCAL_FEATURES = {
    "personal-rns/serial",
    "personal-rns/kiss",
    "personal-rns/ax25",
    "personal-rns/rnode",
    "personal-rns/weave",
    "personal-rns/wifi-auto",
    "personal-rns/wifi-auto-mdns",
    "personal-rns/usb",
    "personal-rns/bluetooth-auto",
    "dep:dbus",
}
FORBIDDEN_CLOUD_PACKAGES = {
    "bluer",
    "bluez-async",
    "bluez-generated",
    "btleplug",
    "dbus",
    "dbus-crossroads",
    "dbus-tokio",
    "hidapi",
    "ksni",
    "libdbus-sys",
    "nusb",
    "prns-ffi",
    "serial2",
    "serial2-tokio",
    "tray-icon",
    "winit",
}
REQUIRED_COMPLETE_PACKAGES = {
    "bluer",
    "btleplug",
    "dbus",
    "libdbus-sys",
    "serial2",
    "serial2-tokio",
}
REQUIRED_OTLP_PACKAGES = {
    "opentelemetry",
    "opentelemetry-otlp",
    "opentelemetry_sdk",
    "tracing-opentelemetry",
}


def dependency_packages(features: str) -> set[str]:
    output = subprocess.run(
        [
            "cargo",
            "tree",
            "--manifest-path",
            str(MANIFEST),
            "--target",
            "x86_64-unknown-linux-gnu",
            "--no-default-features",
            "--features",
            features,
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout
    return {
        line.split(maxsplit=1)[0]
        for line in output.splitlines()
        if line and not line.startswith("[")
    }


def main() -> int:
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    features = manifest["features"]
    cloud = set(features["tokio-cloud-host"])
    complete = set(features["tokio-host"])
    otlp = set(features["otlp"])
    if cloud != CLOUD_FEATURES:
        raise ValueError(
            "tokio-cloud-host feature surface drifted: "
            f"missing={sorted(CLOUD_FEATURES - cloud)}, "
            f"unexpected={sorted(cloud - CLOUD_FEATURES)}"
        )
    if complete != {"tokio-cloud-host", *LOCAL_FEATURES}:
        raise ValueError(
            "tokio-host must remain tokio-cloud-host plus all local-device capabilities"
        )
    if otlp != OTLP_FEATURES:
        raise ValueError(
            "otlp feature surface drifted: "
            f"missing={sorted(OTLP_FEATURES - otlp)}, "
            f"unexpected={sorted(otlp - OTLP_FEATURES)}"
        )
    required = set(manifest["bin"][0]["required-features"])
    if required != {"tokio-cloud-host", "observability"}:
        raise ValueError("the prnsd binary must be available to the cloud-host profile")

    dockerfile = (ROOT / "Dockerfile").read_text(encoding="utf-8")
    if f"--features {CLOUD_IMAGE_FEATURES}" not in dockerfile:
        raise ValueError(
            "the official cloud image must compile the opt-in OTLP capability"
        )

    cloud_packages = dependency_packages(CLOUD_IMAGE_FEATURES)
    leaked = cloud_packages & FORBIDDEN_CLOUD_PACKAGES
    if leaked:
        raise ValueError(f"tokio-cloud-host leaked local-device packages: {sorted(leaked)}")
    missing_otlp = REQUIRED_OTLP_PACKAGES - cloud_packages
    if missing_otlp:
        raise ValueError(
            f"the official cloud image lost OTLP packages: {sorted(missing_otlp)}"
        )

    complete_packages = dependency_packages("tokio-host,observability,tray")
    missing = REQUIRED_COMPLETE_PACKAGES - complete_packages
    if missing:
        raise ValueError(f"tokio-host lost complete host packages: {sorted(missing)}")

    print("prnsd cloud image, tokio-cloud-host, OTLP, and tokio-host feature contracts are exact")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
