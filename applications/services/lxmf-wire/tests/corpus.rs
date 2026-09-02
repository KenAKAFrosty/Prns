#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use curve25519_dalek::constants::{ED25519_BASEPOINT_POINT, EIGHT_TORSION};
use prns_lxmf_wire::{
    encode_current_lxmf_announce, normalize_lxmf_display_name, parse_lxmf_announce,
    select_delivery, validate_messagepack_value, validate_pow_stamp, validate_ticket_stamp,
    AnnounceFormat, Canonicality, CarrierIngress, CarrierKind, DeliveryMethod, DesiredDelivery,
    LimitKind, MessagePackKind, MessageView, PowBudget, Representation, RequiredStampCost,
    SignatureError, StampAdmission, StampError, StampKind, StampPolicy, StampPolicyError,
    WireError, WireLimits, ABSOLUTE_MAX_NESTING_DEPTH, ENCRYPTED_PACKET_MAX_CONTENT,
    LINK_PACKET_MAX_CONTENT,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const CORPUS_JSON: &str = include_str!("../interop/vectors/lxmf-1.1.0-v1.json");
const GENERATOR: &[u8] = include_bytes!("../interop/python/generate_lxmf_1_1_0_vectors.py");
const REQUIREMENTS: &[u8] = include_bytes!("../interop/python/requirements-lxmf-1.1.0.txt");

const POSITIVE_NAMES: [&str; 10] = [
    "basic_binary",
    "direct_limit_319",
    "direct_over_320",
    "empty_binary",
    "one_byte_content",
    "opportunistic_limit_295",
    "opportunistic_over_296",
    "pow_stamp_32",
    "rich_fields",
    "ticket_stamp_16",
];
const NEGATIVE_NAMES: [&str; 6] = [
    "content_bit_flip",
    "pow_stamp_bit_flip",
    "signature_bit_flip",
    "source_hash_bit_flip",
    "ticket_stamp_bit_flip",
    "truncated_payload",
];

fn limits() -> WireLimits {
    WireLimits::new(4_096, 2_048, 256, 2_048, 65_536, 16)
}

#[derive(Deserialize)]
struct Corpus {
    generator_source_sha256: String,
    requirements_sha256: String,
    delivery_constants: DeliveryConstants,
    messages: Vec<MessageFixture>,
    negative_mutations: Vec<NegativeFixture>,
    inbound_oracles: Vec<InboundOracle>,
    announce_vectors: Vec<AnnounceFixture>,
}

#[derive(Deserialize)]
struct DeliveryConstants {
    encrypted_packet_max_content: usize,
    link_packet_max_content: usize,
    pow_stamp_length: usize,
    ticket_length: usize,
}

#[derive(Deserialize)]
struct MessageFixture {
    name: String,
    desired_method: String,
    actual_method: String,
    representation: String,
    selection_content_size: i64,
    destination_hash_hex: String,
    source_hash_hex: String,
    source_public_key_hex: String,
    signature_hex: String,
    payload4_hex: String,
    wire_payload_hex: String,
    fields_msgpack_hex: String,
    full_wire_hex: String,
    message_id_hex: String,
    decoded: DecodedFixture,
    stamp: Option<StampFixture>,
    ingress: IngressFixture,
}

#[derive(Deserialize)]
struct DecodedFixture {
    timestamp_f64_bits_hex: String,
    title_hex: String,
    content_hex: String,
}

#[derive(Debug, Deserialize)]
struct StampFixture {
    kind: String,
    hex: String,
    target_cost: Option<u16>,
    ticket_hex: Option<String>,
    value: u16,
}

#[derive(Deserialize)]
struct IngressFixture {
    carrier_event: String,
    payload_hex: String,
    implied_destination_hash_hex: Option<String>,
}

#[derive(Deserialize)]
struct NegativeFixture {
    name: String,
    based_on: String,
    full_wire_hex: String,
    python_parse: PythonParse,
    stamp_validation: Option<NegativeStamp>,
}

#[derive(Deserialize)]
struct PythonParse {
    result: String,
    message_id_hex: Option<String>,
    signature_validated: Option<bool>,
    unverified_reason: Option<String>,
    exception_type: Option<String>,
}

#[derive(Deserialize)]
struct NegativeStamp {
    kind: String,
    target_cost: Option<u16>,
    ticket_hex: Option<String>,
    valid: bool,
}

#[derive(Deserialize)]
struct InboundOracle {
    name: String,
    full_wire_hex: String,
    received_payload_hex: String,
    canonical_repack_hex: String,
    message_id_hex: String,
    source_public_key_hex: String,
}

#[derive(Deserialize)]
struct AnnounceFixture {
    name: String,
    format: String,
    raw_hex: String,
    display_name_hex: Option<String>,
    python_display_name: Option<String>,
    python_stamp_cost: Option<u64>,
    python_compression_support: bool,
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS_JSON).expect("checked-in LXMF corpus JSON")
}

