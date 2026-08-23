from __future__ import annotations

import io
import os
import sys
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest import mock

from validation.interop.host_contract import (
    HOST_C_STATIC_LIBRARY,
    PACKAGE_HOST_NATIVE,
    HostContractFailure,
    HostContractFailureKind,
    build_host_library,
    dynamic_loader_variable,
    dynamic_library_name,
    environment,
    host_contract_main,
    package_host_native,
    run_command,
    swift_host_target,
)


class HostContractTests(unittest.TestCase):
    def test_environment_inherits_and_overrides_values(self) -> None:
        with mock.patch.dict(os.environ, {"INHERITED": "yes", "OVERRIDDEN": "old"}, clear=True):
            configured = environment({"OVERRIDDEN": "new", "NUMBER": 7})
        self.assertEqual(
            configured,
            {"INHERITED": "yes", "OVERRIDDEN": "new", "NUMBER": "7"},
        )

    def test_command_failure_is_structured(self) -> None:
        with mock.patch("validation.interop.host_contract.subprocess.run") as process:
            process.return_value.returncode = 7
            with self.assertRaises(HostContractFailure) as raised:
                run_command(("tool", "argument"), "contract failed")
        self.assertEqual(raised.exception.kind, HostContractFailureKind.COMMAND_FAILED)
        self.assertIn("status 7", raised.exception.detail)

    def test_host_library_build_uses_the_owned_manifest(self) -> None:
        with mock.patch("validation.interop.host_contract.run_command") as run:
            build_host_library()
        command = run.call_args.args[0]
        self.assertEqual(command[:3], ("cargo", "build", "--manifest-path"))
        self.assertEqual(command[-1], "--locked")

    def test_dynamic_library_name_follows_the_host(self) -> None:
        with mock.patch("validation.interop.host_contract.sys.platform", "darwin"):
            self.assertEqual(dynamic_library_name(), "libprns_host.dylib")
        with mock.patch("validation.interop.host_contract.sys.platform", "linux"):
            with mock.patch("validation.interop.host_contract.os.name", "posix"):
                self.assertEqual(dynamic_library_name(), "libprns_host.so")

    def test_dynamic_loader_variable_follows_the_host(self) -> None:
        with mock.patch("validation.interop.host_contract.sys.platform", "darwin"):
            self.assertEqual(dynamic_loader_variable(), "DYLD_LIBRARY_PATH")
        with mock.patch("validation.interop.host_contract.sys.platform", "linux"):
            self.assertEqual(dynamic_loader_variable(), "LD_LIBRARY_PATH")

    def test_swift_host_target_follows_supported_hosts(self) -> None:
        expected_targets = (
            ("Darwin", "arm64", "aarch64-apple-darwin", "libprns_host.dylib"),
            ("Darwin", "x86_64", "x86_64-apple-darwin", "libprns_host.dylib"),
            ("Linux", "aarch64", "aarch64-unknown-linux-gnu", "libprns_host.so"),
            ("Linux", "x86_64", "x86_64-unknown-linux-gnu", "libprns_host.so"),
        )
        for system, machine, rust_target, dynamic_library_name in expected_targets:
            with self.subTest(system=system, machine=machine):
                with mock.patch(
                    "validation.interop.host_contract.platform.system",
                    return_value=system,
                ):
                    with mock.patch(
                        "validation.interop.host_contract.platform.machine",
                        return_value=machine,
                    ):
                        target = swift_host_target()
                self.assertEqual(target.rust_target, rust_target)
                self.assertEqual(target.dynamic_library_name, dynamic_library_name)

    def test_swift_host_target_rejects_unsupported_hosts(self) -> None:
        with mock.patch("validation.interop.host_contract.platform.system", return_value="Plan9"):
            with mock.patch(
                "validation.interop.host_contract.platform.machine",
                return_value="mips",
            ):
                with self.assertRaises(HostContractFailure) as raised:
                    swift_host_target()
        self.assertEqual(raised.exception.kind, HostContractFailureKind.UNSUPPORTED_PLATFORM)
        self.assertIn("Plan9-mips", raised.exception.detail)

    def test_native_package_uses_both_host_libraries(self) -> None:
        output = Path("/temporary/native")
        dynamic_library = Path("/temporary/libprns_host.so")
        with mock.patch("validation.interop.host_contract.run_command") as run:
            package_host_native(
                output=output,
                rust_target="x86_64-unknown-linux-gnu",
                dynamic_library=dynamic_library,
            )
        run.assert_called_once_with(
            (
                sys.executable,
                PACKAGE_HOST_NATIVE,
                "--target",
                "x86_64-unknown-linux-gnu",
                "--library",
                dynamic_library,
                "--library",
                HOST_C_STATIC_LIBRARY,
                "--output",
                output,
            ),
            "Prns host native package for x86_64-unknown-linux-gnu failed",
        )

    def test_contract_main_reports_a_structured_failure(self) -> None:
        stderr = io.StringIO()

        def fail() -> None:
            raise HostContractFailure(HostContractFailureKind.COMMAND_FAILED, "evidence")

        with redirect_stderr(stderr):
            status = host_contract_main(fail, "PASS")
        self.assertEqual(status, 1)
        self.assertIn("command failed: evidence", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
