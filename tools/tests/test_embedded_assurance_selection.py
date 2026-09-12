from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import tomllib
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
    def suite_ids(self, paths: set[str], lane: selection.Lane) -> tuple[str, ...]:
        return selection.selection_for_paths(paths).suite_ids(lane)

    def test_resource_dependency_closure_selects_both_production_build_suites(self) -> None:
        paths = {
            "personal-hopspot/embedded/nrf52840/src/lib.rs",
            "personal-hopspot/assurance-kernel/src/lib.rs",
            "personal-hopspot/memory/src/profiles/nrf52840/mod.rs",
            "prns-flash-manifest/src/catalog/mod.rs",
            "personal-hopspot/builder/src/toolchain/mod.rs",
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
        self.assertEqual(
            selected.suite_ids(selection.Lane.RESOURCES),
            ("embedded-builds", "esp32-firmware-check"),
        )
        self.assertEqual(selected.paths(selection.Lane.RESOURCES), tuple(sorted(paths)))

    def test_every_classifier_suite_is_owned_by_the_validation_registry(self) -> None:
        manifest = tomllib.loads(
            (ROOT / "validation/manifest.toml").read_text(encoding="utf-8")
        )
        registered = {suite["id"] for suite in manifest["suite"]}
        self.assertLessEqual({suite.value for suite in selection.Suite}, registered)

    def test_board_firmware_selects_only_its_instruction_architecture(self) -> None:
        cases = (
            (
                "personal-hopspot/embedded/nrf52840/src/bin/t096.rs",
                ("embedded-isa-thumbv7em",),
                ("embedded-platform-nrf52840",),
            ),
            (
                "personal-hopspot/embedded/esp32/boards/xiao-esp32-c6/src/main.rs",
                ("embedded-isa-riscv32imac",),
                (),
            ),
            (
                "personal-hopspot/embedded/esp32/boards/heltec-v4/src/main.rs",
                ("embedded-isa-xtensa-esp32s3",),
                ("embedded-platform-esp32s3",),
            ),
        )
        for path, isa, pilots in cases:
            with self.subTest(path=path):
                selected = selection.selection_for_paths({path})
                self.assertEqual(
                    selected.suite_ids(selection.Lane.RESOURCES),
                    ("embedded-builds", "esp32-firmware-check"),
                )
                self.assertEqual(selected.suite_ids(selection.Lane.MIRI), ())
                self.assertEqual(selected.suite_ids(selection.Lane.ISA), isa)
                self.assertEqual(selected.suite_ids(selection.Lane.PILOTS), pilots)

    def test_shared_component_change_selects_complete_required_evidence(self) -> None:
        selected = selection.selection_for_paths(
            {"prns-interfaces/impls/embassy/src/radios/sx126x.rs"}
        )
        self.assertEqual(
            selected.suite_ids(selection.Lane.RESOURCES),
            ("embedded-builds", "esp32-firmware-check"),
        )
        self.assertEqual(
            selected.suite_ids(selection.Lane.MIRI), ("embedded-miri-quick",)
        )
        self.assertEqual(
            selected.suite_ids(selection.Lane.ISA),
            (
                "embedded-isa-thumbv7em",
                "embedded-isa-riscv32imac",
                "embedded-isa-xtensa-esp32s3",
            ),
        )
        self.assertTrue(selected.complete_required_evidence())

    def test_architecture_runner_changes_select_one_isa_suite(self) -> None:
        cases = (
            ("thumbv7em.py", "embedded-isa-thumbv7em"),
            ("riscv32imac.py", "embedded-isa-riscv32imac"),
            ("xtensa_esp32s3.py", "embedded-isa-xtensa-esp32s3"),
        )
        for filename, suite in cases:
            path = f"validation/hardening/embedded_isa/architecture/{filename}"
            with self.subTest(path=path):
                selected = selection.selection_for_paths({path})
                self.assertEqual(selected.suite_ids(selection.Lane.ISA), (suite,))

    def test_memory_profiles_select_their_consuming_architectures_and_pilots(self) -> None:
        cases = (
            (
                "personal-hopspot/memory/src/profiles/nrf52840/mod.rs",
                ("embedded-isa-thumbv7em",),
                ("embedded-platform-nrf52840",),
            ),
            (
                "personal-hopspot/memory/src/profiles/espressif/mod.rs",
                ("embedded-isa-riscv32imac", "embedded-isa-xtensa-esp32s3"),
                ("embedded-platform-esp32s3",),
            ),
            (
                "personal-hopspot/memory/src/profiles/linker/mod.rs",
                (
                    "embedded-isa-thumbv7em",
                    "embedded-isa-riscv32imac",
                    "embedded-isa-xtensa-esp32s3",
                ),
                ("embedded-platform-nrf52840", "embedded-platform-esp32s3"),
            ),
        )
        for path, isa, pilots in cases:
            with self.subTest(path=path):
                selected = selection.selection_for_paths({path})
                self.assertEqual(selected.suite_ids(selection.Lane.ISA), isa)
                self.assertEqual(selected.suite_ids(selection.Lane.PILOTS), pilots)

    def test_catalog_change_selects_resources_and_the_catalog_consuming_pilot(self) -> None:
        selected = selection.selection_for_paths(
            {"prns-flash-manifest/src/catalog/mod.rs", "release/flash/boards.json"}
        )
        self.assertEqual(
            selected.suite_ids(selection.Lane.RESOURCES),
            ("embedded-builds", "esp32-firmware-check"),
        )
        self.assertEqual(selected.suite_ids(selection.Lane.MIRI), ())
        self.assertEqual(selected.suite_ids(selection.Lane.ISA), ())
        self.assertEqual(
            selected.suite_ids(selection.Lane.PILOTS),
            ("embedded-platform-esp32s3",),
        )

    def test_embassy_interface_and_runtime_changes_select_required_proofs(self) -> None:
        for path in (
            "prns-interfaces/impls/embassy/src/radios/lr1110/mod.rs",
            "prns-runtime/impls/embassy/src/runtime/embedded_persistence.rs",
        ):
            with self.subTest(path=path):
                selected = selection.selection_for_paths({path})
                self.assertTrue(selected.required(selection.Lane.RESOURCES))
                self.assertTrue(selected.required(selection.Lane.MIRI))
                self.assertEqual(len(selected.suites(selection.Lane.ISA)), 3)
                self.assertTrue(selected.complete_required_evidence())

    def test_inventories_select_their_exact_consumers(self) -> None:
        isa = selection.selection_for_paths(
            {"validation/hardening/embedded-isa.toml"}
        )
        self.assertEqual(len(isa.suites(selection.Lane.ISA)), 3)
        self.assertEqual(
            isa.suite_ids(selection.Lane.PILOTS), ("embedded-platform-esp32s3",)
        )

        pilots = selection.selection_for_paths(
            {"validation/hardening/embedded-platform.toml"}
        )
        self.assertEqual(
            pilots.suite_ids(selection.Lane.PILOTS),
            ("embedded-platform-nrf52840", "embedded-platform-esp32s3"),
        )
        self.assertFalse(pilots.required(selection.Lane.RESOURCES))

        failure = selection.selection_for_paths(
            {"validation/hardening/embedded_failure.py"}
        )
        self.assertEqual(
            failure.suite_ids(selection.Lane.MIRI), ("embedded-miri-quick",)
        )
        self.assertEqual(len(failure.suites(selection.Lane.ISA)), 3)
        self.assertEqual(len(failure.suites(selection.Lane.PILOTS)), 2)

    def test_selected_suites_retain_their_matched_path_reasons(self) -> None:
        paths = {
            "personal-hopspot/embedded/nrf52840/src/lib.rs",
            "personal-hopspot/memory/src/profiles/nrf52840/mod.rs",
        }
        selected = selection.selection_for_paths(paths)
        thumb = selected.suites(selection.Lane.ISA)
        self.assertEqual(
            thumb,
            (
                selection.SelectedSuite(
                    selection.Suite.ISA_THUMBV7EM,
                    tuple(sorted(paths)),
                ),
            ),
        )

    def test_unrelated_paths_select_no_embedded_suite(self) -> None:
        selected = selection.selection_for_paths(
            {
                "docs/architecture.md",
                "personal-hopspot/mobile/ios/README.md",
                "prns-runtime/impls/tokio/src/lib.rs",
            }
        )
        for lane in selection.Lane:
            self.assertFalse(selected.required(lane))

    def test_pull_request_and_push_events_use_the_same_classifier(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            event_path = Path(directory) / "event.json"
            cases = (
                ("pull_request", {"pull_request": {"base": {"sha": "a" * 40}}}),
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
                    self.assertTrue(selected.complete_required_evidence())
                    changed.assert_called_once_with("a" * 40, "b" * 40)

    def test_manual_dispatch_selects_every_suite_without_inventing_paths(self) -> None:
        selected = selection.github_selection(
            {"GITHUB_EVENT_NAME": "workflow_dispatch"}
        )
        self.assertEqual(
            tuple(item.suite for item in selected.selected), tuple(selection.Suite)
        )
        self.assertTrue(selected.complete_required_evidence())
        self.assertTrue(all(not item.matched_paths for item in selected.selected))

    def test_main_exports_exact_suites_and_derived_booleans(self) -> None:
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
                    "resources_suites=embedded-builds esp32-firmware-check",
                    "miri_required=false",
                    "miri_suites=",
                    "isa_required=false",
                    "isa_suites=",
                    "pilots_required=false",
                    "pilots_suites=",
                    "embedded_builds_required=true",
                    "esp32_firmware_check_required=true",
                    "aggregate_required=false",
                    "required=true",
                ],
            )

    def test_commit_ranges_are_strictly_validated(self) -> None:
        with self.assertRaises(selection.SelectionError):
            selection.changed_paths("main", "b" * 40)


if __name__ == "__main__":
    unittest.main()
