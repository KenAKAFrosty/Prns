from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

GENERIC_PAGES = ("help.html", "settings.html")
CURRENT_CRATE = re.compile(rb'data-current-crate="[^"]+"')
CRATE_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def normalize_generic_pages(output: Path, current_crate: str) -> None:
    if not output.is_dir():
        raise ValueError("Rustdoc output directory is unavailable")
    if CRATE_NAME.fullmatch(current_crate) is None:
        raise ValueError("Rustdoc current crate is invalid")
    if not (output / current_crate / "index.html").is_file():
        raise ValueError("Rustdoc current crate is absent from the output")

    replacement = f'data-current-crate="{current_crate}"'.encode()
    normalized: dict[Path, bytes] = {}
    for relative in GENERIC_PAGES:
        path = output / relative
        if not path.is_file():
            raise ValueError(f"Rustdoc generic page is unavailable: {relative}")
        value, replacements = CURRENT_CRATE.subn(replacement, path.read_bytes())
        if replacements != 1:
            raise ValueError(
                f"Rustdoc generic page has {replacements} current-crate fields: {relative}"
            )
        normalized[path] = value

    for path, value in normalized.items():
        path.write_bytes(value)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--current-crate", required=True)
    arguments = parser.parse_args()
    try:
        normalize_generic_pages(arguments.output, arguments.current_crate)
    except (OSError, ValueError) as error:
        print(f"Rustdoc normalization failed: {error}", file=sys.stderr)
        return 1
    print(f"normalized Rustdoc generic pages to {arguments.current_crate}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