fn decode(hex_value: &str) -> Vec<u8> {
    hex::decode(hex_value).expect("fixture hex")
}

fn array<const N: usize>(hex_value: &str) -> [u8; N] {
    decode(hex_value).try_into().expect("fixture fixed length")
}

fn fixture<'a>(corpus: &'a Corpus, name: &str) -> &'a MessageFixture {
    corpus
        .messages
        .iter()
        .find(|message| message.name == name)
        .expect("fixture name")
}

fn saturated_selection_content_size(fixture: &MessageFixture) -> usize {
    match fixture.selection_content_size {
        -1 => 0,
        value => usize::try_from(value).expect("nonnegative Python content size fits usize"),
    }
}

fn parse_bound<'a>(
    fixture: &MessageFixture,
    full_wire: &'a [u8],
    destination: &'a [u8; 16],
) -> Result<MessageView<'a>, WireError> {
    match (
        fixture.actual_method.as_str(),
        fixture.representation.as_str(),
    ) {
        ("opportunistic", "packet") => MessageView::parse_ingress(
            CarrierIngress::Opportunistic {
                implied_destination: destination,
                payload: &full_wire[16..],
            },
            limits(),
        ),
        ("direct", "packet") => MessageView::parse_ingress(
            CarrierIngress::LinkDataContextNone {
                expected_destination: destination,
                payload: full_wire,
            },
            limits(),
        ),
        ("direct", "resource") => MessageView::parse_ingress(
            CarrierIngress::ResourceComplete {
                expected_destination: destination,
                payload: full_wire,
            },
            limits(),
        ),
        lane => panic!("unexpected fixture lane {lane:?}"),
    }
}

#[test]
fn corpus_provenance_and_fixture_inventory_are_closed() {
    let corpus = corpus();
    assert_eq!(
        hex::encode(Sha256::digest(GENERATOR)),
        corpus.generator_source_sha256
    );
    assert_eq!(
        hex::encode(Sha256::digest(REQUIREMENTS)),
        corpus.requirements_sha256
    );
    assert_eq!(corpus.delivery_constants.encrypted_packet_max_content, 295);
    assert_eq!(corpus.delivery_constants.link_packet_max_content, 319);
    assert_eq!(corpus.delivery_constants.pow_stamp_length, 32);
    assert_eq!(corpus.delivery_constants.ticket_length, 16);

    let mut positives: Vec<&str> = corpus
        .messages
        .iter()
        .map(|message| message.name.as_str())
        .collect();
    positives.sort_unstable();
    assert_eq!(positives, POSITIVE_NAMES);
    let mut negatives: Vec<&str> = corpus
        .negative_mutations
        .iter()
        .map(|mutation| mutation.name.as_str())
        .collect();
    negatives.sort_unstable();
    assert_eq!(negatives, NEGATIVE_NAMES);
    assert_eq!(corpus.inbound_oracles.len(), 1);
    assert_eq!(
        corpus.inbound_oracles[0].name,
        "exact_four_array16_raw_hash"
    );
}

#[test]
fn shared_announce_vectors_match_existing_consumers_and_python_projection() {
    let corpus = corpus();
    let mut names: Vec<&str> = corpus
        .announce_vectors
        .iter()
        .map(|fixture| fixture.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "browser_playground_legacy",
            "personal_hopspot_e290_legacy",
            "prns_direct_current",
            "python_display_normalization",
            "python_lxmf_current",
        ]
    );
    for fixture in &corpus.announce_vectors {
        let raw = decode(&fixture.raw_hex);
        let parsed = parse_lxmf_announce(&raw, 255).expect("Python-compatible announce");
        let expected_format = match fixture.format.as_str() {
            "legacy_two_item" => AnnounceFormat::LegacyTwoItem,
            "current_three_item" => AnnounceFormat::CurrentThreeItem,
            other => panic!("unknown announce format {other}"),
        };
        assert_eq!(parsed.format, expected_format, "{}", fixture.name);
        assert_eq!(
            parsed.display_name.map(hex::encode),
            fixture.display_name_hex,
            "{}",
            fixture.name
        );
        assert_eq!(
            parsed.required_stamp_cost, fixture.python_stamp_cost,
            "{}",
            fixture.name
        );
        let mut normalized = [0u8; 255];
        let projected = parsed
            .display_name
            .map(|name| normalize_lxmf_display_name(name, &mut normalized).expect("normalization"));
        assert_eq!(
            projected,
            fixture.python_display_name.as_deref(),
            "{}",
            fixture.name
        );
        if fixture.name == "prns_direct_current" {
            assert!(!fixture.python_compression_support);
            let mut encoded = [0u8; 64];
            let length = encode_current_lxmf_announce(b"Prns live peer", &mut encoded)
                .expect("service announce");
            assert_eq!(&encoded[..length], raw);
        }
    }
    assert_eq!(
        decode(
            &corpus
                .announce_vectors
                .iter()
                .find(|fixture| fixture.name == "personal_hopspot_e290_legacy")
                .expect("Personal Hopspot fixture")
                .raw_hex
        ),
        b"\x92\xc4\x15Personal Hopspot E290\xc0"
    );
    assert_eq!(
        decode(
            &corpus
                .announce_vectors
                .iter()
                .find(|fixture| fixture.name == "browser_playground_legacy")
                .expect("browser fixture")
                .raw_hex
        ),
        b"\x92\xc4\x17Prns Browser Playground\xc0"
    );
}

