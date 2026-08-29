from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools" / "device" / "hopspot-dev-flasher.py"
SPEC = importlib.util.spec_from_file_location("hopspot_dev_flasher", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not import {SCRIPT}")
DEV = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEV
SPEC.loader.exec_module(DEV)


class SourceIdentityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.repository = Path(self.temporary.name)
        self.git("init")
        self.git("config", "user.email", "tests@example.test")
        self.git("config", "user.name", "Prns Tests")
        (self.repository / "VERSION").write_text("0.3.1\n", encoding="utf-8")
        (self.repository / ".gitignore").write_text("ignored/\n", encoding="utf-8")
        (self.repository / "tracked.txt").write_text("tracked\n", encoding="utf-8")
        self.git("add", ".")
        self.git("commit", "-m", "fixture")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def git(self, *arguments: str) -> None:
        subprocess.run(
            ["git", *arguments],
            cwd=self.repository,
            check=True,
            capture_output=True,
        )

    def test_identity_is_deterministic_and_tracks_worktree_content(self) -> None:
        clean = DEV.source_identity(self.repository)
        self.assertEqual(DEV.source_identity(self.repository), clean)
        self.assertEqual(clean.state, "clean")
        self.assertEqual(clean.version, f"0.3.1-dev.clean.{clean.digest}")

        (self.repository / "tracked.txt").write_text("changed\n", encoding="utf-8")
        tracked = DEV.source_identity(self.repository)
        self.assertEqual(tracked.state, "dirty")
        self.assertNotEqual(tracked.digest, clean.digest)

        (self.repository / "tracked.txt").write_text("tracked\n", encoding="utf-8")
        self.assertEqual(DEV.source_identity(self.repository), clean)

        (self.repository / "untracked.txt").write_text("untracked\n", encoding="utf-8")
        untracked = DEV.source_identity(self.repository)
        self.assertEqual(untracked.state, "dirty")
        self.assertNotEqual(untracked.digest, clean.digest)

        (self.repository / "untracked.txt").unlink()
        ignored = self.repository / "ignored" / "cache.bin"
        ignored.parent.mkdir()
        ignored.write_bytes(b"ignored")
        self.assertEqual(DEV.source_identity(self.repository), clean)

    def test_head_and_executable_identity_are_hashed(self) -> None:
        initial = DEV.source_identity(self.repository)
        tracked = self.repository / "tracked.txt"
        tracked.chmod(tracked.stat().st_mode | stat.S_IXUSR)
        executable = DEV.source_identity(self.repository)
        self.assertNotEqual(executable.digest, initial.digest)
        tracked.chmod(tracked.stat().st_mode & ~stat.S_IXUSR)
        self.git("commit", "--allow-empty", "-m", "new head")
        new_head = DEV.source_identity(self.repository)
        self.assertNotEqual(new_head.head, initial.head)
        self.assertNotEqual(new_head.digest, initial.digest)

    def test_changed_source_aborts_candidate(self) -> None:
        initial = DEV.source_identity(self.repository)
        (self.repository / "tracked.txt").write_text("changed\n", encoding="utf-8")
        final = DEV.source_identity(self.repository)
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "changed during the build"):
            DEV.require_unchanged_source(initial, final)

    def test_known_bad_source_digest_is_quarantined(self) -> None:
        digest = next(iter(DEV.QUARANTINED_SOURCE_DIGESTS))
        identity = DEV.SourceIdentity(
            head="0" * 40,
            digest=digest,
            state="dirty",
            version=f"0.3.1-dev.dirty.{digest}",
        )
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "quarantined"):
            DEV.require_unquarantined_source(identity)


