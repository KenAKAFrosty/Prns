from pathlib import Path

from validation.interop.harness import (
    FailureKind,
    InteropCase,
    InteropFailure,
    cargo_binary,
    case_main,
    reference_python,
    require_evidence,
    require_hex_output,
    require_output_marker,
    run_checked,
    run_checked_bytes,
    run_expect_status,
)


ROOT = Path(__file__).resolve().parents[3]
PRNSD_MANIFEST = ROOT / "prnsd/Cargo.toml"
STOCK_ORACLE = ROOT / "validation/interop/peers/rns_rnid_oracle.py"
EXPECTED_PUBLIC_KEY = (
    "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20d"
    "04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"
)
EXPECTED_DESTINATION_HASH = "f6f4f4a9b02bf85109dbf53265984547"
ENCODINGS = (
    ("base32", "-B"),
    ("base64", "-b"),
    ("base256", "-U"),
    ("hex", "-F"),
)
SUCCESS = "PASS: Prnsd id matches stock RNS identity, RSG/RSM, encryption, and encoding behavior"


def run() -> None:
    python = reference_python("RPC_SMOKE_PYTHON")
    prnsd = cargo_binary(PRNSD_MANIFEST, "prnsd")
    with InteropCase() as case:
        private_identity = case.work / "oracle.rid"
        public_identity = case.work / "oracle.pub"
        stock_message = case.work / "stock-message"
        stock_signature = case.work / "stock-message.rsg"
        candidate_message = case.work / "candidate-message"
        plaintext = case.work / "plaintext"
        stock_encrypted = case.work / "stock.rfe"
        artifact_message = case.work / "stock-artifact-message"
        stock_rsg = case.work / "stock-artifact-message.rsg"
        candidate_artifact_message = case.work / "prns-artifact-message"
        stock_rsm = case.work / "stock.rsm"
        candidate_rsm = case.work / "prns.rsm"
        metadata = case.work / "metadata"
        metadata_spec = case.work / "metadata.spec"

        identity_hash = require_hex_output(
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "prepare",
                    str(private_identity),
                    str(public_identity),
                    str(stock_message),
                    str(stock_signature),
                    str(plaintext),
                    str(stock_encrypted),
                ),
                "stock RNS did not prepare local identity fixtures",
            ),
            16,
            "stock RNS did not report a valid identity hash",
        )
        run_checked(
            (
                str(python),
                str(STOCK_ORACLE),
                "prepare-artifacts",
                str(private_identity),
                str(artifact_message),
                str(stock_rsg),
                str(stock_rsm),
            ),
            "stock RNS did not prepare canonical signature fixtures",
        )
        candidate_artifact_message.write_bytes(artifact_message.read_bytes())
        candidate_message.write_bytes(stock_message.read_bytes())

        printed = run_checked(
            (str(prnsd), "id", "-i", str(private_identity), "-p", "-P"),
            "Prnsd did not print the stock RNS identity",
        )
        require_output_marker(
            printed,
            f"Identity Hash : <{identity_hash}>",
            "Prnsd printed the wrong identity hash",
        )
        require_output_marker(
            printed,
            f"Public Key    : {EXPECTED_PUBLIC_KEY}",
            "Prnsd printed the wrong public key",
        )

        hashed = run_checked(
            (str(prnsd), "id", "-i", str(private_identity), "-H", "example.aspect"),
            "Prnsd did not derive a destination hash",
        )
        require_output_marker(
            hashed,
            f"<{EXPECTED_DESTINATION_HASH}>",
            "Prnsd derived the wrong destination hash",
        )
        require_output_marker(
            hashed,
            f"<example.aspect.{identity_hash}:{EXPECTED_DESTINATION_HASH}>",
            "Prnsd rendered the wrong full destination specifier",
        )

        run_checked(
            (str(prnsd), "id", "-i", str(private_identity), "-w", str(case.work / "exported")),
            "Prnsd did not export the public identity",
        )
        require_evidence(
            public_identity.read_bytes() == (case.work / "exported.pub").read_bytes(),
            "Prnsd exported different public identity bytes",
        )

        run_checked(
            (str(prnsd), "id", "-i", str(private_identity), "-s", str(candidate_message), "--raw"),
            "Prnsd did not create a raw signature",
        )
        run_checked(
            (
                str(python),
                str(STOCK_ORACLE),
                "verify",
                str(private_identity),
                str(candidate_message),
                str(candidate_message) + ".rsg",
            ),
            "stock RNS rejected the raw Prns signature",
        )
        run_checked(
            (str(prnsd), "id", "-m", str(public_identity), "-V", str(stock_message)),
            "Prnsd rejected the stock RNS raw signature",
        )

        run_checked(
            (
                str(prnsd),
                "id",
                "-i",
                str(private_identity),
                "-s",
                str(candidate_artifact_message),
            ),
            "Prnsd did not create a canonical RSG",
        )
        run_checked(
            (
                str(python),
                str(STOCK_ORACLE),
                "verify-rsg",
                str(private_identity),
                str(candidate_artifact_message),
                str(candidate_artifact_message) + ".rsg",
            ),
            "stock RNS rejected the canonical Prns RSG",
        )
        run_checked(
            (str(prnsd), "id", "-V", str(artifact_message)),
            "Prnsd rejected the canonical stock RNS RSG",
        )
        for encoding, flag in ENCODINGS:
            encoded_rsg = case.work / f"artifact-{encoding}.out"
            encoded_rsg.write_text(
                run_checked(
                    (
                        str(prnsd),
                        "id",
                        "-i",
                        str(private_identity),
                        "-s",
                        str(candidate_artifact_message),
                        flag,
                    ),
                    f"Prnsd did not render a {encoding} RSG",
                ),
                encoding="utf-8",
            )
            run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "verify-encoded-rsg",
                    str(private_identity),
                    str(candidate_artifact_message),
                    str(encoded_rsg),
                    encoding,
                ),
                f"stock RNS rejected the {encoding} Prns RSG",
            )

        metadata.write_text(
            "name = Prns\nversion = 3\ntags = one, two\nstable = yes\n",
            encoding="utf-8",
        )
        metadata_spec.write_text(
            "name = string()\nversion = integer()\ntags = string_list()\nstable = boolean()\n",
            encoding="utf-8",
        )
        run_checked(
            (
                str(prnsd),
                "id",
                "-i",
                str(private_identity),
                "-S",
                "canonical-message-oracle",
                "-E",
                str(metadata),
                "--meta-spec",
                str(metadata_spec),
                "-w",
                str(case.work / "prns"),
            ),
            "Prnsd did not create a canonical RSM",
        )
        run_checked(
            (str(python), str(STOCK_ORACLE), "verify-rsm", str(private_identity), str(candidate_rsm)),
            "stock RNS rejected the canonical Prns RSM",
        )
        stock_rsm_result = run_checked(
            (str(prnsd), "id", "-V", str(stock_rsm), "--meta"),
            "Prnsd rejected the canonical stock RNS RSM",
        )
        require_output_marker(
            stock_rsm_result,
            "canonical-message-oracle",
            "Prnsd did not emit the stock RNS embedded message",
        )

        run_checked(
            (str(prnsd), "id", "-m", str(public_identity), "-e", str(plaintext)),
            "Prnsd did not encrypt the stock plaintext",
        )
        run_checked(
            (
                str(python),
                str(STOCK_ORACLE),
                "decrypt",
                str(private_identity),
                str(plaintext) + ".rfe",
                str(plaintext),
            ),
            "stock RNS could not decrypt the Prns ciphertext",
        )
        opened = case.work / "opened"
        run_checked(
            (
                str(prnsd),
                "id",
                "-i",
                str(private_identity),
                "-d",
                str(stock_encrypted),
                "-w",
                str(opened),
            ),
            "Prnsd could not decrypt the stock RNS ciphertext",
        )
        require_evidence(
            plaintext.read_bytes() == opened.read_bytes(),
            "Prnsd decrypted different plaintext",
        )

        no_clobber = case.work / "no-clobber"
        no_clobber.write_text("existing", encoding="utf-8")
        run_expect_status(
            (
                str(prnsd),
                "id",
                "-i",
                str(private_identity),
                "-d",
                str(stock_encrypted),
                "-w",
                str(no_clobber),
            ),
            11,
            "Prnsd did not preserve the no-clobber exit status",
        )
        if no_clobber.read_text(encoding="utf-8") != "existing":
            raise InteropFailure(
                FailureKind.EVIDENCE_UNEXPECTED,
                "Prnsd changed the no-clobber destination",
            )

        corrupt = case.work / "corrupt.rfe"
        corrupt_bytes = bytearray(stock_encrypted.read_bytes())
        corrupt_bytes[64] ^= 1
        corrupt.write_bytes(corrupt_bytes)
        partial = case.work / "partial"
        run_expect_status(
            (
                str(prnsd),
                "id",
                "-i",
                str(private_identity),
                "-d",
                str(corrupt),
                "-w",
                str(partial),
            ),
            12,
            "Prnsd did not preserve the corrupt-ciphertext exit status",
        )
        if partial.exists():
            raise InteropFailure(
                FailureKind.EVIDENCE_UNEXPECTED,
                "Prnsd published partial plaintext from corrupt ciphertext",
            )

        for encoding, flag in ENCODINGS:
            expected = run_checked(
                (
                    str(python),
                    str(STOCK_ORACLE),
                    "encoding",
                    str(public_identity),
                    encoding,
                ),
                f"stock RNS did not render the {encoding} public identity",
            ).strip()
            actual = run_checked(
                (str(prnsd), "id", "-m", str(public_identity), "-x", flag),
                f"Prnsd did not render the {encoding} public identity",
            )
            require_output_marker(
                actual,
                expected,
                f"Prnsd {encoding} identity output differs from stock RNS",
            )

        stdout_ciphertext = case.work / "stdout.rfe"
        stdout_ciphertext.write_bytes(
            run_checked_bytes(
                (
                    str(prnsd),
                    "id",
                    "-i",
                    str(private_identity),
                    "-e",
                    str(plaintext),
                    "-O",
                ),
                "Prnsd did not write ciphertext to standard output",
            )
        )
        run_checked(
            (
                str(python),
                str(STOCK_ORACLE),
                "decrypt",
                str(private_identity),
                str(stdout_ciphertext),
                str(plaintext),
            ),
            "stock RNS could not decrypt Prns standard output",
        )
        stdin_signature = case.work / "stdin.rsg"
        stdin_signature.write_bytes(
            run_checked_bytes(
                (
                    str(prnsd),
                    "id",
                    "-M",
                    private_identity.read_bytes().hex(),
                    "-s",
                    "--raw",
                    "-I",
                    "-O",
                ),
                "Prnsd did not sign standard input to standard output",
                standard_input=candidate_message.read_bytes(),
            )
        )
        run_checked(
            (
                str(python),
                str(STOCK_ORACLE),
                "verify",
                str(private_identity),
                str(candidate_message),
                str(stdin_signature),
            ),
            "stock RNS rejected the Prns standard-input signature",
        )

        generated_identity = case.work / "generated.rid"
        run_checked(
            (str(prnsd), "id", "-g", str(generated_identity)),
            "Prnsd did not generate an identity",
        )
        require_evidence(
            len(generated_identity.read_bytes()) == 64,
            "Prnsd generated an identity with the wrong length",
        )


if __name__ == "__main__":
    raise SystemExit(case_main(run, SUCCESS))
