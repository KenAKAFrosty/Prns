from __future__ import annotations

import io
import os
import unittest
from contextlib import redirect_stderr
from unittest import mock

from validation.interop.host_contract import (
    HostContractFailure,
    HostContractFailureKind,
    build_host_library,
    dynamic_library_name,
    environment,
    host_contract_main,
    run_command,
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