class SelectionTests(unittest.TestCase):
    def test_explicit_selection_is_unique_known_and_canonical(self) -> None:
        selection = DEV.parse_selection(["t-echo", "heltec-v4", "--port", "1234"])
        self.assertEqual(selection.boards, ("heltec-v4", "t-echo"))
        self.assertEqual(selection.port, 1234)

    def test_all_selects_every_shipping_board(self) -> None:
        boards = DEV.shipping_boards()
        self.assertEqual(DEV.parse_selection(["--all"]).boards, boards)
        self.assertIn("t096", boards)
        self.assertIn("t1000-e", boards)
        self.assertNotIn("heltec-e290", boards)

    def test_qualification_board_requires_explicit_selection(self) -> None:
        selection = DEV.parse_selection(["heltec-e290"])
        self.assertEqual(selection.boards, ("heltec-e290",))
        self.assertEqual(
            tuple((target.board_slug, target.transport) for target in DEV.selected_targets(selection)),
            (("heltec-e290", "esp-serial"),),
        )

    def test_explicit_nordic_selection_uses_catalog_order(self) -> None:
        selection = DEV.parse_selection(["t1000-e", "t096"])
        self.assertEqual(selection.boards, ("t096", "t1000-e"))
        self.assertEqual(
            tuple((target.board_slug, target.transport) for target in DEV.selected_targets(selection)),
            (("t096", "uf2-mass-storage"), ("t1000-e", "nrf-serial-dfu")),
        )

    def test_missing_duplicate_unknown_and_invalid_port_are_rejected(self) -> None:
        for arguments in (
            [],
            ["heltec-v4", "heltec-v4"],
            ["unknown"],
            ["--all", "t-echo"],
            ["t-echo", "--port", "0"],
            ["t-echo", "--port", "65536"],
        ):
            with self.subTest(arguments=arguments), self.assertRaises(SystemExit):
                DEV.parse_selection(arguments)


class MinisignWorkflowTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def signer(self, version: str = "0.12") -> Path:
        path = self.root / f"minisign-{version}"
        path.write_text(
            f"""#!/usr/bin/env python3
import hashlib
from pathlib import Path
import sys

args = sys.argv[1:]
def value(flag):
    return args[args.index(flag) + 1]

if "-v" in args:
    print("minisign {version}")
elif "-G" in args:
    Path(value("-p")).write_text("untrusted comment: minisign public key 6B62D3410E007120\\nRWQgcQAOQdNia9cRKsl1wJxV2iODb6aBWOI1G0yDDk4ORXKecWSigfoy\\n")
    Path(value("-s")).write_text("untrusted comment: minisign secret key\\nTEST\\n")
elif "-S" in args:
    document = Path(value("-m")).read_bytes()
    Path(value("-x")).write_text(hashlib.sha256(document).hexdigest())
elif "-Vm" in args:
    document = Path(args[args.index("-Vm") + 1]).read_bytes()
    expected = hashlib.sha256(document).hexdigest()
    if Path(value("-x")).read_text() != expected:
        raise SystemExit(1)
else:
    raise SystemExit(2)
""",
            encoding="utf-8",
        )
        path.chmod(0o755)
        return path

    def test_requires_exact_minisign_version_and_prints_supported_install(self) -> None:
        valid = self.signer()
        self.assertEqual(
            DEV.require_minisign({"PRNS_MINISIGN_BIN": str(valid), "PATH": os.environ["PATH"]}),
            valid.resolve(),
        )
        wrong = self.signer("0.11")
        with self.assertRaisesRegex(
            DEV.DeveloperFlasherError,
            r"(?s)Minisign 0\.12 is required.*release\.toolchain\.minisign\.install",
        ):
            DEV.require_minisign(
                {"PRNS_MINISIGN_BIN": str(wrong), "PATH": os.environ["PATH"]}
            )
        with mock.patch.object(DEV, "PINNED_MINISIGN", self.root / "missing"):
            with self.assertRaisesRegex(
                DEV.DeveloperFlasherError,
                r"(?s)not found.*release\.toolchain\.minisign\.install",
            ):
                DEV.require_minisign({"PATH": str(self.root)})

    def test_key_generation_signing_verification_and_tampering(self) -> None:
        signer = self.signer()
        secrets = self.root / "secrets"
        secrets.mkdir(mode=0o700)
        public, secret, key_id = DEV.generate_key(signer, secrets, os.environ.copy())
        self.assertEqual(key_id, "6B62D3410E007120")
        self.assertEqual(stat.S_IMODE(secret.stat().st_mode), 0o600)
        document = self.root / "manifest.json"
        document.write_text('{"schema":2}\n', encoding="utf-8")
        signature = DEV.sign_and_verify(
            signer,
            document,
            secret,
            public,
            os.environ.copy(),
        )
        document.write_text('{"schema":3}\n', encoding="utf-8")
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "verification"):
            DEV.run_process(
                [signer, "-Vm", document, "-x", signature, "-p", public],
                cwd=self.root,
                environment=os.environ.copy(),
                capture=True,
                label="tamper verification",
            )

    def test_key_generation_and_signing_fail_closed(self) -> None:
        failing = self.root / "failing"
        failing.write_text("#!/bin/sh\nexit 7\n", encoding="utf-8")
        failing.chmod(0o755)
        secrets = self.root / "secrets"
        secrets.mkdir()
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "key generation"):
            DEV.generate_key(failing, secrets, os.environ.copy())

        signer = self.signer()
        public, secret, _ = DEV.generate_key(signer, secrets, os.environ.copy())
        document = self.root / "manifest.json"
        document.write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "signing"):
            DEV.sign_and_verify(failing, document, secret, public, os.environ.copy())