#[test]
fn all_python_positive_messages_parse_authenticate_and_select_the_same_lane() {
    let corpus = corpus();
    for expected_name in POSITIVE_NAMES {
        let fixture = fixture(&corpus, expected_name);
        let full_wire = decode(&fixture.full_wire_hex);
        let destination = array::<16>(&fixture.destination_hash_hex);
        let message = parse_bound(fixture, &full_wire, &destination).expect("positive carrier");

        assert_eq!(
            message.normalized_wire_len(),
            full_wire.len(),
            "{expected_name}"
        );
        assert_eq!(message.destination_hash(), &destination, "{expected_name}");
        assert_eq!(
            message.source_hash(),
            &array::<16>(&fixture.source_hash_hex)
        );
        assert_eq!(message.signature(), &array::<64>(&fixture.signature_hex));
        assert_eq!(message.payload().raw(), decode(&fixture.wire_payload_hex));
        assert_eq!(
            message.payload().timestamp_bits(),
            u64::from_be_bytes(array::<8>(&fixture.decoded.timestamp_f64_bits_hex,))
        );
        assert_eq!(
            message.payload().title().as_bytes(),
            decode(&fixture.decoded.title_hex)
        );
        assert_eq!(
            message.payload().content().as_bytes(),
            decode(&fixture.decoded.content_hex)
        );
        assert_eq!(
            message.payload().fields().raw(),
            decode(&fixture.fields_msgpack_hex)
        );
        assert_eq!(message.message_id(), array::<32>(&fixture.message_id_hex));
        assert_eq!(
            message.selection_content_size(),
            saturated_selection_content_size(fixture)
        );

        let ingress_payload = decode(&fixture.ingress.payload_hex);
        let expected_body = if fixture.ingress.carrier_event == "destination_data" {
            ingress_payload.as_slice()
        } else {
            &ingress_payload[16..]
        };
        assert_eq!(message.carrier_body(), expected_body, "{expected_name}");
        let expected_carrier = match fixture.ingress.carrier_event.as_str() {
            "destination_data" => {
                assert_eq!(
                    fixture.ingress.implied_destination_hash_hex.as_deref(),
                    Some(fixture.destination_hash_hex.as_str())
                );
                CarrierKind::Opportunistic
            }
            "link_data" => CarrierKind::LinkDataContextNone,
            "resource_complete" => CarrierKind::ResourceComplete,
            other => panic!("unexpected carrier {other}"),
        };
        assert_eq!(message.carrier_kind(), expected_carrier);
        assert!(message.is_destination_bound());

        let payload4 = decode(&fixture.payload4_hex);
        assert_eq!(
            message.payload().payload_without_stamp_len(),
            payload4.len()
        );
        let mut payload_output = vec![0_u8; payload4.len()];
        assert_eq!(
            message
                .payload()
                .write_payload_without_stamp(&mut payload_output)
                .expect("payload scratch"),
            payload4.len()
        );
        assert_eq!(payload_output, payload4);
        let mut signature_input = vec![0_u8; message.signature_input_len()];
        assert_eq!(
            message
                .write_signature_input(&mut signature_input)
                .expect("signature input scratch"),
            signature_input.len()
        );
        assert!(message
            .write_signature_input(&mut signature_input[..1])
            .is_err());

        let source_public_key = array::<64>(&fixture.source_public_key_hex);
        let source = message
            .bind_source_identity(&source_public_key)
            .expect("source destination binding");
        let verified = message.verify_signature(&source).expect("Python signature");
        let validated = match &fixture.stamp {
            None => verified
                .apply_stamp_policy(StampPolicy::NotRequired)
                .expect("explicit unstamped policy"),
            Some(stamp) if stamp.kind == "ticket" => {
                let ticket = array::<16>(stamp.ticket_hex.as_deref().expect("ticket fixture"));
                verified
                    .apply_stamp_policy(StampPolicy::TrustedPriorTickets(&[ticket]))
                    .expect("trusted prior ticket")
            }
            Some(stamp) if stamp.kind == "proof_of_work" => {
                let target_cost = RequiredStampCost::new(stamp.target_cost.expect("PoW cost"))
                    .expect("policy cost");
                verified
                    .apply_stamp_policy(StampPolicy::ProofOfWork {
                        target_cost,
                        budget: PowBudget::new(3_000),
                    })
                    .expect("Python PoW")
            }
            Some(stamp) => panic!("unexpected stamp kind {}", stamp.kind),
        };
        assert_eq!(validated.message().message_id(), message.message_id());
        match (&fixture.stamp, validated.stamp_admission()) {
            (
                None,
                StampAdmission::NotRequired {
                    stamp_present: false,
                },
            ) => {}
            (Some(stamp), StampAdmission::TrustedPriorTicket) if stamp.kind == "ticket" => {
                assert_eq!(stamp.value, 256);
                assert_eq!(
                    message.payload().stamp().expect("ticket").as_bytes(),
                    decode(&stamp.hex)
                );
            }
            (Some(stamp), StampAdmission::ProofOfWork { target_cost, value })
                if stamp.kind == "proof_of_work" =>
            {
                assert_eq!(target_cost.get(), stamp.target_cost.expect("cost"));
                assert_eq!(value, stamp.value);
                assert_eq!(
                    message.payload().stamp().expect("PoW").as_bytes(),
                    decode(&stamp.hex)
                );
            }
            other => panic!("unexpected admission {other:?}"),
        }

        let desired = if fixture.desired_method == "opportunistic" {
            DesiredDelivery::Opportunistic
        } else {
            DesiredDelivery::Direct
        };
        let decision = select_delivery(desired, saturated_selection_content_size(fixture));
        assert_eq!(
            decision.method,
            if fixture.actual_method == "opportunistic" {
                DeliveryMethod::Opportunistic
            } else {
                DeliveryMethod::Direct
            }
        );
        assert_eq!(
            decision.representation,
            if fixture.representation == "packet" {
                Representation::Packet
            } else {
                Representation::Resource
            }
        );

        let debug = format!("{message:?}");
        for secret in [fixture.title_for_debug(), fixture.content_for_debug()] {
            if !secret.is_empty() {
                assert!(!debug.contains(&secret));
            }
        }
        assert!(!debug.contains(&fixture.signature_hex));
    }
}

