from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "unsafe_audit", ROOT / "validation/security/unsafe-audit.py"
)
assert SPEC is not None and SPEC.loader is not None
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)


class UnsafeAuditTests(unittest.TestCase):
    def test_package_counts_exclude_nested_crates_but_keep_owned_test_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sources = {
                "src/lib.rs": "unsafe { owner(); }",
                "tests/owned.rs": "unsafe fn owned_test() {}",
                "member/src/lib.rs": "unsafe impl Send for Member {}",
                "vendor/dependency/src/lib.rs": 'unsafe extern "C" {}',
                "tests/fixtures/consumer/src/lib.rs": "unsafe { fixture(); }",
                "target/ignored.rs": "unsafe { build_output(); }",
                ".git/ignored.rs": "unsafe { metadata(); }",
            }
            for relative, source in sources.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(source)
            for relative in (".", "member", "vendor/dependency", "tests/fixtures/consumer"):
                (root / relative / "Cargo.toml").write_text("[package]\nname = 'fixture'\n")
            self.assertEqual(audit.unsafe_counts(root), {
                "tokens": 2, "blocks": 1, "functions": 1, "impls": 0, "externs": 0,
            })
            for relative, kind in (("member", "impls"), ("vendor/dependency", "externs"),
                                   ("tests/fixtures/consumer", "blocks")):
                expected = {"tokens": 1, "blocks": 0, "functions": 0, "impls": 0, "externs": 0}
                expected[kind] = 1
                self.assertEqual(audit.unsafe_counts(root / relative), expected)