class CandidateSafetyTests(unittest.TestCase):
    def test_temporary_directory_is_private_and_removed_on_failure(self) -> None:
        location = None
        with self.assertRaisesRegex(RuntimeError, "interrupted"):
            with DEV.temporary_run_directory() as run_directory:
                location = run_directory
                self.assertEqual(stat.S_IMODE(run_directory.stat().st_mode), 0o700)
                raise RuntimeError("interrupted")
        self.assertIsNotNone(location)
        self.assertFalse(location.exists())

    def test_secret_must_be_removed_before_listening(self) -> None:
        with DEV.temporary_run_directory() as run_directory:
            secret = run_directory / "minisign.key"
            secret.write_bytes(b"untrusted comment: minisign secret key\nTEST\n")
            with self.assertRaisesRegex(DEV.DeveloperFlasherError, "still exists"):
                DEV.assert_secret_removed(run_directory, secret)
            leaked = run_directory / "leaked.bin"
            leaked.write_bytes(secret.read_bytes())
            secret.unlink()
            with self.assertRaisesRegex(DEV.DeveloperFlasherError, "material remains"):
                DEV.assert_secret_removed(run_directory, secret)
            leaked.unlink()
            DEV.assert_secret_removed(run_directory, secret)

    def test_shared_server_remains_loopback_only(self) -> None:
        with DEV.temporary_run_directory() as run_directory:
            website = run_directory / "website"
            website.mkdir()
            (website / "index.html").write_text("local\n", encoding="utf-8")
            module = DEV.load_candidate_server()
            server = mock.Mock()
            with mock.patch.object(
                module.http.server,
                "ThreadingHTTPServer",
                return_value=server,
            ) as constructor:
                self.assertIs(module.create_server(website, 0), server)
            self.assertEqual(constructor.call_args.args[0], ("127.0.0.1", 0))


