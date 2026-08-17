from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "validation" / "release" / "acceptance-doc-contracts.py"


def load_checker():
    spec = importlib.util.spec_from_file_location("acceptance_doc_contracts", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not import {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CHECKER = load_checker()


class AcceptanceDocContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.path = self.root / "acceptance.md"
        self.contracts = (
            CHECKER.CountContract(
                "acceptance.md",
                "Acceptance requires ",
                "physical",
                " physical rows.",
            ),
        )
        self.derived = {"physical": 12}

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def check(self) -> list[str]:
        return CHECKER.check(self.root, self.derived, self.contracts)

    def test_accepts_spelled_and_decimal_counts(self) -> None:
        for count in ("twelve", "12"):
            with self.subTest(count=count):
                self.path.write_text(
                    f"Acceptance requires {count} physical rows.\n",
                    encoding="utf-8",
                )
                self.assertEqual(self.check(), [])

    def test_rejects_wrong_count_at_source_line(self) -> None:
        self.path.write_text(
            "# Acceptance\n\nAcceptance requires eleven physical rows.\n",
            encoding="utf-8",
        )
        self.assertEqual(
            self.check(),
            [
                "acceptance.md:3: expected derived physical count 12, found eleven",
            ],
        )

    def test_rejects_missing_registered_anchor(self) -> None:
        self.path.write_text(
            "Acceptance needs twelve physical rows.\n",
            encoding="utf-8",
        )
        self.assertEqual(
            self.check(),
            [
                "acceptance.md: cannot find the registered physical count between "
                "'Acceptance requires ' and ' physical rows.'",
            ],
        )

    def test_rejects_duplicate_registered_count(self) -> None:
        self.path.write_text(
            "Acceptance requires twelve physical rows.\n"
            "Acceptance requires twelve physical rows.\n",
            encoding="utf-8",
        )
        self.assertEqual(
            self.check(),
            [
                "acceptance.md:2: the registered physical count occurs more than once",
            ],
        )

    def test_reports_missing_document(self) -> None:
        errors = self.check()
        self.assertEqual(len(errors), 1)
        self.assertTrue(errors[0].startswith("acceptance.md: cannot read governed document:"))

    def test_repository_documents_match_derived_counts(self) -> None:
        self.assertEqual(
            CHECKER.check(
                ROOT,
                CHECKER.derived_counts(ROOT),
                CHECKER.COUNT_CONTRACTS,
            ),
            [],
        )

    def test_installer_roster_count_tracks_published_targets(self) -> None:
        targets = dict(CHECKER.CLI_TARGETS)
        targets["second-x86_64-linux"] = ("linux", "x86_64")
        with patch.object(CHECKER, "CLI_TARGETS", targets):
            derived = CHECKER.derived_counts(ROOT)
        self.assertEqual(derived["installer_roster"], len(targets))


if __name__ == "__main__":
    unittest.main()