#[test]
fn current_message_ticket_field_never_self_authorizes() {
    let corpus = corpus();
    let fixture = fixture(&corpus, "ticket_stamp_16");
    let full_wire = decode(&fixture.full_wire_hex);
    let destination = array::<16>(&fixture.destination_hash_hex);
    let message = parse_bound(fixture, &full_wire, &destination).expect("ticket carrier");
    assert_eq!(
        message.payload().fields().raw(),
        decode(&fixture.fields_msgpack_hex)
    );
    let fields = message.payload().fields().raw();
    let carried_ticket = array::<16>(
        fixture
            .stamp
            .as_ref()
            .and_then(|stamp| stamp.ticket_hex.as_deref())
            .expect("carried FIELD_TICKET"),
    );
    assert_eq!(&fields[..3], &[0x81, 0x0c, 0x92]);
    assert_eq!(&fields[12..14], &[0xc4, 0x10]);
    assert_eq!(&fields[14..], &carried_ticket);

    let public_key = array::<64>(&fixture.source_public_key_hex);
    let source = message
        .bind_source_identity(&public_key)
        .expect("source destination binding");
    let verified = message.verify_signature(&source).expect("Python signature");
    let no_prior_tickets: [[u8; 16]; 0] = [];

    assert_eq!(
        verified
            .apply_stamp_policy(StampPolicy::TrustedPriorTickets(&no_prior_tickets))
            .unwrap_err(),
        StampPolicyError::Rejected {
            kind: StampKind::Ticket
        }
    );
    assert_eq!(
        verified
            .apply_stamp_policy(StampPolicy::TrustedPriorTicketsOrProofOfWork {
                tickets: &no_prior_tickets,
                target_cost: RequiredStampCost::new(1).expect("valid cost"),
                budget: PowBudget::new(3_000),
            })
            .unwrap_err(),
        StampPolicyError::Rejected {
            kind: StampKind::Ticket
        }
    );
}

#[test]
fn public_stamp_helpers_reject_bad_parameters_and_cover_target_boundaries() {
    let message_id = [0_u8; 32];
    let pow_stamp = [0_u8; 32];
    let ticket = [0_u8; 16];

    assert_eq!(
        validate_pow_stamp(&pow_stamp[..31], &message_id, 0, PowBudget::new(3_000)),
        Err(StampError::InvalidPowStampLength { actual: 31 })
    );
    assert_eq!(
        validate_pow_stamp(&pow_stamp, &message_id, 257, PowBudget::new(3_000)),
        Err(StampError::InvalidTargetCost { actual: 257 })
    );
    assert_eq!(
        validate_pow_stamp(&pow_stamp, &message_id, 0, PowBudget::new(2_999)),
        Err(StampError::BudgetExceeded {
            required: 3_000,
            authorized: 2_999,
        })
    );

    let cost_zero = validate_pow_stamp(&pow_stamp, &message_id, 0, PowBudget::new(3_000))
        .expect("cost zero is a valid low-level boundary");
    assert!(cost_zero.valid);
    let cost_256 = validate_pow_stamp(&pow_stamp, &message_id, 256, PowBudget::new(3_000))
        .expect("cost 256 is a valid low-level boundary");
    assert_eq!(cost_256.value, cost_zero.value);
    assert_eq!(cost_256.valid, cost_256.value >= 255);

    assert!(!validate_ticket_stamp(&[0_u8; 15], &message_id, &ticket));
}

