from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "validation" / "hygiene" / "embedded_assurance_selection.py"
SPEC = importlib.util.spec_from_file_location("embedded_assurance_selection", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
selection = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = selection
SPEC.loader.exec_module(selection)


class EmbeddedAssuranceSelectionTests(unittest.TestCase):
    def test_dependency_closure_covers_every_resource_input_family(self) -> None:
        paths = {
            "personal-hopspot/embedded/nrf52840/src/lib.rs",
            "personal-hopspot/assurance-kernel/src/lib.rs",
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
        selected = selection.selection_for_paths(paths)
        self.assertEqual(selected.resources, tuple(sorted(paths)))

    def test_lanes_are_selected_independently(self) -> None:
        selected = selection.selection_for_paths(
            {"personal-hopspot/resources/src/report/model.rs"}
        )

        self.assertTrue(selected.required(selection.Lane.RESOURCES))
        self.assertFalse(selected.required(selection.Lane.MIRI))
        self.assertFalse(selected.required(selection.Lane.ISA))
        self.assertFalse(selected.required(selection.Lane.PILOTS))

        selected = selection.selection_for_paths({"validation/hardening/miri.sh"})
        self.assertFalse(selected.required(selection.Lane.RESOURCES))
        self.assertTrue(selected.required(selection.Lane.MIRI))
        self.assertFalse(selected.required(selection.Lane.ISA))
        self.assertFalse(selected.required(selection.Lane.PILOTS))

        selected = selection.selection_for_paths(
            {
                "validation/hardening/embedded-miri.toml",
                "validation/hardening/embedded_miri.py",
            }
        )
        self.assertEqual(
            selected.miri,
            (
                "validation/hardening/embedded-miri.toml",
                "validation/hardening/embedded_miri.py",
            ),
        )
        self.assertFalse(selected.required(selection.Lane.RESOURCES))
        self.assertFalse(selected.required(selection.Lane.ISA))
        self.assertFalse(selected.required(selection.Lane.PILOTS))

        selected = selection.selection_for_paths(
            {
                "personal-hopspot/assurance-kernel/src/lib.rs",
                "validation/hardening/embedded-isa.toml",
                "validation/hardening/embedded_isa/architecture/thumbv7em.py",
            }
        )
        self.assertEqual(
            selected.isa,
            (
                "personal-hopspot/assurance-kernel/src/lib.rs",
                "validation/hardening/embedded-isa.toml",
                "validation/hardening/embedded_isa/architecture/thumbv7em.py",
            ),
        )
        self.assertEqual(
            selected.resources,
            ("personal-hopspot/assurance-kernel/src/lib.rs",),
        )
        self.assertFalse(selected.required(selection.Lane.MIRI))
        self.assertEqual(
            selected.pilots,
            (
                "personal-hopspot/assurance-kernel/src/lib.rs",
                "validation/hardening/embedded-isa.toml",
            ),
        )

        selected = selection.selection_for_paths(
            {
                "validation/hardening/embedded-isa.toml",
                "validation/hardening/embedded-platform.toml",
                "validation/hardening/embedded_isa/emulator.py",
                "validation/hardening/embedded_platform/platform/nrf52840.py",
            }
        )
        self.assertEqual(
            selected.pilots,
            (
                "validation/hardening/embedded-isa.toml",
                "validation/hardening/embedded-platform.toml",
                "validation/hardening/embedded_isa/emulator.py",
                "validation/hardening/embedded_platform/platform/nrf52840.py",
            ),
        )
        self.assertFalse(selected.required(selection.Lane.RESOURCES))
        self.assertFalse(selected.required(selection.Lane.MIRI))
        self.assertEqual(
            selected.isa,
            (
                "validation/hardening/embedded-isa.toml",
                "validation/hardening/embedded_isa/emulator.py",
            ),
        )

        selected = selection.selection_for_paths(
            {"personal-hopspot/builder/src/architecture/thumbv7em.rs"}
        )
        self.assertTrue(selected.required(selection.Lane.RESOURCES))
        self.assertFalse(selected.required(selection.Lane.MIRI))
        self.assertTrue(selected.required(selection.Lane.ISA))
        self.assertTrue(selected.required(selection.Lane.PILOTS))

        selected = selection.selection_for_paths(
            {
                "prns-flash-manifest/src/catalog/mod.rs",
                "release/flash/boards.json",
            }
        )
        self.assertEqual(
            selected.resources,
            (
                "prns-flash-manifest/src/catalog/mod.rs",
                "release/flash/boards.json",
            ),
        )
        self.assertEqual(selected.pilots, selected.resources)
        self.assertFalse(selected.required(selection.Lane.MIRI))
        self.assertFalse(selected.required(selection.Lane.ISA))

        selected = selection.selection_for_paths(
            {
                "validation/hardening/embedded_architectures.py",
                "validation/hardening/embedded_readiness/contract.py",
            }
        )
        self.assertTrue(selected.required(selection.Lane.RESOURCES))
        self.assertEqual(
            selected.isa,
            (
                "validation/hardening/embedded_architectures.py",
                "validation/hardening/embedded_readiness/contract.py",
            ),
        )
        self.assertFalse(selected.required(selection.Lane.MIRI))

        selected = selection.selection_for_paths(
            {"validation/hardening/embedded_readiness/prepare.py"}
        )
        self.assertFalse(selected.required(selection.Lane.RESOURCES))
        self.assertFalse(selected.required(selection.Lane.MIRI))
        self.assertTrue(selected.required(selection.Lane.ISA))
        self.assertTrue(selected.required(selection.Lane.PILOTS))

        selected = selection.selection_for_paths({"validation/run.py"})
        for lane in selection.Lane:
            self.assertTrue(selected.required(lane))

    def test_unrelated_surfaces_do_not_select_resource_linking(self) -> None:
        self.assertEqual(
            selection.selection_for_paths(
                {
                    "docs/architecture.md",
                    "personal-hopspot/mobile/ios/README.md",
                    "prns-runtime/impls/tokio/src/lib.rs",
                }
            ).resources,
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
                        selected = selection.github_selection(
                            {
                                "GITHUB_EVENT_NAME": event_name,
                                "GITHUB_EVENT_PATH": str(event_path),
                                "GITHUB_SHA": "b" * 40,
                            }
                        )
                    self.assertTrue(selected.required(selection.Lane.RESOURCES))
                    self.assertEqual(
                        selected.resources, ("personal-hopspot/core/src/lib.rs",)
                    )
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
                selected = selection.github_selection(
                    {
                        "GITHUB_EVENT_NAME": "push",
                        "GITHUB_EVENT_PATH": str(event_path),
                        "GITHUB_SHA": "b" * 40,
                    }
                )
                for lane in selection.Lane:
                    self.assertFalse(selected.required(lane))

    def test_manual_dispatch_always_selects_the_matrix(self) -> None:
        selected = selection.github_selection(
            {"GITHUB_EVENT_NAME": "workflow_dispatch"}
        )
        for lane in selection.Lane:
            self.assertTrue(selected.required(lane))
            self.assertEqual(selected.paths(lane), ())

    def test_main_exports_every_lane_and_the_resource_compatibility_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output"
            selected = selection.selection_for_paths(
                {"personal-hopspot/resources/src/report/model.rs"}
            )
            with mock.patch.object(
                selection, "github_selection", return_value=selected
            ), mock.patch.dict(selection.os.environ, {"GITHUB_OUTPUT": str(output)}):
                self.assertEqual(selection.main(), 0)
            self.assertEqual(
                output.read_text(encoding="utf-8").splitlines(),
                [
                    "resources_required=true",
                    "miri_required=false",
                    "isa_required=false",
                    "pilots_required=false",
                    "required=true",
                ],
            )

    def test_commit_ranges_are_strictly_validated(self) -> None:
        with self.assertRaises(selection.SelectionError):
            selection.changed_paths("main", "b" * 40)


if __name__ == "__main__":
    unittest.main()
