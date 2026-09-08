from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "validation" / "hygiene" / "embedded_resource_selection.py"
SPEC = importlib.util.spec_from_file_location("embedded_resource_selection", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
selection = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = selection
SPEC.loader.exec_module(selection)


class EmbeddedResourceSelectionTests(unittest.TestCase):
    def test_dependency_closure_covers_every_resource_input_family(self) -> None:
        paths = {
            "personal-hopspot/embedded/nrf52840/src/lib.rs",
            "personal-hopspot/memory/src/profiles/nrf52840.rs",
            "prns-flash-manifest/src/catalog.rs",
            "personal-hopspot/builder/src/toolchain.rs",
            "personal-hopspot/resources/src/main.rs",
            "personal-hopspot/core/src/lib.rs",
            "personal-rns/src/lib.rs",
            "prns-core/src/lib.rs",
            "prns-macros/src/lib.rs",
            "prns-interfaces/impls/embassy/src/lib.rs",
            "prns-runtime/core/src/lib.rs",
            "prns-runtime/impls/embassy/src/lib.rs",
            "Cargo.lock",
            "rust-toolchain.toml",
            "tools/release/release-esp-toolchain-identity.sh",
            "validation/platforms/no-std-esp-build.sh",
        }
        self.assertEqual(selection.affected_paths(paths), tuple(sorted(paths)))

    def test_unrelated_surfaces_do_not_select_resource_linking(self) -> None:
        self.assertEqual(
            selection.affected_paths(
                {
                    "docs/architecture.md",
                    "personal-hopspot/mobile/ios/README.md",
                    "prns-runtime/impls/tokio/src/lib.rs",
                }
            ),
            (),
        )

    def test_pull_request_and_push_events_use_the_same_classifier(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            event_path = Path(directory) / "event.json"
            cases = (
                (
                    "pull_request",
                    {"pull_request": {"base": {"sha": "a" * 40}}},
                ),
                ("push", {"before": "a" * 40}),
            )
            for event_name, event in cases:
                with self.subTest(event=event_name):
                    event_path.write_text(json.dumps(event), encoding="utf-8")
                    with mock.patch.object(
                        selection,
                        "changed_paths",
                        return_value=("personal-hopspot/core/src/lib.rs",),
                    ) as changed:
                        required, matches = selection.github_selection(
                            {
                                "GITHUB_EVENT_NAME": event_name,
                                "GITHUB_EVENT_PATH": str(event_path),
                                "GITHUB_SHA": "b" * 40,
                            }
                        )
                    self.assertTrue(required)
                    self.assertEqual(matches, ("personal-hopspot/core/src/lib.rs",))
                    changed.assert_called_once_with("a" * 40, "b" * 40)

    def test_ci_selection_is_false_for_an_unrelated_diff(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            event_path = Path(directory) / "event.json"
            event_path.write_text(json.dumps({"before": "a" * 40}), encoding="utf-8")
            with mock.patch.object(
                selection,
                "changed_paths",
                return_value=("docs/architecture.md",),
            ):
                self.assertEqual(
                    selection.github_selection(
                        {
                            "GITHUB_EVENT_NAME": "push",
                            "GITHUB_EVENT_PATH": str(event_path),
                            "GITHUB_SHA": "b" * 40,
                        }
                    ),
                    (False, ()),
                )

    def test_manual_dispatch_always_selects_the_matrix(self) -> None:
        self.assertEqual(
            selection.github_selection({"GITHUB_EVENT_NAME": "workflow_dispatch"}),
            (True, ()),
        )

    def test_commit_ranges_are_strictly_validated(self) -> None:
        with self.assertRaises(selection.SelectionError):
            selection.changed_paths("main", "b" * 40)


if __name__ == "__main__":
    unittest.main()