impl MessageFixture {
    fn title_for_debug(&self) -> String {
        String::from_utf8_lossy(&decode(&self.decoded.title_hex)).into_owned()
    }

    fn content_for_debug(&self) -> String {
        String::from_utf8_lossy(&decode(&self.decoded.content_hex)).into_owned()
    }
}

#[test]
fn every_negative_mutation_preserves_its_expected_failure_class() {
    let corpus = corpus();
    for expected_name in NEGATIVE_NAMES {
        let mutation = corpus
            .negative_mutations
            .iter()
            .find(|entry| entry.name == expected_name)
            .expect("negative fixture");
        let based_on = fixture(&corpus, &mutation.based_on);
        let wire = decode(&mutation.full_wire_hex);

        if mutation.python_parse.result == "exception" {
            assert_eq!(
                mutation.python_parse.exception_type.as_deref(),
                Some("InsufficientDataException")
            );
            assert!(MessageView::parse_complete(&wire, limits()).is_err());
            continue;
        }

        let message = MessageView::parse_complete(&wire, limits()).expect("structural negative");
        assert_eq!(
            message.message_id(),
            array::<32>(
                mutation
                    .python_parse
                    .message_id_hex
                    .as_deref()
                    .expect("message id")
            )
        );
        let source_public_key = array::<64>(&based_on.source_public_key_hex);
        match mutation.name.as_str() {
            "source_hash_bit_flip" => {
                assert_eq!(
                    mutation.python_parse.unverified_reason.as_deref(),
                    Some("source_unknown")
                );
                assert_eq!(
                    message
                        .bind_source_identity(&source_public_key)
                        .unwrap_err(),
                    SignatureError::SourceHashMismatch
                );
            }
            "signature_bit_flip" | "content_bit_flip" => {
                assert_eq!(mutation.python_parse.signature_validated, Some(false));
                let source = message
                    .bind_source_identity(&source_public_key)
                    .expect("bound source");
                assert!(matches!(
                    message.verify_signature(&source),
                    Err(SignatureError::InvalidSignature
                        | SignatureError::InvalidSignaturePoint
                        | SignatureError::NonPrimeOrderSignaturePoint)
                ));
            }
            "pow_stamp_bit_flip" | "ticket_stamp_bit_flip" => {
                assert_eq!(mutation.python_parse.signature_validated, Some(true));
                let destination = array::<16>(&based_on.destination_hash_hex);
                let bound =
                    parse_bound(based_on, &wire, &destination).expect("bound stamp mutation");
                let source = bound
                    .bind_source_identity(&source_public_key)
                    .expect("bound source");
                let verified = bound
                    .verify_signature(&source)
                    .expect("stamp excluded from signature");
                let expected_stamp = mutation
                    .stamp_validation
                    .as_ref()
                    .expect("stamp validation");
                assert!(!expected_stamp.valid);
                let error = if expected_stamp.kind == "ticket" {
                    let ticket = array::<16>(
                        expected_stamp
                            .ticket_hex
                            .as_deref()
                            .expect("trusted ticket"),
                    );
                    verified.apply_stamp_policy(StampPolicy::TrustedPriorTickets(&[ticket]))
                } else {
                    verified.apply_stamp_policy(StampPolicy::ProofOfWork {
                        target_cost: RequiredStampCost::new(
                            expected_stamp.target_cost.expect("PoW cost"),
                        )
                        .expect("policy cost"),
                        budget: PowBudget::new(3_000),
                    })
                }
                .expect_err("mutated stamp must fail");
                assert_eq!(
                    error,
                    StampPolicyError::Rejected {
                        kind: if expected_stamp.kind == "ticket" {
                            StampKind::Ticket
                        } else {
                            StampKind::ProofOfWork
                        }
                    }
                );
            }
            other => panic!("unhandled negative fixture {other}"),
        }
    }
}

