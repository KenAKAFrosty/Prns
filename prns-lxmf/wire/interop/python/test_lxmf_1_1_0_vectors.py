"""Pinned Python LXMF 1.1.0 direct-link corpus checks."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import unittest

import generate_lxmf_1_1_0_vectors as vectors


class LxmfVectorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.generated = vectors.build_vectors()
        cls.messages = {
            case["name"]: case for case in cls.generated["messages"]
        }
        cls.inbound_oracles = {
            case["name"]: case for case in cls.generated["inbound_oracles"]
        }

    def test_committed_corpus_matches_python_authority(self) -> None:
        committed = json.loads(vectors.DEFAULT_OUTPUT.read_text(encoding="utf-8"))
        self.assertEqual(committed, self.generated)

    def test_authority_versions_and_source_digests_are_pinned(self) -> None:
        authority = self.generated["authority"]
        self.assertEqual(
            self.generated["generator_source_sha256"],
            hashlib.sha256(Path(vectors.__file__).read_bytes()).hexdigest(),
        )
        self.assertEqual(
            self.generated["requirements_sha256"],
            hashlib.sha256(vectors.REQUIREMENTS.read_bytes()).hexdigest(),
        )
        self.assertEqual(authority["version"], "1.1.0")
        self.assertEqual(authority["revision"], vectors.LXMF_REVISION)
        self.assertEqual(
            authority["vcs"]["vcs_info"]["commit_id"],
            vectors.LXMF_REVISION,
        )
        self.assertEqual(authority["lxmf_core_sha256"], vectors.LXMF_CORE_SHA256)
        self.assertEqual(
            authority["lxmf_message_sha256"], vectors.LXMF_MESSAGE_SHA256
        )
        self.assertEqual(authority["reticulum"]["version"], "1.5.2")
        self.assertEqual(
            authority["reticulum"]["vcs"]["vcs_info"]["commit_id"],
            vectors.RNS_REVISION,
        )
        self.assertEqual(
            authority["reticulum"]["source_sha256"],
            vectors.RNS_SOURCE_SHA256,
        )
        self.assertEqual(
            authority["messagepack"]["source_sha256"],
            vectors.UMSGPACK_SHA256,
        )

    def test_announce_vectors_are_generated_by_pinned_python_helpers(self) -> None:
        announces = {
            case["name"]: case for case in self.generated["announce_vectors"]
        }
        self.assertEqual(
            set(announces),
            {
                "personal_hopspot_e290_legacy",
                "browser_playground_legacy",
                "python_lxmf_current",
                "prns_direct_current",
                "python_display_normalization",
            },
        )
        self.assertEqual(
            announces["python_display_normalization"]["python_display_name"],
            "Python  peer",
        )
        self.assertFalse(
            announces["prns_direct_current"]["python_compression_support"]
        )
        self.assertTrue(
            announces["python_lxmf_current"]["python_compression_support"]
        )
        for case in announces.values():
            raw = bytes.fromhex(case["raw_hex"])
            self.assertEqual(
                vectors.LXMF.display_name_from_app_data(raw),
                case["python_display_name"],
            )
            self.assertEqual(
                vectors.LXMF.stamp_cost_from_app_data(raw),
                case["python_stamp_cost"],
            )

    def test_basic_message_matches_precursor_python_known_answer(self) -> None:
        case = self.messages["basic_binary"]
        self.assertTrue(case["precursor_python_known_answer_input"])
        self.assertEqual(
            case["message_id_hex"],
            "c00af1f9ba72e66d4b9a41fbe76a55d6bbb1c8dfb9271f0cf660ed101e174c96",
        )
        self.assertEqual(
            case["full_wire_hex"],
            "021e68345db8a80c29d0c2f193baa5f4"
            "20f7e44b55b06cff39719106f2bd1fd2"
            "cfeaf89e57248baad43791a115345482f6b54b6e90aa0d02b5d8eddad1dc6a6a"
            "323ec74921c618ae95e69153e9645db6f223d5d387db37ae23f58ef1f0560700"
            "94cb41d954fc40000000c4094772656574696e6773c41648656c6c6f2066726f"
            "6d20507974686f6e204c584d4680",
        )

    def test_ids_and_signatures_cover_exact_payload4_bytes(self) -> None:
        source_identity, _, _, _ = vectors._ensure_rns()
        for case in self.generated["messages"]:
            with self.subTest(name=case["name"]):
                destination = bytes.fromhex(case["destination_hash_hex"])
                source = bytes.fromhex(case["source_hash_hex"])
                payload4 = bytes.fromhex(case["payload4_hex"])
                hashed_part = destination + source + payload4
                message_id = hashlib.sha256(hashed_part).digest()
                self.assertEqual(message_id.hex(), case["message_id_hex"])
                self.assertTrue(
                    source_identity.validate(
                        bytes.fromhex(case["signature_hex"]),
                        hashed_part + message_id,
                    )
                )

    def test_noncanonical_exact_four_inbound_hashes_received_bytes(self) -> None:
        case = self.inbound_oracles["exact_four_array16_raw_hash"]
        received_payload = bytes.fromhex(case["received_payload_hex"])
        canonical_repack = vectors.umsgpack.packb(
            vectors.umsgpack.unpackb(received_payload)
        )
        self.assertEqual(canonical_repack.hex(), case["canonical_repack_hex"])
        self.assertNotEqual(canonical_repack, received_payload)

        destination = bytes.fromhex(case["destination_hash_hex"])
        source = bytes.fromhex(case["source_hash_hex"])
        hashed_part = destination + source + received_payload
        message_id = hashlib.sha256(hashed_part).digest()
        self.assertEqual(message_id.hex(), case["message_id_hex"])
        source_identity, _, _, _ = vectors._ensure_rns()
        self.assertTrue(
            source_identity.validate(
                bytes.fromhex(case["signature_hex"]),
                hashed_part + message_id,
            )
        )
        self.assertEqual(
            vectors.parse_outcome(bytes.fromhex(case["full_wire_hex"])),
            case["python_parse"],
        )

    def test_rich_fields_preserve_binary_and_nested_messagepack_types(self) -> None:
        case = self.messages["rich_fields"]
        self.assertEqual(case["decoded"]["title_hex"], "ff007469746c65")
        self.assertEqual(case["decoded"]["content_hex"], "800072696368")
        fields = vectors.umsgpack.unpackb(bytes.fromhex(case["fields_msgpack_hex"]))
        self.assertEqual(
            fields[1],
            [None, True, False, -33, 127, 128, 65_536, 1.5, b"\x00\xff", "utf8"],
        )
        self.assertEqual(fields[9][b"bin-key"], [1, 2])
        self.assertEqual(fields[9]["string-key"]["nested"], b"\xfe")
        self.assertEqual(fields[0x7F], {-1: None})
        self.assertEqual(fields["vendor.extension"], {"opaque": b"\x00\x01"})
        self.assertEqual(fields[b"\x00vendor"], [False, {"deep": -1}])
        self.assertEqual(case["decoded"]["fields"]["type"], "map")

    def test_pow_and_ticket_stamps_have_reference_lengths_and_validate(self) -> None:
        pow_case = self.messages["pow_stamp_32"]
        ticket_case = self.messages["ticket_stamp_16"]
        self.assertEqual(pow_case["stamp"]["length"], 32)
        self.assertEqual(pow_case["stamp"]["target_cost"], vectors.POW_COST)
        self.assertTrue(pow_case["stamp"]["valid"])
        self.assertGreaterEqual(pow_case["stamp"]["value"], vectors.POW_COST)
        self.assertEqual(ticket_case["stamp"]["length"], 16)
        self.assertEqual(ticket_case["stamp"]["ticket_hex"], vectors.TICKET.hex())
        self.assertTrue(ticket_case["stamp"]["valid"])

        fields = vectors.umsgpack.unpackb(
            bytes.fromhex(ticket_case["fields_msgpack_hex"])
        )
        expiry, ticket = fields[vectors.LXMF.FIELD_TICKET]
        self.assertEqual(expiry, vectors.TICKET_EXPIRY)
        self.assertEqual(ticket, vectors.TICKET)
        expected_stamp = vectors.RNS.Identity.truncated_hash(
            ticket + bytes.fromhex(ticket_case["message_id_hex"])
        )
        self.assertEqual(expected_stamp.hex(), ticket_case["stamp"]["hex"])

    def test_python_method_and_representation_thresholds_are_exact(self) -> None:
        expected = {
            "empty_binary": (-1, "opportunistic", "packet"),
            "one_byte_content": (0, "opportunistic", "packet"),
            "opportunistic_limit_295": (295, "opportunistic", "packet"),
            "opportunistic_over_296": (296, "direct", "packet"),
            "direct_limit_319": (319, "direct", "packet"),
            "direct_over_320": (320, "direct", "resource"),
        }
        for name, (size, method, representation) in expected.items():
            with self.subTest(name=name):
                case = self.messages[name]
                self.assertEqual(case["selection_content_size"], size)
                self.assertEqual(case["actual_method"], method)
                self.assertEqual(case["representation"], representation)

    def test_every_ingress_form_normalizes_to_full_lxmf(self) -> None:
        carriers = set()
        for case in self.generated["messages"]:
            with self.subTest(name=case["name"]):
                ingress = case["ingress"]
                carriers.add(ingress["carrier_event"])
                payload = bytes.fromhex(ingress["payload_hex"])
                if ingress["normalization"] == "prepend_implied_destination_hash":
                    normalized = bytes.fromhex(
                        ingress["implied_destination_hash_hex"]
                    ) + payload
                else:
                    self.assertEqual(ingress["normalization"], "identity")
                    normalized = payload
                self.assertEqual(normalized.hex(), case["full_wire_hex"])
        self.assertEqual(
            carriers, {"destination_data", "link_data", "resource_complete"}
        )

    def test_negative_mutations_match_python_parse_and_stamp_outcomes(self) -> None:
        cases = {
            case["name"]: case for case in self.generated["negative_mutations"]
        }
        expected_parse = {
            "signature_bit_flip": ("message", False, "signature_invalid"),
            "content_bit_flip": ("message", False, "signature_invalid"),
            "source_hash_bit_flip": ("message", False, "source_unknown"),
            "truncated_payload": ("exception", None, None),
            "pow_stamp_bit_flip": ("message", True, None),
            "ticket_stamp_bit_flip": ("message", True, None),
        }
        for name, (result, validated, reason) in expected_parse.items():
            with self.subTest(name=name):
                case = cases[name]
                observed = vectors.parse_outcome(bytes.fromhex(case["full_wire_hex"]))
                self.assertEqual(observed, case["python_parse"])
                self.assertEqual(observed["result"], result)
                if result == "message":
                    self.assertEqual(observed["signature_validated"], validated)
                    self.assertEqual(observed["unverified_reason"], reason)
        self.assertFalse(cases["pow_stamp_bit_flip"]["stamp_validation"]["valid"])
        self.assertFalse(
            cases["ticket_stamp_bit_flip"]["stamp_validation"]["valid"]
        )


if __name__ == "__main__":
    unittest.main()
