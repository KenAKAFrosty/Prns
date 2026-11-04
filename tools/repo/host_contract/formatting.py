"""Formatting shared by the canonical Rust contract generators."""

import subprocess


def format_rust(source):
    return subprocess.run(
        ["rustfmt", "--edition", "2021"],
        input=source,
        text=True,
        capture_output=True,
        check=True,
    ).stdout