#[test]
fn exact_four_raw_and_stamped_canonicalization_rules_are_distinct() {
    let corpus = corpus();
    let basic = fixture(&corpus, "basic_binary");
    let oracle = &corpus.inbound_oracles[0];
    assert_eq!(oracle.name, "exact_four_array16_raw_hash");
    assert_ne!(oracle.received_payload_hex, oracle.canonical_repack_hex);
    let public_key = array::<64>(&oracle.source_public_key_hex);

    let array16_wire = decode(&oracle.full_wire_hex);
    let raw4 = MessageView::parse_complete(&array16_wire, limits()).expect("array16 exact-four");
    assert_eq!(raw4.payload().raw(), decode(&oracle.received_payload_hex));
    assert_eq!(raw4.message_id(), array::<32>(&oracle.message_id_hex));
    let source = raw4
        .bind_source_identity(&public_key)
        .expect("source binding");
    raw4.verify_signature(&source)
        .expect("Python-signed raw array16 payload");

    let basic_wire = decode(&basic.full_wire_hex);
    let basic_payload = decode(&basic.wire_payload_hex);
    let mut stamped_array16 = basic_wire[..96].to_vec();
    stamped_array16.extend_from_slice(&[0xdc, 0x00, 0x05]);
    stamped_array16.extend_from_slice(&basic_payload[1..]);
    stamped_array16.extend_from_slice(&[0xc4, 0x10]);
    stamped_array16.extend_from_slice(&[0_u8; 16]);
    let stamped = MessageView::parse_complete(&stamped_array16, limits())
        .expect("outer array width excluded from stamped canonical payload");
    assert_eq!(stamped.message_id(), array::<32>(&basic.message_id_hex));
    let source = stamped
        .bind_source_identity(&public_key)
        .expect("source binding");
    stamped
        .verify_signature(&source)
        .expect("canonical first four retain signature");

    let noncanonical_tail = decode(
        "95cb41d954fc40000000c500094772656574696e6773c41648656c6c6f2066726f6d20507974686f6e204c584d4680c41000000000000000000000000000000000",
    );
    let mut noncanonical_stamped = basic_wire[..96].to_vec();
    noncanonical_stamped.extend_from_slice(&noncanonical_tail);
    assert_eq!(
        MessageView::parse_complete(&noncanonical_stamped, limits()).unwrap_err(),
        WireError::NonCanonicalStampedPayload
    );

    let mut array32_raw4 = array16_wire[..96].to_vec();
    array32_raw4.extend_from_slice(&[0xdd, 0, 0, 0, 4]);
    array32_raw4.extend_from_slice(&array16_wire[99..]);
    MessageView::parse_complete(&array32_raw4, limits()).expect("array32 exact-four parser");
}

#[test]
fn malformed_trailing_destination_and_all_limit_classes_fail_closed() {
    let corpus = corpus();
    let basic = fixture(&corpus, "basic_binary");
    let rich = fixture(&corpus, "rich_fields");
    let basic_wire = decode(&basic.full_wire_hex);
    let rich_wire = decode(&rich.full_wire_hex);

    let mut trailing = basic_wire.clone();
    trailing.push(0);
    assert!(matches!(
        MessageView::parse_complete(&trailing, limits()),
        Err(WireError::TrailingBytes { trailing: 1, .. })
    ));
    let mut reserved = basic_wire.clone();
    reserved[97] = 0xc1;
    assert!(matches!(
        MessageView::parse_complete(&reserved, limits()),
        Err(WireError::ReservedMarker { .. })
    ));

    let destination = array::<16>(&basic.destination_hash_hex);
    let mut wrong_destination = destination;
    wrong_destination[0] ^= 1;
    let mut malformed_wrong_destination = basic_wire.clone();
    malformed_wrong_destination[97] = 0xc1;
    assert_eq!(
        MessageView::parse_ingress(
            CarrierIngress::LinkDataContextNone {
                expected_destination: &wrong_destination,
                payload: &malformed_wrong_destination,
            },
            limits(),
        )
        .unwrap_err(),
        WireError::DestinationMismatch
    );

    let limit_cases = [
        (
            WireLimits::new(basic_wire.len() - 1, 2_048, 256, 2_048, 65_536, 16),
            LimitKind::WireBytes,
            &basic_wire,
        ),
        (
            WireLimits::new(4_096, 8, 256, 2_048, 65_536, 16),
            LimitKind::ValueBytes,
            &basic_wire,
        ),
        (
            WireLimits::new(4_096, 2_048, 4, 2_048, 65_536, 16),
            LimitKind::ContainerItems,
            &rich_wire,
        ),
        (
            WireLimits::new(4_096, 2_048, 256, 4, 65_536, 16),
            LimitKind::TotalValues,
            &basic_wire,
        ),
        (
            WireLimits::new(4_096, 2_048, 256, 2_048, 5, 16),
            LimitKind::ScanSteps,
            &rich_wire,
        ),
        (
            WireLimits::new(4_096, 2_048, 256, 2_048, 65_536, 2),
            LimitKind::NestingDepth,
            &rich_wire,
        ),
    ];
    for (wire_limits, expected_kind, wire) in limit_cases {
        assert!(matches!(
            MessageView::parse_complete(wire, wire_limits),
            Err(WireError::LimitExceeded { kind, .. }) if kind == expected_kind
        ));
    }

    let mut nested = vec![0x91; ABSOLUTE_MAX_NESTING_DEPTH + 1];
    nested.push(0xc0);
    let high_caller_limit = WireLimits::new(
        nested.len(),
        nested.len(),
        2,
        nested.len() + 1,
        nested.len() + 1,
        usize::MAX,
    );
    assert!(matches!(
        validate_messagepack_value(&nested, high_caller_limit),
        Err(WireError::LimitExceeded {
            kind: LimitKind::NestingDepth,
            maximum: ABSOLUTE_MAX_NESTING_DEPTH,
            ..
        })
    ));
}