class WebsiteBuildTests(unittest.TestCase):
    def test_local_site_stages_the_nordic_dfu_browser_core(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "dioxus"
            output.mkdir()
            (output / "index.html").write_text("site\n", encoding="utf-8")
            website = root / "website"
            public_key = root / "minisign.pub"
            public_key.write_text("fixture key\n", encoding="utf-8")
            identity = DEV.SourceIdentity(
                head="0" * 40,
                digest="a" * 64,
                state="dirty",
                version=f"0.3.1-dev.dirty.{'a' * 64}",
            )

            with (
                mock.patch.object(
                    DEV,
                    "require_node_tools",
                    return_value=(root / "tailwindcss", root / "esbuild"),
                ),
                mock.patch.object(DEV, "clear_dioxus_output", return_value=output),
                mock.patch.object(DEV, "clean_build_environment", return_value={}),
                mock.patch.object(DEV, "run_process") as run_process,
            ):
                DEV.build_website(
                    website,
                    identity,
                    DEV.Selection(("t1000-e",), 8765),
                    public_key,
                )

            command = run_process.call_args_list[-1].args[0]
            self.assertEqual(
                command,
                [
                    "bash",
                    DEV.ROOT / "tools" / "build" / "stage-web-flasher-nrf-dfu-wasm.sh",
                    website / "assets" / "flasher" / "nrf-dfu",
                ],
            )
            self.assertEqual(
                run_process.call_args_list[-1].kwargs["label"],
                "local developer Nordic DFU browser core build",
            )



class CandidateValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.candidate = Path(self.temporary.name)
        self.identity = DEV.SourceIdentity(
            head="0" * 40,
            digest="a" * 64,
            state="dirty",
            version=f"0.3.1-dev.dirty.{'a' * 64}",
        )
        self.key_id = "0123456789ABCDEF"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def artifact(self, relative: str, payload: bytes, **fields: object) -> dict:
        path = self.candidate / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
        return {
            "path": relative,
            "size": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
            **fields,
        }

    def esp_target(self, board: str = "heltec-v4") -> dict:
        payload = (
            f"version={self.identity.version} source={self.identity.digest}"
        ).encode("ascii")
        part = self.artifact(f"firmware/{board}/application.bin", payload, kind="application")
        return {
            "board_slug": board,
            "transport": "esp-serial",
            "parts": [part],
            "variants": [],
        }

    def uf2_payload(self, application_base: int = 0x26000) -> bytes:
        block = bytearray(512)
        words = {
            0: 0x0A324655,
            4: 0x9E5D5157,
            8: 0x00002000,
            12: application_base,
            16: 256,
            20: 0,
            24: 1,
            28: 0xADA52840,
            508: 0x0AB16F30,
        }
        for offset, value in words.items():
            block[offset : offset + 4] = value.to_bytes(4, "little")
        return bytes(block)

    def uf2_target(self) -> dict:
        payload = self.uf2_payload()
        variant = self.artifact(
            "firmware/t-echo/t-echo-s140-6.1.1.uf2",
            payload,
            softdevice_family="s140",
            softdevice_version="6.1.1",
            fwid="0x00b6",
            application_base="0x00026000",
            family_id="0xada52840",
        )
        return {
            "board_slug": "t-echo",
            "transport": "uf2-mass-storage",
            "parts": [],
            "variants": [variant],
        }

    def recovery_uf2_payload(self, application: bytes) -> bytes:
        blocks = []
        block_count = (len(application) + 255) // 256
        for index in range(block_count):
            block = bytearray(512)
            words = {
                0: 0x0A324655,
                4: 0x9E5D5157,
                8: 0x00002000,
                12: 0x27000 + index * 256,
                16: 256,
                20: index,
                24: block_count,
                28: 0xADA52840,
                508: 0x0AB16F30,
            }
            for offset, value in words.items():
                block[offset : offset + 4] = value.to_bytes(4, "little")
            start = index * 256
            block[32 : 32 + min(256, len(application) - start)] = application[
                start : start + 256
            ]
            blocks.append(bytes(block))
        return b"".join(blocks)

    def nrf_serial_dfu_target(self) -> dict:
        application_payload = bytes(index % 251 for index in range(300))
        application = self.artifact(
            "firmware/t1000-e/t1000e.bin",
            application_payload,
            kind="dfu-application",
        )
        init_packet = self.artifact(
            "firmware/t1000-e/t1000e.dat",
            b"init packet",
            kind="dfu-init-packet",
        )
        recovery = self.artifact(
            "firmware/t1000-e/t1000e.uf2",
            self.recovery_uf2_payload(application_payload),
            kind="uf2",
        )
        return {
            "board_slug": "t1000-e",
            "transport": "nrf-serial-dfu",
            "parts": [],
            "variants": [],
            "nrf_serial_dfu": {
                "compatibility": {
                    "softdevice_family": "s140",
                    "softdevice_version": "7.3.0",
                    "fwid": "0x0123",
                    "application_base": "0x00027000",
                    "application_end_exclusive": "0x000ea000",
                },
                "application": application,
                "init_packet": init_packet,
                "recovery": {
                    "mount_label": "T1000-E",
                    "board_id_prefix": "nrf52840-t1000-e-v1",
                    "family_id": "0xada52840",
                    "artifact": recovery,
                },
            },
        }

    def write_manifest(self, targets: list[dict], schema: int = 3) -> Path:
        path = self.candidate / "flash-manifest.json"
        path.write_text(
            json.dumps(
                {
                    "schema": schema,
                    "release": {
                        "version": self.identity.version,
                        "channel": "preview",
                        "commit": self.identity.head,
                    },
                    "signing": {"key_id": self.key_id},
                    "targets": targets,
                }
            ),
            encoding="utf-8",
        )
        return path

    def validate(self, targets: list[dict], boards: tuple[str, ...]) -> object:
        return DEV.verify_manifest_artifacts(
            self.candidate,
            self.write_manifest(targets),
            self.identity,
            DEV.Selection(boards, 8765),
            self.key_id,
        )

    def test_schema_three_accepts_esp_parts_and_uf2_variants_in_selection_order(self) -> None:
        validated = self.validate(
            [self.esp_target(), self.uf2_target()],
            ("heltec-v4", "t-echo"),
        )
        self.assertEqual(
            tuple((target.board_slug, target.transport) for target in validated.targets),
            (("heltec-v4", "esp-serial"), ("t-echo", "uf2-mass-storage")),
        )
        self.assertEqual(tuple(len(target.artifacts) for target in validated.targets), (1, 1))

    def test_nordic_recovery_is_bound_to_the_exact_dfu_application(self) -> None:
        target = self.nrf_serial_dfu_target()
        validated = self.validate([target], ("t1000-e",))
        self.assertEqual(
            tuple(artifact.path.name for artifact in validated.targets[0].artifacts),
            ("t1000e.bin", "t1000e.dat", "t1000e.uf2"),
        )

        application = target["nrf_serial_dfu"]["application"]
        application_path = self.candidate / application["path"]
        changed = bytearray(application_path.read_bytes())
        changed[17] ^= 0x80
        application_path.write_bytes(changed)
        application["sha256"] = hashlib.sha256(changed).hexdigest()
        with self.assertRaisesRegex(
            DEV.DeveloperFlasherError,
            "recovery UF2 block 0 disagrees with the exact DFU application",
        ):
            self.validate([target], ("t1000-e",))

    def test_schema_two_is_rejected_by_the_shared_contract(self) -> None:
        manifest = self.write_manifest([self.esp_target()], schema=2)
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "schema 3"):
            DEV.verify_manifest_artifacts(
                self.candidate,
                manifest,
                self.identity,
                DEV.Selection(("heltec-v4",), 8765),
                self.key_id,
            )

    def test_wrong_transport_and_artifact_shapes_are_rejected(self) -> None:
        cases = []
        wrong_transport = self.esp_target()
        wrong_transport["transport"] = "uf2-mass-storage"
        cases.append((wrong_transport, "transport disagrees"))
        wrong_shape = self.esp_target()
        wrong_shape["variants"] = [wrong_shape["parts"][0]]
        cases.append((wrong_shape, "disagree with its transport"))
        for target, message in cases:
            with self.subTest(message=message), self.assertRaisesRegex(
                DEV.DeveloperFlasherError, message
            ):
                self.validate([target], ("heltec-v4",))

    def test_duplicate_missing_linked_and_traversing_paths_are_rejected(self) -> None:
        target = self.esp_target()
        target["parts"].append(dict(target["parts"][0]))
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "repeats artifact path"):
            self.validate([target], ("heltec-v4",))

        for path, message in (
            ("firmware/missing.bin", "unavailable"),
            ("../outside.bin", "unsafe"),
        ):
            target = self.esp_target()
            target["parts"][0]["path"] = path
            with self.subTest(path=path), self.assertRaisesRegex(
                DEV.DeveloperFlasherError, message
            ):
                self.validate([target], ("heltec-v4",))

        target = self.esp_target()
        original = self.candidate / target["parts"][0]["path"]
        linked = original.with_name("linked.bin")
        linked.symlink_to(original)
        target["parts"][0]["path"] = linked.relative_to(self.candidate).as_posix()
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "contains a link"):
            self.validate([target], ("heltec-v4",))

    def test_size_and_hash_mismatches_are_rejected(self) -> None:
        for field, value in (("size", 1), ("sha256", "0" * 64)):
            target = self.esp_target()
            target["parts"][0][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(
                DEV.DeveloperFlasherError, "hash or size"
            ):
                self.validate([target], ("heltec-v4",))

    def test_malformed_uf2_evidence_is_rejected(self) -> None:
        target = self.uf2_target()
        artifact = self.candidate / target["variants"][0]["path"]
        artifact.write_bytes(b"not uf2")
        target["variants"][0]["size"] = len(b"not uf2")
        target["variants"][0]["sha256"] = hashlib.sha256(b"not uf2").hexdigest()
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "UF2 evidence is invalid"):
            self.validate([target], ("t-echo",))

    def test_esp_application_must_embed_signed_source_identity(self) -> None:
        target = self.esp_target()
        part = target["parts"][0]
        payload = self.identity.version.encode("ascii")
        artifact = self.candidate / part["path"]
        artifact.write_bytes(payload)
        part["size"] = len(payload)
        part["sha256"] = hashlib.sha256(payload).hexdigest()
        with self.assertRaisesRegex(DEV.DeveloperFlasherError, "does not embed"):
            self.validate([target], ("heltec-v4",))

    def test_release_signing_and_selection_identity_must_be_exact(self) -> None:
        target = self.esp_target()
        manifest = self.write_manifest([target])
        document = json.loads(manifest.read_text(encoding="utf-8"))
        cases = (
            (("release", "version"), "other", "release identity"),
            (("release", "commit"), "1" * 40, "release identity"),
            (("signing", "key_id"), "FEDCBA9876543210", "release identity"),
            (("targets", 0, "board_slug"), "t-echo", "exact selection"),
        )
        for coordinates, value, message in cases:
            changed = json.loads(json.dumps(document))
            owner = changed
            for coordinate in coordinates[:-1]:
                owner = owner[coordinate]
            owner[coordinates[-1]] = value
            manifest.write_text(json.dumps(changed), encoding="utf-8")
            with self.subTest(coordinates=coordinates), self.assertRaisesRegex(
                DEV.DeveloperFlasherError, message
            ):
                DEV.verify_manifest_artifacts(
                    self.candidate,
                    manifest,
                    self.identity,
                    DEV.Selection(("heltec-v4",), 8765),
                    self.key_id,
                )

    def test_staging_uses_only_the_immutable_validated_artifacts(self) -> None:
        target = self.esp_target()
        manifest = self.write_manifest([target])
        validated = DEV.verify_manifest_artifacts(
            self.candidate,
            manifest,
            self.identity,
            DEV.Selection(("heltec-v4",), 8765),
            self.key_id,
        )
        artifact = self.candidate / target["parts"][0]["path"]
        original = artifact.read_bytes()
        artifact.write_bytes(b"tampered after validation")
        extra = artifact.with_name("target.json")
        extra.write_text("{}\n", encoding="utf-8")
        website = self.candidate / "website"
        website.mkdir()
        manifest_signature = self.candidate / "flash-manifest.json.minisig"
        channel = self.candidate / "preview.json"
        channel_signature = self.candidate / "preview.json.minisig"
        public_key = self.candidate / "minisign.pub"
        for path in (manifest_signature, channel, channel_signature, public_key):
            path.write_text(f"{path.name}\n", encoding="utf-8")

        DEV.stage_signed_release(
            website,
            validated,
            manifest,
            manifest_signature,
            channel,
            channel_signature,
            public_key,
        )

        staged = website / "releases" / self.identity.version / target["parts"][0]["path"]
        self.assertEqual(staged.read_bytes(), original)
        self.assertFalse((staged.parent / extra.name).exists())


if __name__ == "__main__":
    unittest.main()
