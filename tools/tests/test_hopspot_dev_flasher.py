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
        self.assertEqual(DEV.parse_selection(["--all"]).boards, DEV.shipping_boards())

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

    def test_manifest_artifact_tampering_is_rejected(self) -> None:
        with DEV.temporary_run_directory() as candidate:
            identity = DEV.SourceIdentity(
                head="0" * 40,
                digest="a" * 64,
                state="dirty",
                version=f"0.3.1-dev.dirty.{'a' * 64}",
            )
            selection = DEV.Selection(("t-echo",), 8765)
            artifact = (
                candidate
                / "firmware"
                / "hopspot"
                / "t-echo"
                / identity.version
                / "firmware.uf2"
            )
            artifact.parent.mkdir(parents=True)
            artifact.write_bytes(b"firmware")
            manifest = candidate / "flash-manifest.json"
            manifest.write_text(
                json.dumps(
                    {
                        "schema": 2,
                        "release": {
                            "version": identity.version,
                            "channel": "preview",
                            "commit": identity.head,
                        },
                        "signing": {"key_id": "0123456789ABCDEF"},
                        "targets": [
                            {
                                "board_slug": "t-echo",
                                "parts": [
                                    {
                                        "path": artifact.relative_to(candidate).as_posix(),
                                        "size": len(b"firmware"),
                                        "sha256": "0" * 64,
                                    }
                                ],
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(DEV.DeveloperFlasherError, "hash or size"):
                DEV.verify_manifest_artifacts(
                    candidate,
                    manifest,
                    identity,
                    selection,
                    "0123456789ABCDEF",
                )

    def test_esp_application_must_embed_signed_source_identity(self) -> None:
        with DEV.temporary_run_directory() as candidate:
            identity = DEV.SourceIdentity(
                head="0" * 40,
                digest="a" * 64,
                state="dirty",
                version=f"0.3.1-dev.dirty.{'a' * 64}",
            )
            selection = DEV.Selection(("heltec-v4",), 8765)
            artifact = (
                candidate
                / "firmware"
                / "hopspot"
                / "heltec-v4"
                / identity.version
                / "application.bin"
            )
            artifact.parent.mkdir(parents=True)
            artifact.write_bytes(identity.version.encode("ascii"))
            payload = artifact.read_bytes()
            manifest = candidate / "flash-manifest.json"
            manifest.write_text(
                json.dumps(
                    {
                        "schema": 2,
                        "release": {
                            "version": identity.version,
                            "channel": "preview",
                            "commit": identity.head,
                        },
                        "signing": {"key_id": "0123456789ABCDEF"},
                        "targets": [
                            {
                                "board_slug": "heltec-v4",
                                "parts": [
                                    {
                                        "kind": "application",
                                        "path": artifact.relative_to(candidate).as_posix(),
                                        "size": len(payload),
                                        "sha256": hashlib.sha256(payload).hexdigest(),
                                    }
                                ],
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(DEV.DeveloperFlasherError, "does not embed"):
                DEV.verify_manifest_artifacts(
                    candidate,
                    manifest,
                    identity,
                    selection,
                    "0123456789ABCDEF",
                )

    def test_signed_release_stages_only_manifest_artifacts(self) -> None:
        with DEV.temporary_run_directory() as run_directory:
            candidate = run_directory / "candidate"
            website = candidate / "website"
            candidate.mkdir()
            website.mkdir()
            identity = DEV.SourceIdentity(
                head="0" * 40,
                digest="a" * 64,
                state="dirty",
                version=f"0.3.1-dev.dirty.{'a' * 64}",
            )
            artifact = (
                candidate
                / "firmware"
                / "hopspot"
                / "t-echo"
                / identity.version
                / "firmware.uf2"
            )
            artifact.parent.mkdir(parents=True)
            artifact.write_bytes(b"firmware")
            (artifact.parent / "target.json").write_text("{}\n", encoding="utf-8")
            (artifact.parent / "source-capability.json").write_text(
                "{}\n",
                encoding="utf-8",
            )
            manifest = candidate / "flash-manifest.json"
            manifest.write_text(
                json.dumps(
                    {
                        "targets": [
                            {
                                "parts": [
                                    {
                                        "path": artifact.relative_to(candidate).as_posix(),
                                    }
                                ]
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            manifest_signature = candidate / "flash-manifest.json.minisig"
            manifest_signature.write_text("signature\n", encoding="utf-8")
            channel = candidate / "preview.json"
            channel.write_text("{}\n", encoding="utf-8")
            channel_signature = candidate / "preview.json.minisig"
            channel_signature.write_text("signature\n", encoding="utf-8")
            public_key = candidate / "minisign.pub"
            public_key.write_text("public key\n", encoding="utf-8")

            DEV.stage_signed_release(
                candidate,
                website,
                identity,
                manifest,
                manifest_signature,
                channel,
                channel_signature,
                public_key,
            )

            staged = (
                website
                / "releases"
                / identity.version
                / artifact.relative_to(candidate)
            )
            self.assertEqual(staged.read_bytes(), b"firmware")
            self.assertFalse((staged.parent / "target.json").exists())
            self.assertFalse((staged.parent / "source-capability.json").exists())


if __name__ == "__main__":
    unittest.main()