#[test]
fn scanner_covers_extensions_utf8_and_python_map_semantics() {
    let generous = WireLimits::new(70_000, 70_000, 256, 1_024, 8_192, 16);
    for encoded in [
        vec![0xd4, 0x01, 0xaa],
        vec![0xd5, 0x01, 0xaa, 0xbb],
        vec![0xd6, 0x01, 0, 1, 2, 3],
        vec![0xd7, 0x01, 0, 1, 2, 3, 4, 5, 6, 7],
        {
            let mut value = vec![0xd8, 0x01];
            value.extend_from_slice(&[0_u8; 16]);
            value
        },
        vec![0xc7, 0x00, 0x01],
        vec![0xc7, 0x03, 0x01, 1, 2, 3],
        {
            let mut value = vec![0xc8, 0x01, 0x00, 0x01];
            value.extend_from_slice(&[0_u8; 256]);
            value
        },
        {
            let mut value = vec![0xc9, 0x00, 0x01, 0x00, 0x00, 0x01];
            value.extend_from_slice(&vec![0_u8; 65_536]);
            value
        },
    ] {
        let view = validate_messagepack_value(&encoded, generous).expect("generic extension");
        assert_eq!(view.kind(), MessagePackKind::Extension);
        assert_eq!(view.canonicality(), Canonicality::Canonical);
    }

    assert!(matches!(
        validate_messagepack_value(&[0xa1, 0xff], generous),
        Err(WireError::InvalidUtf8 { .. })
    ));
    assert!(matches!(
        validate_messagepack_value(&[0x82, 0x01, 0xc0, 0xcc, 0x01, 0xc0], generous),
        Err(WireError::DuplicateMapKey { .. })
    ));
    assert!(matches!(
        validate_messagepack_value(&[0x81, 0x90, 0xc0], generous),
        Err(WireError::UnsupportedMapKey { .. })
    ));
    let mut float_key = vec![0x81, 0xcb];
    float_key.extend_from_slice(&0_f64.to_bits().to_be_bytes());
    float_key.push(0xc0);
    assert!(matches!(
        validate_messagepack_value(&float_key, generous),
        Err(WireError::UnsupportedMapKey { .. })
    ));
    assert!(matches!(
        validate_messagepack_value(&[0x81, 0x80, 0xc0], generous),
        Err(WireError::UnsupportedMapKey { .. })
    ));
    assert!(matches!(
        validate_messagepack_value(&[0x82, 0xc3, 0xc0, 0x01, 0xc0], generous),
        Err(WireError::DuplicateMapKey { .. })
    ));
    assert!(matches!(
        validate_messagepack_value(
            &[0x82, 0xd4, 0x01, 0xaa, 0xc0, 0xd4, 0x01, 0xaa, 0xc0],
            generous,
        ),
        Err(WireError::DuplicateMapKey { .. })
    ));
    assert!(matches!(
        validate_messagepack_value(&[0xd6, 0xff, 0, 0, 0, 0], generous),
        Err(WireError::UnsupportedTimestampExtension { .. })
    ));

    let tiny = WireLimits::new(8, 8, 8, 8, 8, 4);
    assert!(matches!(
        validate_messagepack_value(&[0xdb, 0xff, 0xff, 0xff, 0xff], tiny),
        Err(WireError::LimitExceeded {
            kind: LimitKind::ValueBytes,
            ..
        })
    ));
    assert!(matches!(
        validate_messagepack_value(&[0xdf, 0xff, 0xff, 0xff, 0xff], tiny),
        Err(WireError::LimitExceeded {
            kind: LimitKind::ContainerItems,
            ..
        })
    ));

    for bits in [
        0_u64,
        (-0.0_f64).to_bits(),
        f64::INFINITY.to_bits(),
        0x7ff8_0000_0000_0042,
    ] {
        let mut encoded = vec![0xcb];
        encoded.extend_from_slice(&bits.to_be_bytes());
        assert_eq!(
            validate_messagepack_value(&encoded, generous)
                .expect("float64 fixed point")
                .canonicality(),
            Canonicality::Canonical
        );
    }
}

#[test]
fn policy_ranges_and_unbound_complete_messages_are_explicit() {
    assert!(RequiredStampCost::new(0).is_err());
    assert!(RequiredStampCost::new(1).is_ok());
    assert!(RequiredStampCost::new(254).is_ok());
    assert!(RequiredStampCost::new(255).is_err());

    let corpus = corpus();
    let basic = fixture(&corpus, "basic_binary");
    let wire = decode(&basic.full_wire_hex);
    let message = MessageView::parse_complete(&wire, limits()).expect("generic complete");
    assert!(!message.is_destination_bound());
    let public_key = array::<64>(&basic.source_public_key_hex);
    let source = message
        .bind_source_identity(&public_key)
        .expect("source binding");
    let verified = message.verify_signature(&source).expect("signature");
    assert_eq!(
        verified
            .apply_stamp_policy(StampPolicy::NotRequired)
            .unwrap_err(),
        StampPolicyError::DestinationUnbound
    );
}

#[test]
fn delivery_threshold_constants_match_exact_boundaries() {
    assert_eq!(ENCRYPTED_PACKET_MAX_CONTENT, 295);
    assert_eq!(LINK_PACKET_MAX_CONTENT, 319);
    assert_eq!(
        select_delivery(DesiredDelivery::Opportunistic, 295),
        prns_lxmf_wire::DeliveryDecision {
            method: DeliveryMethod::Opportunistic,
            representation: Representation::Packet,
        }
    );
    assert_eq!(
        select_delivery(DesiredDelivery::Opportunistic, 296),
        prns_lxmf_wire::DeliveryDecision {
            method: DeliveryMethod::Direct,
            representation: Representation::Packet,
        }
    );
    assert_eq!(
        select_delivery(DesiredDelivery::Direct, 320),
        prns_lxmf_wire::DeliveryDecision {
            method: DeliveryMethod::Direct,
            representation: Representation::Resource,
        }
    );
}

#[test]
fn strict_firmware_profile_rejects_non_prime_order_ed25519_points() {
    let corpus = corpus();
    let basic = fixture(&corpus, "basic_binary");
    let mut weak_key_wire = decode(&basic.full_wire_hex);
    let mut weak_public_key = [0_u8; 64];
    weak_public_key[32] = 1;
    let identity_hash = Sha256::digest(weak_public_key);
    let mut destination_hasher = Sha256::new();
    destination_hasher.update([0x6e, 0xc6, 0x0b, 0xc3, 0x18, 0xe2, 0xc0, 0xf0, 0xd9, 0x08]);
    destination_hasher.update(&identity_hash[..16]);
    let derived = destination_hasher.finalize();
    weak_key_wire[16..32].copy_from_slice(&derived[..16]);
    let weak_key_message =
        MessageView::parse_complete(&weak_key_wire, limits()).expect("structural weak key");
    assert_eq!(
        weak_key_message
            .bind_source_identity(&weak_public_key)
            .unwrap_err(),
        SignatureError::NonPrimeOrderPublicKey
    );

    let mut weak_r_wire = decode(&basic.full_wire_hex);
    weak_r_wire[32..64].fill(0);
    weak_r_wire[32] = 1;
    let weak_r_message =
        MessageView::parse_complete(&weak_r_wire, limits()).expect("structural weak R");
    let public_key = array::<64>(&basic.source_public_key_hex);
    let source = weak_r_message
        .bind_source_identity(&public_key)
        .expect("strong source key");
    assert_eq!(
        weak_r_message.verify_signature(&source).unwrap_err(),
        SignatureError::NonPrimeOrderSignaturePoint
    );

    let mixed_point = ED25519_BASEPOINT_POINT + EIGHT_TORSION[1];
    assert!(!mixed_point.is_small_order());
    assert!(!mixed_point.is_torsion_free());
    let mixed_encoding = mixed_point.compress().to_bytes();

    let mut mixed_public_key = array::<64>(&basic.source_public_key_hex);
    mixed_public_key[32..].copy_from_slice(&mixed_encoding);
    let identity_hash = Sha256::digest(mixed_public_key);
    let mut destination_hasher = Sha256::new();
    destination_hasher.update([0x6e, 0xc6, 0x0b, 0xc3, 0x18, 0xe2, 0xc0, 0xf0, 0xd9, 0x08]);
    destination_hasher.update(&identity_hash[..16]);
    let derived = destination_hasher.finalize();
    let mut mixed_key_wire = decode(&basic.full_wire_hex);
    mixed_key_wire[16..32].copy_from_slice(&derived[..16]);
    let mixed_key_message =
        MessageView::parse_complete(&mixed_key_wire, limits()).expect("structural mixed key");
    assert_eq!(
        mixed_key_message
            .bind_source_identity(&mixed_public_key)
            .unwrap_err(),
        SignatureError::NonPrimeOrderPublicKey
    );

    let mut mixed_r_wire = decode(&basic.full_wire_hex);
    mixed_r_wire[32..64].copy_from_slice(&mixed_encoding);
    let mixed_r_message =
        MessageView::parse_complete(&mixed_r_wire, limits()).expect("structural mixed R");
    let source = mixed_r_message
        .bind_source_identity(&public_key)
        .expect("prime-order source key");
    assert_eq!(
        mixed_r_message.verify_signature(&source).unwrap_err(),
        SignatureError::NonPrimeOrderSignaturePoint
    );
}
